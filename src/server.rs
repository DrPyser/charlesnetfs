use crate::config::Config;
use crate::pinger;
use ninep::fs::{FileMeta, FileType, Mode, Perm, Stat};
use ninep::sansio::server::ClientId;
use ninep::sync::server::{ReadOutcome, Serve9p};
use ninep::Result;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const QID_ROOT: u64 = 0;
const IO_UNIT: ninep::fs::IoUnit = 4096;

fn join_path(base: &str, child: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{}/{}", base, child)
}

pub struct PingFS {
    config: Config,
    ping_results: Mutex<BTreeMap<u64, String>>,
    qid_cache: Mutex<BTreeMap<u64, String>>, // qid -> path
}

impl PingFS {
    pub fn new(config: Config) -> Self {
        let mut qid_cache = BTreeMap::new();

        // Root is always QID 0
        qid_cache.insert(QID_ROOT, "/".to_string());

        // Add all targets and their ping/config files
        for name in config.targets.keys() {
            let dir_path = join_path("/", name);
            let ping_path = join_path(&dir_path, "ping");
            let config_path = join_path(&dir_path, "config");

            let dir_qid = qid_from_path(&dir_path);
            let ping_qid = qid_from_path(&ping_path);
            let config_qid = qid_from_path(&config_path);

            qid_cache.insert(dir_qid, dir_path);
            qid_cache.insert(ping_qid, ping_path);
            qid_cache.insert(config_qid, config_path);
        }

        Self {
            config,
            ping_results: Mutex::new(BTreeMap::new()),
            qid_cache: Mutex::new(qid_cache),
        }
    }
}

/// Compute QID from path. Root "/" always returns QID_ROOT (0).
fn qid_from_path(path: &str) -> u64 {
    if path == "/" {
        return QID_ROOT;
    }

    let sum: u64 = path.bytes().map(|b| b as u64).sum();
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
            if self.config.targets.contains_key(child) {
                let path = join_path("/", child);
                let qid = qid_from_path(&path);
                Ok(FileMeta::dir(child, qid))
            } else {
                Err("not found".into())
            }
        } else {
            let parent_path = self.path_from_qid(parent_qid);
            let path = join_path(&parent_path, child);
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

        let path = self.path_from_qid(qid);
        if self.is_valid_path(&path) {
            return Ok(IO_UNIT);
        }

        Err("not found".into())
    }

    fn read(
        &self,
        _cid: ClientId,
        qid: u64,
        _offset: usize,
        _count: usize,
        _uname: &str,
    ) -> Result<ReadOutcome> {
        if qid == QID_ROOT {
            return Ok(ReadOutcome::Immediate(Vec::new()));
        }

        let path = self.path_from_qid(qid);
        let p = std::path::Path::new(&path);

        if p.ends_with("ping") {
            let target_name = p
                .parent()
                .and_then(|pp| pp.file_name())
                .and_then(|n| n.to_str())
                .ok_or("invalid target name")?;
            let target = self
                .config
                .targets
                .get(target_name)
                .ok_or("target not found")?;

            let addr = &target.addr;
            let timeout = target.timeout(self.config.global_timeout_ms);
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

            let result = pinger::ping(addr, timeout, &payload);

            let response = match result {
                Ok(ping_result) => {
                    format!("pong {}ms", ping_result.latency_ms.unwrap())
                }
                Err(e) => {
                    format!("fail: {}", e)
                }
            };

            Ok(ReadOutcome::Immediate(response.into_bytes()))
        } else if p.ends_with("config") {
            let target_name = p
                .parent()
                .and_then(|pp| pp.file_name())
                .and_then(|n| n.to_str())
                .ok_or("invalid target name")?;
            let target = self
                .config
                .targets
                .get(target_name)
                .ok_or("target not found")?;

            let config_text = format!(
                "addr: {}\ntimeout: {}ms\n",
                target.addr,
                target.timeout(self.config.global_timeout_ms)
            );
            Ok(ReadOutcome::Immediate(config_text.into_bytes()))
        } else {
            Err("not found".into())
        }
    }

    fn read_dir(&self, _cid: ClientId, qid: u64, _uname: &str) -> Result<Vec<Stat>> {
        if qid == QID_ROOT {
            let mut stats = Vec::new();
            for name in self.config.targets.keys() {
                let path = format!("/{}", name);
                let dir_qid = qid_from_path(&path);
                stats.push(Stat {
                    fm: FileMeta::dir(name, dir_qid),
                    perms: Perm::from_bits_truncate(0o755),
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

        let base_path = self.parent_path_from_qid(qid);
        let bp = std::path::Path::new(&base_path);
        let target_name = bp.file_name().and_then(|n| n.to_str());
        if target_name
            .map(|t| self.config.targets.contains_key(t))
            .unwrap_or(false)
        {
            let ping_path = join_path(&base_path, "ping");
            let config_path = join_path(&base_path, "config");

            return Ok(vec![
                Stat {
                    fm: FileMeta::file("ping", qid_from_path(&ping_path)),
                    perms: Perm::from_bits_truncate(0o644),
                    n_bytes: 0,
                    last_accesses: SystemTime::now(),
                    last_modified: SystemTime::now(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    last_modified_by: "root".to_string(),
                },
                Stat {
                    fm: FileMeta::file("config", qid_from_path(&config_path)),
                    perms: Perm::from_bits_truncate(0o444),
                    n_bytes: 0,
                    last_accesses: SystemTime::now(),
                    last_modified: SystemTime::now(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    last_modified_by: "root".to_string(),
                },
            ]);
        }

        Err("not found".into())
    }

    fn stat(&self, _cid: ClientId, qid: u64, _uname: &str) -> Result<Stat> {
        if qid == QID_ROOT {
            return Ok(Stat {
                fm: FileMeta::dir("", QID_ROOT),
                perms: Perm::from_bits_truncate(0o755),
                n_bytes: 0,
                last_accesses: SystemTime::now(),
                last_modified: SystemTime::now(),
                owner: "root".to_string(),
                group: "root".to_string(),
                last_modified_by: "root".to_string(),
            });
        }

        let path = self.path_from_qid(qid);
        let p = std::path::Path::new(&path);

        if p.as_os_str() == "/" {
            return Ok(Stat {
                fm: FileMeta::dir("", QID_ROOT),
                perms: Perm::from_bits_truncate(0o755),
                n_bytes: 0,
                last_accesses: SystemTime::now(),
                last_modified: SystemTime::now(),
                owner: "root".to_string(),
                group: "root".to_string(),
                last_modified_by: "root".to_string(),
            });
        }

        if p.is_absolute() && p.parent() == Some(std::path::Path::new("/")) {
            let target_name = p.file_name().and_then(|n| n.to_str());
            if target_name
                .map(|t| self.config.targets.contains_key(t))
                .unwrap_or(false)
            {
                return Ok(Stat {
                    fm: FileMeta {
                        name: target_name.unwrap().to_string(),
                        ty: FileType::Directory,
                        qid,
                    },
                    perms: Perm::from_bits_truncate(0o755),
                    n_bytes: 0,
                    last_accesses: SystemTime::now(),
                    last_modified: SystemTime::now(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    last_modified_by: "root".to_string(),
                });
            }
        }

        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ty = if name == "ping" || name == "config" {
            FileType::Regular
        } else {
            return Err("not found".into());
        };

        let perms = if name == "config" { 0o444u32 } else { 0o644u32 };

        Ok(Stat {
            fm: FileMeta {
                name: name.to_string(),
                ty,
                qid,
            },
            perms: Perm::from_bits_truncate(perms),
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
        _qid: u64,
        _offset: usize,
        _data: Vec<u8>,
        _uname: &str,
    ) -> Result<usize> {
        Err("not supported".into())
    }

    fn remove(&self, _cid: ClientId, _qid: u64, _uname: &str) -> Result<()> {
        Err("not supported".into())
    }

    fn write_stat(&self, _cid: ClientId, _qid: u64, _stat: Stat, _uname: &str) -> Result<()> {
        Err("not supported".into())
    }
}

impl PingFS {
    fn path_from_qid(&self, qid: u64) -> String {
        self.qid_cache
            .lock()
            .unwrap()
            .get(&qid)
            .cloned()
            .unwrap_or_else(|| "/".to_string())
    }

    fn parent_path_from_qid(&self, qid: u64) -> String {
        let path = self.path_from_qid(qid);
        std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string())
    }

    fn is_valid_path(&self, path: &str) -> bool {
        use std::path::{Component, Path};

        let components: Vec<_> = Path::new(path).components().collect();

        match components[..] {
            [Component::RootDir] => true,
            [Component::RootDir, Component::Normal(name)] => self
                .config
                .targets
                .contains_key(name.to_string_lossy().as_ref()),
            [Component::RootDir, Component::Normal(name), Component::Normal(file)] => {
                let target = name.to_string_lossy();
                let sub = file.to_string_lossy();
                self.config.targets.contains_key(target.as_ref())
                    && matches!(sub.as_ref(), "ping" | "config")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TargetConfig;
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
                addr: "192.168.1.1:22".to_string(),
                timeout_ms: None,
            },
        );
        targets.insert(
            "nas".to_string(),
            TargetConfig {
                addr: "192.168.1.50:22".to_string(),
                timeout_ms: Some(3000),
            },
        );
        Config {
            global_timeout_ms: 5000,
            targets,
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
        eprintln!("router_meta: {:?}", router_meta);

        // Then walk to config
        let result = fs.walk(cid, router_meta.qid, "config", "user");
        eprintln!("result: {:?}", router_meta);
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
    fn test_is_valid_path_root() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path("/"));
    }

    #[test]
    fn test_is_valid_path_target_dir() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path("/router"));
        assert!(fs.is_valid_path("/nas"));
    }

    #[test]
    fn test_is_valid_path_ping() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path("/router/ping"));
        assert!(fs.is_valid_path("/nas/ping"));
    }

    #[test]
    fn test_is_valid_path_config() {
        let fs = PingFS::new(test_config());
        assert!(fs.is_valid_path("/router/config"));
        assert!(fs.is_valid_path("/nas/config"));
    }

    #[test]
    fn test_is_valid_path_invalid() {
        let fs = PingFS::new(test_config());
        assert!(!fs.is_valid_path("/router/invalid"));
        assert!(!fs.is_valid_path("/nonexistent"));
        assert!(!fs.is_valid_path("/router/ping/extra"));
        assert!(!fs.is_valid_path(""));
    }

    #[test]
    fn test_parent_path_from_qid() {
        let fs = PingFS::new(test_config());

        let router_qid = qid_from_path("/router");
        let parent = fs.parent_path_from_qid(router_qid);
        assert_eq!(parent, "/");

        let ping_qid = qid_from_path("/router/ping");
        let parent = fs.parent_path_from_qid(ping_qid);
        assert_eq!(parent, "/router");

        let root_parent = fs.parent_path_from_qid(QID_ROOT);
        assert_eq!(root_parent, "/");
    }
}
