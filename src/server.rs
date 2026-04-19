use crate::config::Config;
use crate::pinger;
use crate::UnixPath;
use ninep::fs::{FileMeta, FileType, Mode, Perm, Stat, WStat};
use ninep::sansio::server::ClientId;
use ninep::sync::server::{ReadOutcome, Serve9p};
use ninep::Result;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

const QID_ROOT: u64 = 0;
const QID_CONFIG: u64 = 1;
const IO_UNIT: ninep::fs::IoUnit = 4096;

pub struct PingFS {
    config: RwLock<Config>,
    ping_results: Mutex<BTreeMap<u64, String>>,
    qid_cache: Mutex<BTreeMap<u64, UnixPath>>, // qid -> path
}

impl PingFS {
    pub fn new(config: Config) -> Self {
        let fs = Self {
            config: RwLock::new(config),
            ping_results: Mutex::new(BTreeMap::new()),
            qid_cache: Mutex::new(BTreeMap::new()),
        };
        fs.rebuild_qid_cache();
        fs
    }

    fn rebuild_qid_cache(&self) {
        let mut cache = self.qid_cache.lock().unwrap();
        let config = self.config.read().unwrap();

        cache.clear();
        cache.insert(QID_ROOT, UnixPath::ROOT);
        cache.insert(QID_CONFIG, UnixPath::ROOT.join("config"));

        log::info!("rebuilding qid cache for {} targets", config.targets.len());

        for name in config.targets.keys() {
            let dir_path = UnixPath::ROOT.join(name);
            let ping_path = dir_path.join("ping");
            let config_path = dir_path.join("config");

            let dir_qid = qid_from_path(&dir_path);
            let ping_qid = qid_from_path(&ping_path);
            let config_qid = qid_from_path(&config_path);

            cache.insert(dir_qid, dir_path);
            cache.insert(ping_qid, ping_path);
            cache.insert(config_qid, config_path);
        }
    }
}

// Compute QID from path. Root "/" always returns QID_ROOT (0).
fn qid_from_path(path: impl Into<UnixPath>) -> u64 {
    let path: UnixPath = path.into();
    let s = path.to_string();
    if s == "/" {
        return QID_ROOT;
    }
    if s == "/config" {
        return QID_CONFIG;
    }

    let sum: u64 = s.bytes().map(|b| b as u64).sum();
    (sum % 0xFFFF_FFFF) + 100
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

impl Serve9p for PingFS {
    fn walk(&self, _cid: ClientId, parent_qid: u64, child: &str, _uname: &str) -> Result<FileMeta> {
        if parent_qid == QID_ROOT {
            if child == "config" {
                return Ok(FileMeta::file("config", QID_CONFIG));
            }
            if self.config.read().unwrap().targets.contains_key(child) {
                let path = UnixPath::ROOT.join(child);
                let qid = qid_from_path(&path);
                Ok(FileMeta::dir(child, qid))
            } else {
                Err("not found".into())
            }
        } else {
            let parent_path = self
                .path_from_qid(parent_qid)
                .ok_or_else(|| "not found".to_string())?;
            let path = parent_path.join(child);
            if self.is_valid_path(&path) {
                let qid = qid_from_path(&path);
                let ty = match child {
                    "ping" | "config" => FileType::Regular,
                    _ => FileType::Directory,
                };
                Ok(FileMeta {
                    name: child.to_string(),
                    ty,
                    qid,
                })
            } else {
                Err("not found".into())
            }
        }
    }

    fn open(
        &self,
        _cid: ClientId,
        qid: u64,
        _mode: Mode,
        _uname: &str,
    ) -> Result<ninep::fs::IoUnit> {
        if qid == QID_ROOT {
            return Ok(IO_UNIT);
        }

        let path = self
            .path_from_qid(qid)
            .ok_or_else(|| "not found".to_string())?;
        if self.is_valid_path(&path) {
            return Ok(IO_UNIT);
        }

        Err("not found".into())
    }

    fn read(
        &self,
        _cid: ClientId,
        qid: u64,
        offset: usize,
        count: usize,
        _uname: &str,
    ) -> Result<ReadOutcome> {
        if qid == QID_ROOT {
            return Ok(ReadOutcome::Immediate(Vec::new()));
        }

        if qid == QID_CONFIG {
            log::debug!("read /config offset={} count={}", offset, count);
            let config = self.config.read().unwrap();
            let yaml = serde_yaml::to_string(&*config).map_err(|e| e.to_string())?;
            let data = yaml.into_bytes();
            if offset >= data.len() {
                return Ok(ReadOutcome::Immediate(Vec::new()));
            }
            let end = (offset + count).min(data.len());
            let data = &data[offset..end];
            return Ok(ReadOutcome::Immediate(data.to_vec()));
        }

        let path = self
            .path_from_qid(qid)
            .ok_or_else(|| "not found".to_string())?;

        if path.ends_with("ping") {
            let parent_path = path.parent().ok_or("invalid target name")?;
            let target_name = parent_path.file_name().ok_or("invalid target name")?;
            let config = self.config.read().unwrap();
            let target = config.targets.get(target_name).ok_or("target not found")?;

            let addr = target.addr.to_string();
            let timeout = target.timeout(self.config.read().unwrap().global_timeout_ms);
            let counter = self
                .ping_results
                .lock()
                .unwrap()
                .entry(qid)
                .and_modify(|_| {})
                .or_insert_with(|| "0".to_string())
                .parse::<u64>()
                .unwrap_or(0)
                + 1;

            let payload = format!("{}_{}", current_timestamp(), counter);

            self.ping_results
                .lock()
                .unwrap()
                .insert(qid, counter.to_string());

            let result = pinger::ping(&addr, timeout, &payload);

            let response = match result {
                Ok(ping_result) => {
                    let ms = ping_result.latency_ms.unwrap();
                    log::debug!("ping {} → {}ms", addr, ms);
                    format!("pong {}ms", ms)
                }
                Err(e) => {
                    log::debug!("ping {} → failed: {}", addr, e);
                    format!("fail: {}", e)
                }
            };

            let data = response.into_bytes();
            if offset >= data.len() {
                return Ok(ReadOutcome::Immediate(Vec::new()));
            }
            let end = (offset + count).min(data.len());
            let data = &data[offset..end];
            Ok(ReadOutcome::Immediate(data.to_vec()))
        } else if path.ends_with("config") {
            let parent_path = path.parent().ok_or("invalid target name")?;
            let target_name = parent_path.file_name().ok_or("invalid target name")?;
            log::debug!(
                "read {}/config offset={} count={}",
                target_name,
                offset,
                count
            );
            let config = self.config.read().unwrap();
            let target = config.targets.get(target_name).ok_or("target not found")?;

            let config_text = format!(
                "addr: {}\ntimeout: {}ms\n",
                target.addr,
                target.timeout(self.config.read().unwrap().global_timeout_ms)
            );

            let data = config_text.into_bytes();
            if offset >= data.len() {
                return Ok(ReadOutcome::Immediate(Vec::new()));
            }
            let end = (offset + count).min(data.len());
            let data = &data[offset..end];
            Ok(ReadOutcome::Immediate(data.to_vec()))
        } else {
            Err("not found".into())
        }
    }

    fn read_dir(&self, _cid: ClientId, qid: u64, _uname: &str) -> Result<Vec<Stat>> {
        if qid == QID_ROOT {
            let mut stats = Vec::new();

            // Add /config file at root
            stats.push(Stat {
                fm: FileMeta::file("config", QID_CONFIG),
                perms: Perm::FILE
                    | Perm::OWNER_READ
                    | Perm::OWNER_WRITE
                    | Perm::GROUP_READ
                    | Perm::OTHER_READ,
                n_bytes: 0,
                last_accesses: SystemTime::now(),
                last_modified: SystemTime::now(),
                owner: "root".to_string(),
                group: "root".to_string(),
                last_modified_by: "root".to_string(),
            });

            for name in self.config.read().unwrap().targets.keys() {
                let path = UnixPath::ROOT.join(name);
                let dir_qid = qid_from_path(&path);
                stats.push(Stat {
                    fm: FileMeta::dir(name, dir_qid),
                    perms: Perm::DIR
                        | Perm::OWNER_READ
                        | Perm::OWNER_WRITE
                        | Perm::OWNER_EXEC
                        | Perm::GROUP_READ
                        | Perm::GROUP_EXEC
                        | Perm::OTHER_READ
                        | Perm::OTHER_EXEC,
                    n_bytes: 0,
                    last_accesses: SystemTime::now(),
                    last_modified: SystemTime::now(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    last_modified_by: "root".to_string(),
                });
            }
            return Ok(stats);
        }

        let base_path = self
            .path_from_qid(qid)
            .ok_or_else(|| "not found".to_string())?;

        if !self.is_valid_path(&base_path) {
            return Err("not found".to_string());
        }

        if base_path.file_name().is_none() {
            return Err("not found".to_string());
        }

        Ok(vec![
            Stat {
                fm: FileMeta::file("ping", qid_from_path(base_path.join("ping"))),
                perms: Perm::OWNER_READ | Perm::OWNER_WRITE | Perm::GROUP_READ | Perm::OTHER_READ,
                n_bytes: 0,
                last_accesses: SystemTime::now(),
                last_modified: SystemTime::now(),
                owner: "root".to_string(),
                group: "root".to_string(),
                last_modified_by: "root".to_string(),
            },
            Stat {
                fm: FileMeta::file("config", qid_from_path(base_path.join("config"))),
                perms: Perm::OWNER_READ | Perm::GROUP_READ | Perm::OTHER_READ,
                n_bytes: 0,
                last_accesses: SystemTime::now(),
                last_modified: SystemTime::now(),
                owner: "root".to_string(),
                group: "root".to_string(),
                last_modified_by: "root".to_string(),
            },
        ])
    }

    fn stat(&self, _cid: ClientId, qid: u64, _uname: &str) -> Result<Stat> {
        if qid == QID_ROOT {
            return Ok(Stat {
                fm: FileMeta::dir("", QID_ROOT),
                perms: Perm::DIR
                    | Perm::OWNER_READ
                    | Perm::OWNER_WRITE
                    | Perm::OWNER_EXEC
                    | Perm::GROUP_READ
                    | Perm::GROUP_EXEC
                    | Perm::OTHER_READ
                    | Perm::OTHER_EXEC,
                n_bytes: 0,
                last_accesses: SystemTime::now(),
                last_modified: SystemTime::now(),
                owner: "root".to_string(),
                group: "root".to_string(),
                last_modified_by: "root".to_string(),
            });
        }

        let path = self
            .path_from_qid(qid)
            .ok_or_else(|| "not found".to_string())?;

        assert!(!path.is_root());

        if path.parent().map(|p| p.is_root()).unwrap_or(false) {
            let target_name = path.file_name();
            if target_name
                .map(|t| self.config.read().unwrap().targets.contains_key(t))
                .unwrap_or(false)
            {
                return Ok(Stat {
                    fm: FileMeta {
                        name: target_name.unwrap().to_string(),
                        ty: FileType::Directory,
                        qid,
                    },
                    perms: Perm::DIR
                        | Perm::OWNER_READ
                        | Perm::OWNER_WRITE
                        | Perm::OWNER_EXEC
                        | Perm::GROUP_READ
                        | Perm::GROUP_EXEC
                        | Perm::OTHER_READ
                        | Perm::OTHER_EXEC,
                    n_bytes: 0,
                    last_accesses: SystemTime::now(),
                    last_modified: SystemTime::now(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    last_modified_by: "root".to_string(),
                });
            }
        }

        let name = path.file_name().unwrap_or("");
        let ty = if name == "ping" || name == "config" {
            FileType::Regular
        } else {
            return Err("not found".into());
        };

        let perms = if name == "config" && !path.parent().map(|p| p.is_root()).unwrap_or(false) {
            Perm::OWNER_READ | Perm::GROUP_READ | Perm::OTHER_READ
        } else {
            // includes /config
            Perm::OWNER_READ | Perm::OWNER_WRITE | Perm::GROUP_READ | Perm::OTHER_READ
        };

        Ok(Stat {
            fm: FileMeta {
                name: name.to_string(),
                ty,
                qid,
            },
            perms,
            n_bytes: 0,
            last_accesses: SystemTime::now(),
            last_modified: SystemTime::now(),
            owner: "root".to_string(),
            group: "root".to_string(),
            last_modified_by: "root".to_string(),
        })
    }

    fn create(
        &self,
        _cid: ClientId,
        _parent: u64,
        _name: &str,
        _perm: Perm,
        _mode: Mode,
        _uname: &str,
    ) -> Result<(FileMeta, ninep::fs::IoUnit)> {
        Err("not supported".into())
    }

    fn write(
        &self,
        _cid: ClientId,
        qid: u64,
        offset: usize,
        data: Vec<u8>,
        _uname: &str,
    ) -> Result<usize> {
        if qid == QID_CONFIG {
            if offset > 0 {
                return Err("partial writes not supported".into());
            }

            let len = data.len();
            let yaml = String::from_utf8(data).map_err(|_| "invalid utf-8")?;

            let new_config: Config =
                serde_yaml::from_str(&yaml).map_err(|e| format!("invalid config: {}", e))?;

            if new_config.targets.is_empty() {
                return Err("config must have at least one target".into());
            }

            *self.config.write().unwrap() = new_config;
            self.rebuild_qid_cache();

            if let Ok(config) = self.config.read() {
                log::info!(
                    "config updated ({} targets, default timeout {}ms)",
                    config.targets.len(),
                    config.global_timeout_ms
                );
            }

            Ok(len)
        } else {
            Err("not supported".into())
        }
    }

    fn remove(&self, _cid: ClientId, _qid: u64, _uname: &str) -> Result<()> {
        Err("not supported".into())
    }

    fn write_stat(&self, _cid: ClientId, _qid: u64, _stat: WStat, _uname: &str) -> Result<()> {
        Ok(())
    }
}

impl PingFS {
    fn path_from_qid(&self, qid: u64) -> Option<UnixPath> {
        self.qid_cache.lock().unwrap().get(&qid).cloned()
    }

    fn is_valid_path(&self, path: &UnixPath) -> bool {
        let components: Vec<_> = path.components().collect();

        match components.as_slice() {
            [] => true,
            ["config"] => true,
            [name] => self.config.read().unwrap().targets.contains_key(*name),
            [name, file] => {
                self.config.read().unwrap().targets.contains_key(*name)
                    && matches!(*file, "ping" | "config")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Addr, TargetConfig};
    use ninep::sansio::server::ClientId;

    #[test]
    fn test_qid_from_path_root() {
        assert_eq!(qid_from_path("/"), QID_ROOT);
    }

    #[test]
    fn test_qid_from_path_router() {
        let qid = qid_from_path("/router");
        assert!(qid > 100);
    }

    #[test]
    fn test_qid_deterministic() {
        assert_eq!(qid_from_path("/router"), qid_from_path("/router"));
    }

    #[test]
    fn test_different_paths_different_qids() {
        assert_ne!(qid_from_path("/router"), qid_from_path("/nas"));
    }

    fn test_config() -> Config {
        use std::collections::BTreeMap;
        let mut targets = BTreeMap::new();
        targets.insert(
            "router".to_string(),
            TargetConfig {
                addr: Addr::parse("192.168.1.1:22").unwrap(),
                ..Default::default()
            },
        );
        targets.insert(
            "nas".to_string(),
            TargetConfig {
                addr: Addr::parse("192.168.1.50:22").unwrap(),
                timeout_ms: Some(3000),
                ..Default::default()
            },
        );
        Config {
            targets,
            ..Config::default()
        }
    }

    fn dummy_client_id() -> ClientId {
        unsafe { std::mem::transmute(12345_u64) }
    }

    #[test]
    fn test_cache_initialization() {
        let fs = PingFS::new(test_config());
        let cache = fs.qid_cache.lock().unwrap();

        // Should have root + 2 targets * 3 files (dir, ping, config)
        assert!(cache.len() >= 7);
    }

    #[test]
    fn test_walk_root_to_target() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.walk(cid, QID_ROOT, "router", "user");
        assert!(result.is_ok());

        let meta = result.unwrap();
        assert_eq!(meta.name, "router");
        assert_eq!(meta.ty, FileType::Directory);
        assert!(meta.qid > 0);
    }

    #[test]
    fn test_walk_root_to_nonexistent() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.walk(cid, QID_ROOT, "nonexistent", "user");
        assert!(result.is_err());
    }

    #[test]
    fn test_walk_target_to_ping() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        // First walk to router
        let router_meta = fs.walk(cid, QID_ROOT, "router", "user").unwrap();

        // Then walk to ping
        let result = fs.walk(cid, router_meta.qid, "ping", "user");
        assert!(result.is_ok());

        let meta = result.unwrap();
        assert_eq!(meta.name, "ping");
        assert_eq!(meta.ty, FileType::Regular);
    }

    #[test]
    fn test_walk_target_to_config() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        // First walk to router
        let router_meta = fs.walk(cid, QID_ROOT, "router", "user").unwrap();

        // Then walk to config
        let result = fs.walk(cid, router_meta.qid, "config", "user");
        assert!(result.is_ok());

        let meta = result.unwrap();
        assert_eq!(meta.name, "config");
        assert_eq!(meta.ty, FileType::Regular);
    }

    #[test]
    fn test_walk_invalid_file() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let router_meta = fs.walk(cid, QID_ROOT, "router", "user").unwrap();
        let result = fs.walk(cid, router_meta.qid, "invalid", "user");

        assert!(result.is_err());
    }

    #[test]
    fn test_stat_root() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.stat(cid, QID_ROOT, "user");
        assert!(result.is_ok());
        let stat = result.unwrap();
        assert_eq!(stat.fm.ty, FileType::Directory);
        assert_eq!(stat.fm.name, "");
    }

    #[test]
    fn test_stat_target() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let router_qid = qid_from_path("/router");
        let result = fs.stat(cid, router_qid, "user");
        assert!(result.is_ok());
        let stat = result.unwrap();
        assert_eq!(stat.fm.ty, FileType::Directory);
        assert_eq!(stat.fm.name, "router");
    }

    #[test]
    fn test_stat_ping() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let ping_qid = qid_from_path("/router/ping");
        let result = fs.stat(cid, ping_qid, "user");
        assert!(result.is_ok());
        let stat = result.unwrap();
        assert_eq!(stat.fm.ty, FileType::Regular);
        assert_eq!(stat.fm.name, "ping");
    }

    #[test]
    fn test_stat_config() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/router/config");
        let result = fs.stat(cid, config_qid, "user");
        assert!(result.is_ok());
        let stat = result.unwrap();
        assert_eq!(stat.fm.ty, FileType::Regular);
        assert_eq!(stat.fm.name, "config");
    }

    #[test]
    fn test_stat_global_config_is_writable() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/config");
        let result = fs.stat(cid, config_qid, "user").unwrap();

        assert!(result.perms.intersects(Perm::OWNER_WRITE));
    }

    #[test]
    fn test_stat_target_config_is_readonly() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/router/config");
        let result = fs.stat(cid, config_qid, "user").unwrap();

        assert!(!result.perms.intersects(Perm::OWNER_WRITE));
    }

    #[test]
    fn test_stat_invalid_qid() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.stat(cid, 999999, "user");
        assert!(result.is_err());
    }

    #[test]
    fn test_is_valid_path_root() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path(&UnixPath::new("/")));
    }

    #[test]
    fn test_is_valid_path_target_dir() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path(&UnixPath::new("/router")));
        assert!(fs.is_valid_path(&UnixPath::new("/nas")));
    }

    #[test]
    fn test_is_valid_path_ping() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path(&UnixPath::new("/router/ping")));
        assert!(fs.is_valid_path(&UnixPath::new("/nas/ping")));
    }

    #[test]
    fn test_is_valid_path_config() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path(&UnixPath::new("/router/config")));
        assert!(fs.is_valid_path(&UnixPath::new("/nas/config")));
    }

    #[test]
    fn test_is_valid_path_invalid() {
        let fs = PingFS::new(test_config());
        assert!(!fs.is_valid_path(&UnixPath::new("/router/invalid")));
        assert!(!fs.is_valid_path(&UnixPath::new("/nonexistent")));
        assert!(!fs.is_valid_path(&UnixPath::new("/router/ping/extra")));
    }

    #[test]
    fn test_open_root() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.open(cid, QID_ROOT, Mode::FILE, "user");
        assert!(result.is_ok());
    }

    #[test]
    fn test_open_valid_target() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let router_qid = qid_from_path("/router");
        let result = fs.open(cid, router_qid, Mode::FILE, "user");
        assert!(result.is_ok());
    }

    #[test]
    fn test_open_valid_ping() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let ping_qid = qid_from_path("/router/ping");
        let result = fs.open(cid, ping_qid, Mode::FILE, "user");
        assert!(result.is_ok());
    }

    #[test]
    fn test_open_invalid_qid() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.open(cid, 999999, Mode::FILE, "user");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_root() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.read(cid, QID_ROOT, 0, 100, "user");
        assert!(result.is_ok());
        let outcome = result.unwrap();
        match outcome {
            ReadOutcome::Immediate(data) => assert!(data.is_empty()),
            _ => panic!("expected Immediate"),
        }
    }

    #[test]
    fn test_read_config() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/router/config");
        let result = fs.read(cid, config_qid, 0, 100, "user");
        assert!(result.is_ok());
        let outcome = result.unwrap();
        match outcome {
            ReadOutcome::Immediate(data) => {
                let content = String::from_utf8(data).unwrap();
                assert!(content.contains("addr:"));
                assert!(content.contains("timeout:"));
            }
            _ => panic!("expected Immediate"),
        }
    }

    #[test]
    fn test_read_ping() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let ping_qid = qid_from_path("/router/ping");
        let result = fs.read(cid, ping_qid, 0, 100, "user");
        assert!(result.is_ok());
        let outcome = result.unwrap();
        match outcome {
            ReadOutcome::Immediate(data) => {
                let content = String::from_utf8(data).unwrap();
                assert!(content.starts_with("pong ") || content.starts_with("fail:"));
            }
            _ => panic!("expected Immediate"),
        }
    }

    #[test]
    fn test_read_invalid_qid() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.read(cid, 999999, 0, 100, "user");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_dir_root() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.read_dir(cid, QID_ROOT, "user");
        assert!(result.is_ok());
        let stats = result.unwrap();
        assert_eq!(stats.len(), 3); // config, router and nas
        assert!(stats.iter().any(|s| s.fm.name == "router"));
        assert!(stats.iter().any(|s| s.fm.name == "nas"));
        assert!(stats.iter().any(|s| s.fm.name == "config"));
    }

    #[test]
    fn test_read_dir_target() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let router_qid = qid_from_path("/router");
        let result = fs.read_dir(cid, router_qid, "user");
        assert!(result.is_ok());
        let stats = result.unwrap();
        assert_eq!(stats.len(), 2); // ping and config
        assert!(stats.iter().any(|s| s.fm.name == "ping"));
        assert!(stats.iter().any(|s| s.fm.name == "config"));
    }

    #[test]
    fn test_read_dir_invalid_qid() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.read_dir(cid, 999999, "user");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_unsupported() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.create(cid, QID_ROOT, "newfile", Perm::FILE, Mode::FILE, "user");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not supported"));
    }

    #[test]
    fn test_write_unsupported() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.write(cid, QID_ROOT, 0, vec![], "user");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not supported"));
    }

    #[test]
    fn test_remove_unsupported() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.remove(cid, QID_ROOT, "user");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not supported"));
    }

    #[test]
    fn test_write_stat_no_change_succeeds() {
        use ninep::sansio::protocol::RawStat;
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();
        let raw = RawStat {
            name: String::new(),
            mode: u32::MAX,
            length: u64::MAX,
            atime: u32::MAX,
            mtime: u32::MAX,
            gid: String::new(),
            muid: String::new(),
            ..Default::default()
        };

        let wstat = WStat::from(raw);

        let result = fs.write_stat(cid, QID_ROOT, wstat, "user");
        assert!(result.is_ok());
    }

    #[test]
    fn test_read_target_config_with_offset() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/router/config");
        let result = fs.read(cid, config_qid, 0, 100, "user").unwrap();
        let ReadOutcome::Immediate(full_data) = result else {
            panic!()
        };
        let full_content = String::from_utf8(full_data).unwrap();

        let result = fs.read(cid, config_qid, 5, 100, "user").unwrap();
        let ReadOutcome::Immediate(partial_data) = result else {
            panic!()
        };
        let partial_content = String::from_utf8(partial_data).unwrap();

        assert_eq!(&full_content[5..], partial_content);
    }

    #[test]
    fn test_read_target_config_offset_returns_empty_at_end() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/router/config");
        let result = fs.read(cid, config_qid, 0, 100, "user").unwrap();
        let ReadOutcome::Immediate(data) = result else {
            panic!()
        };
        let len = data.len();

        let result = fs.read(cid, config_qid, len, 100, "user").unwrap();
        let ReadOutcome::Immediate(data) = result else {
            panic!()
        };
        assert!(data.is_empty());
    }

    #[test]
    fn test_read_config_file() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/config");
        let result = fs.read(cid, config_qid, 0, 1000, "user");
        assert!(result.is_ok());
        let outcome = result.unwrap();
        match outcome {
            ReadOutcome::Immediate(data) => {
                let content = String::from_utf8(data).unwrap();
                assert!(content.contains("global_timeout_ms:"));
                assert!(content.contains("targets:"));
                assert!(content.contains("router:"));
                assert!(content.contains("nas:"));
            }
            _ => panic!("expected Immediate"),
        }
    }

    #[test]
    fn test_read_config_file_with_offset() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let config_qid = qid_from_path("/config");
        let result = fs.read(cid, config_qid, 0, 1000, "user").unwrap();
        let ReadOutcome::Immediate(full_data) = result else {
            panic!()
        };
        let full_content = String::from_utf8(full_data).unwrap();

        let result = fs.read(cid, config_qid, 5, 1000, "user").unwrap();
        let ReadOutcome::Immediate(partial_data) = result else {
            panic!()
        };
        let partial_content = String::from_utf8(partial_data).unwrap();

        assert_eq!(&full_content[5..], partial_content);
    }

    #[test]
    fn test_write_config_valid() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();
        let config_qid = qid_from_path("/config");

        let yaml = "global_timeout_ms: 3000\ntargets:\n  router:\n    addr: 10.0.0.1:22\n";
        let result = fs.write(cid, config_qid, 0, yaml.as_bytes().to_vec(), "user");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), yaml.len());

        let config = fs.config.read().unwrap();
        assert_eq!(config.global_timeout_ms, 3000);
        assert!(config.targets.contains_key("router"));
        assert_eq!(config.targets["router"].addr.to_string(), "10.0.0.1:22");

        let qid_cache = fs.qid_cache.lock().unwrap();
        assert!(qid_cache.values().any(|p| p.to_string() == "/router"));
        assert!(qid_cache.values().any(|p| p.to_string() == "/router/ping"));
        assert!(qid_cache
            .values()
            .any(|p| p.to_string() == "/router/config"));
    }

    #[test]
    fn test_write_config_invalid_yaml() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();
        let config_qid = qid_from_path("/config");

        let orig_global = fs.config.read().unwrap().global_timeout_ms;
        let orig_targets = fs.config.read().unwrap().targets.len();
        let orig_cache_len = fs.qid_cache.lock().unwrap().len();

        let yaml = "invalid: yaml: content:";
        let result = fs.write(cid, config_qid, 0, yaml.as_bytes().to_vec(), "user");

        assert!(result.is_err());

        let config = fs.config.read().unwrap();
        assert_eq!(config.global_timeout_ms, orig_global);
        assert_eq!(config.targets.len(), orig_targets);
        assert_eq!(fs.qid_cache.lock().unwrap().len(), orig_cache_len);
    }

    #[test]
    fn test_write_config_empty_targets() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();
        let config_qid = qid_from_path("/config");

        let orig_global = fs.config.read().unwrap().global_timeout_ms;
        let orig_targets = fs.config.read().unwrap().targets.len();
        let orig_cache_len = fs.qid_cache.lock().unwrap().len();

        let yaml = "global_timeout_ms: 5000\ntargets:\n";
        let result = fs.write(cid, config_qid, 0, yaml.as_bytes().to_vec(), "user");

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least one target"));

        let config = fs.config.read().unwrap();
        assert_eq!(config.global_timeout_ms, orig_global);
        assert_eq!(config.targets.len(), orig_targets);
        assert_eq!(fs.qid_cache.lock().unwrap().len(), orig_cache_len);
    }

    #[test]
    fn test_write_config_with_offset_fails() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();
        let config_qid = qid_from_path("/config");

        let orig_global = fs.config.read().unwrap().global_timeout_ms;
        let orig_targets = fs.config.read().unwrap().targets.len();
        let orig_cache_len = fs.qid_cache.lock().unwrap().len();

        let yaml = "global_timeout_ms: 3000\ntargets:\n  router:\n    addr: 10.0.0.1:22\n";
        let result = fs.write(cid, config_qid, 5, yaml.as_bytes().to_vec(), "user");

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("partial writes not supported"));

        let config = fs.config.read().unwrap();
        assert_eq!(config.global_timeout_ms, orig_global);
        assert_eq!(config.targets.len(), orig_targets);
        assert_eq!(fs.qid_cache.lock().unwrap().len(), orig_cache_len);
    }

    #[test]
    fn test_read_dir_includes_config() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();

        let result = fs.read_dir(cid, QID_ROOT, "user").unwrap();
        let has_config = result.iter().any(|s| s.fm.name == "config");
        assert!(has_config, "root should contain /config");
    }

    #[test]
    fn test_read_dir_shows_new_targets_after_reload() {
        let fs = PingFS::new(test_config());
        let cid = dummy_client_id();
        let config_qid = qid_from_path("/config");

        // Initial config: router, nas
        let result = fs.read_dir(cid, QID_ROOT, "user").unwrap();
        let names: Vec<_> = result.iter().map(|s| s.fm.name.as_str()).collect();
        assert!(names.contains(&"router"));
        assert!(names.contains(&"nas"));
        assert!(!names.contains(&"laptop"));

        // Reload config with new target
        let yaml = "global_timeout_ms: 5000\ntargets:\n  router:\n    addr: 192.168.1.1:22\n  nas:\n    addr: 192.168.1.100:5000\n  laptop:\n    addr: 10.0.0.5:22\n";
        fs.write(cid, config_qid, 0, yaml.as_bytes().to_vec(), "user")
            .unwrap();

        // Verify new target appears
        let result = fs.read_dir(cid, QID_ROOT, "user").unwrap();
        let names: Vec<_> = result.iter().map(|s| s.fm.name.as_str()).collect();
        assert!(names.contains(&"laptop"));
    }
}
