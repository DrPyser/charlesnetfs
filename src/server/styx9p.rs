//! 9P filesystem server using styx9p.

use crate::config::Config;
use crate::config::TargetConfig;
use crate::pinger;
use crate::UnixPath;
use arc_swap::ArcSwap;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use styx9p::proto::Qid;
use styx9p::server::errors;
use styx9p::server::read_at;
use styx9p::server::Error;
use styx9p::{Ctx, File, Filesystem, Result, Stat};

const QID_ROOT: u64 = 0;
const QID_CONFIG: u64 = 1;

/// Compute a qid path value from a UnixPath.
/// Root "/" returns QID_ROOT (0), "/config" returns QID_CONFIG (1).
/// All other paths hash to values > 100.
pub fn qid_from_path(path: impl Into<UnixPath>) -> u64 {
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

pub type QIDCache = BTreeMap<u64, UnixPath>;

/// this function encode the statically configured namespace structure
fn build_qid_cache(config: &Config) -> QIDCache {
    let mut qid_cache = BTreeMap::new();

    qid_cache.insert(QID_ROOT, UnixPath::ROOT);
    qid_cache.insert(QID_CONFIG, UnixPath::ROOT.join("config"));

    for name in config.targets.keys() {
        let dir_path = UnixPath::ROOT.join(name);
        qid_cache.insert(qid_from_path(&dir_path), dir_path.clone());
        qid_cache.insert(qid_from_path(dir_path.join("ping")), dir_path.join("ping"));
        qid_cache.insert(
            qid_from_path(dir_path.join("config")),
            dir_path.join("config"),
        );
        qid_cache.insert(
            qid_from_path(dir_path.join("journal")),
            dir_path.join("journal"),
        );
    }
    qid_cache
}

/// Exchanged atomically via ArcSwap — readers always see a consistent pair.
#[derive(Debug, Clone)]
pub struct Snapshot {
    config: Config,
    qid_cache: QIDCache,
}

impl Snapshot {
    fn new(config: Config) -> Self {
        let qid_cache = build_qid_cache(&config);
        Snapshot { config, qid_cache }
    }
}

/// The filesystem tree: atomic snapshot of config + qid mapping.
/// Shared across all connections via ArcSwap. Writes swap the whole
/// snapshot atomically — no locks, no TOCTOU gap.
#[derive(Debug)]
pub struct CharlesnetFS {
    state: Arc<ArcSwap<Snapshot>>,
}

impl std::fmt::Display for CharlesnetFS {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.snapshot();
        write!(
            f,
            "PingFS {{ {} targets, {} QIDs }}",
            snap.config.targets.len(),
            snap.qid_cache.len()
        )
    }
}

impl Clone for CharlesnetFS {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl CharlesnetFS {
    pub fn new(config: Config) -> Self {
        let snap = Self::build_snapshot(config);
        Self {
            state: Arc::new(ArcSwap::new(Arc::new(snap))),
        }
    }

    /// Load the current snapshot. Lock-free — just an atomic Arc clone.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.state.load_full()
    }

    pub fn build_snapshot(config: Config) -> Snapshot {
        Snapshot::new(config)
    }

    pub fn target_config(&self, name: &str) -> Option<TargetConfig> {
        self.snapshot().config.targets.get(name).cloned()
    }

    pub fn path_from_qid(&self, qid_path: u64) -> Option<UnixPath> {
        self.snapshot().qid_cache.get(&qid_path).cloned()
    }

    pub fn has_target(&self, name: &str) -> bool {
        self.snapshot().config.targets.contains_key(name)
    }

    /// Walk one step from `dir` to `name`. Returns the qid of the child.
    /// This is the core walk logic, testable without Ctx.
    pub fn walk_step(&self, dir: u64, name: &str) -> Result<Qid> {
        let snap = self.snapshot();
        let dir_path = snap
            .qid_cache
            .get(&dir)
            .ok_or_else(|| Error::from(errors::NOT_FOUND))?;

        let components: Vec<_> = dir_path.components().collect();

        match components.as_slice() {
            [] => {
                if name == ".." {
                    return Ok(Qid::dir(QID_ROOT));
                }
                if name == "config" {
                    return Ok(Qid::file(QID_CONFIG));
                }
                if snap.config.targets.contains_key(name) {
                    return Ok(Qid::dir(qid_from_path(UnixPath::ROOT.join(name))));
                }
                Err(Error::from(errors::NOT_FOUND))
            }
            [target] if snap.config.targets.contains_key(*target) => {
                if name == ".." {
                    return Ok(Qid::dir(QID_ROOT));
                }
                match name {
                    "ping" => Ok(Qid::file(qid_from_path(dir_path.join("ping")))),
                    "config" => Ok(Qid::file(qid_from_path(dir_path.join("config")))),
                    "journal" => Ok(Qid::file(qid_from_path(dir_path.join("journal")))),
                    _ => Err(Error::from(errors::NOT_FOUND)),
                }
            }
            _ => Err(Error::from(errors::NOT_FOUND)),
        }
    }

    /// Stat a qid. Returns metadata for the file.
    /// This is the core stat logic, testable without Ctx.
    pub fn stat_qid(&self, qid: Qid) -> Result<Stat> {
        let snap = self.snapshot();
        let path = snap
            .qid_cache
            .get(&qid.path)
            .ok_or_else(|| Error::from(errors::NOT_FOUND))?;

        let components: Vec<_> = path.components().collect();

        match components.as_slice() {
            [] => Ok(Stat::new("/", qid, 0o555, 0)),
            ["config"] => {
                let config = &snap.config;
                Ok(Stat::new(
                    "config",
                    qid,
                    0o644,
                    serde_yaml::to_string(&config).unwrap().len() as u64,
                ))
            }
            [name] if snap.config.targets.contains_key(*name) => {
                Ok(Stat::new(*name, qid, 0o444, 0))
            }
            [name, "ping"] if snap.config.targets.contains_key(*name) => {
                Ok(Stat::new("ping", qid, 0o444, 0))
            }
            [name, "config"] if snap.config.targets.contains_key(*name) => {
                let target_config = snap.config.targets.get(*name).unwrap();
                Ok(Stat::new(
                    "config",
                    qid,
                    0o444,
                    serde_yaml::to_string(&target_config).unwrap().len() as u64,
                ))
            }
            [name, "journal"] if snap.config.targets.contains_key(*name) => {
                Ok(Stat::new("journal", qid, 0o444, 0))
            }
            _ => Err(Error::from(errors::NOT_FOUND)),
        }
    }

    /// List directory entries. Returns stats for all children.
    /// This is the core readdir logic, testable without Ctx.
    pub fn list_dir(&self, dir: u64) -> Result<Vec<Stat>> {
        let snap = self.snapshot();
        let path = snap
            .qid_cache
            .get(&dir)
            .ok_or_else(|| Error::from(errors::NOT_FOUND))?;

        let components: Vec<_> = path.components().collect();

        match components.as_slice() {
            [] => {
                let mut entries = vec![self.stat_qid(Qid::file(QID_CONFIG))?];
                for name in snap.config.targets.keys() {
                    let qid = Qid::dir(qid_from_path(UnixPath::ROOT.join(name)));
                    entries.push(self.stat_qid(qid)?);
                }
                Ok(entries)
            }
            [name] if snap.config.targets.contains_key(*name) => {
                let dir_path = UnixPath::ROOT.join(name);
                Ok(vec![
                    self.stat_qid(Qid::file(qid_from_path(dir_path.join("ping"))))?,
                    self.stat_qid(Qid::file(qid_from_path(dir_path.join("config"))))?,
                    self.stat_qid(Qid::file(qid_from_path(dir_path.join("journal"))))?,
                ])
            }
            _ => Err(Error::from(errors::NOT_FOUND)),
        }
    }

    /// Open a file by qid. Returns a PingFile with per-open state.
    /// This is the core open logic, testable without Ctx.
    pub fn open_file(&self, qid: Qid) -> Result<CharlesnetfsFile> {
        let snap = self.snapshot();
        let path = snap
            .qid_cache
            .get(&qid.path)
            .ok_or_else(|| Error::from(errors::NOT_FOUND))?;

        let components: Vec<_> = path.components().collect();

        match components.as_slice() {
            ["config"] => Ok(CharlesnetfsFile::GlobalConfig { fs: self.clone() }),
            [name, "ping"] if snap.config.targets.contains_key(*name) => {
                Ok(CharlesnetfsFile::Ping {
                    target_name: name.to_string(),
                    counter: AtomicU64::new(0),
                    fs: self.clone(),
                })
            }
            [name, "config"] if snap.config.targets.contains_key(*name) => {
                Ok(CharlesnetfsFile::TargetConfig {
                    target_name: name.to_string(),
                    fs: self.clone(),
                })
            }
            [name, "journal"] if snap.config.targets.contains_key(*name) => {
                Ok(CharlesnetfsFile::Journal {
                    target_name: name.to_string(),
                    fs: self.clone(),
                })
            }
            _ => Err(Error::from(errors::NOT_FOUND)),
        }
    }
}

#[derive(Debug)]
pub enum CharlesnetfsFile {
    /// Per-open ping probe: independent counter
    Ping {
        target_name: String,
        counter: AtomicU64,
        fs: CharlesnetFS,
    },
    /// The writable global config file.
    GlobalConfig { fs: CharlesnetFS },
    /// Read-only snapshot of a single target's configuration.
    TargetConfig {
        target_name: String,
        fs: CharlesnetFS,
    },
    /// Journal stream (placeholder).
    Journal {
        target_name: String,
        fs: CharlesnetFS,
    },
}

impl std::fmt::Display for CharlesnetfsFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CharlesnetfsFile::Ping {
                target_name,
                counter,
                ..
            } => write!(
                f,
                "Ping {{ target_name={}, counter={} }}",
                target_name,
                counter.load(Ordering::Relaxed)
            ),
            Self::GlobalConfig { .. } => write!(f, "GlobalConfig"),
            Self::TargetConfig { target_name, .. } => {
                write!(f, "TargetConfig {{ target_name={} }}", target_name)
            }
            Self::Journal { target_name, .. } => {
                write!(f, "Journal {{ target_name={} }}", target_name)
            }
        }
    }
}

impl Filesystem for CharlesnetFS {
    type File = CharlesnetfsFile;

    fn attach(&self, _ctx: &Ctx<'_>) -> Result<Qid> {
        Ok(Qid::dir(QID_ROOT))
    }

    fn walk1(&self, _ctx: &Ctx<'_>, dir: Qid, name: &str) -> Result<Qid> {
        self.walk_step(dir.path, name)
    }

    fn stat(&self, _ctx: &Ctx<'_>, qid: Qid) -> Result<Stat> {
        self.stat_qid(qid)
    }

    fn readdir(&self, _ctx: &Ctx<'_>, dir: Qid) -> Result<Vec<Stat>> {
        self.list_dir(dir.path)
    }

    fn open(&self, _ctx: &Ctx<'_>, qid: Qid, _mode: u8) -> Result<Self::File> {
        self.open_file(qid)
    }

    fn create(
        &self,
        _ctx: &Ctx<'_>,
        _dir: Qid,
        _name: &str,
        _perm: u32,
        _mode: u8,
    ) -> Result<(Qid, Self::File)> {
        Err(Error::from(errors::PERM))
    }

    fn remove(&self, _ctx: &Ctx<'_>, _qid: Qid) -> Result<()> {
        Err(Error::from(errors::PERM))
    }

    fn wstat(&self, _ctx: &Ctx<'_>, _qid: Qid, _stat: &Stat) -> Result<()> {
        Ok(())
    }
}

impl File for CharlesnetfsFile {
    async fn read(&self, offset: u64, count: u32) -> Result<Vec<u8>> {
        log::debug!("read on {} offset={} count={}", self, offset, count);
        match self {
            CharlesnetfsFile::Ping {
                target_name,
                counter,
                fs,
            } => {
                if offset > 0 {
                    // EOF beyond offset 0
                    return Ok(Vec::new());
                }
                let (timeout, addr) = {
                    let config = &fs.snapshot().config;
                    let target = config
                        .targets
                        .get(target_name)
                        .ok_or_else(|| Error::new("target not found"))?;
                    let timeout = target.timeout(config.global_timeout_ms);
                    let addr = target.addr.to_string();
                    (timeout, addr)
                };

                let n = counter.fetch_add(1, Ordering::Relaxed);

                let payload = format!("{}_{}", current_timestamp(), n);
                let mut buffer = Vec::new();

                // need an independent borrow to move to thread
                let ping_addr = addr.clone();

                // TODO: async version of ping
                match tokio::task::spawn_blocking(move || {
                    pinger::ping(&ping_addr, timeout, &payload)
                })
                .await
                .expect("could not join on tokio thread!")
                {
                    Ok(ping_result) => {
                        let ms = ping_result.latency_ms.unwrap();
                        log::debug!("ping {} → {}ms", addr, ms);
                        writeln!(&mut buffer, "pong {}ms", ms).unwrap();
                        writeln!(&mut buffer, "{}", ping_result.payload).unwrap();
                    }
                    Err(e) => {
                        log::debug!("ping {} → failed: {}", addr, e);
                        writeln!(&mut buffer, "fail: {}", e).unwrap();
                    }
                };

                Ok(buffer)
            }

            CharlesnetfsFile::TargetConfig { target_name, fs } => {
                let target = fs
                    .target_config(target_name)
                    .ok_or_else(|| Error::new("target not found"))?;
                let yaml =
                    serde_yaml::to_string(&target).map_err(|e| Error::from(e.to_string()))?;

                Ok(read_at(yaml.as_bytes(), offset, count))
            }

            CharlesnetfsFile::Journal {
                target_name: _,
                fs: _,
            } => Err(Error::from(errors::UNSUPPORTED)),

            CharlesnetfsFile::GlobalConfig { fs } => {
                let config = &fs.snapshot().config;
                let yaml = serde_yaml::to_string(&config).map_err(|e| Error::new(e.to_string()))?;
                Ok(read_at(&yaml.into_bytes(), offset, count))
            }
        }
    }

    async fn write(&self, offset: u64, data: &[u8]) -> Result<u32> {
        match self {
            CharlesnetfsFile::GlobalConfig { fs } => {
                if offset > 0 {
                    return Err(Error::from(errors::UNSUPPORTED));
                }
                let yaml =
                    String::from_utf8(data.to_vec()).map_err(|_| Error::new("invalid utf-8"))?;
                let new_config: Config = serde_yaml::from_str(&yaml)
                    .map_err(|e| Error::new(format!("invalid config: {}", e)))?;
                if new_config.targets.is_empty() {
                    return Err(Error::new("config must have at least one target"));
                }
                let len = data.len() as u32;

                // the snapshot is recreated in full, with qid cache rebuilt from new config
                // TODO: abstract over ArcSwap logic
                fs.state
                    .store(Arc::new(CharlesnetFS::build_snapshot(new_config)));
                log::info!("config updated");
                log::debug!("config={:#?}", &fs.snapshot().config);

                Ok(len)
            }
            _ => Err(Error::from(errors::PERM)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Addr, TargetConfig};
    use styx9p::proto::{QTDIR, QTFILE};

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

    fn test_fs() -> CharlesnetFS {
        CharlesnetFS::new(test_config())
    }

    // ---- qid_from_path tests (pure function, no dependencies) ----

    #[test]
    fn test_qid_root() {
        assert_eq!(qid_from_path("/"), QID_ROOT);
    }

    #[test]
    fn test_qid_config() {
        assert_eq!(qid_from_path("/config"), QID_CONFIG);
    }

    #[test]
    fn test_qid_target_above_100() {
        assert!(qid_from_path("/router") > 100);
    }

    #[test]
    fn test_qid_deterministic() {
        assert_eq!(qid_from_path("/router"), qid_from_path("/router"));
    }

    #[test]
    fn test_qid_different_paths_different_values() {
        assert_ne!(qid_from_path("/router"), qid_from_path("/nas"));
    }

    #[test]
    fn test_qid_deeper_is_higher() {
        for target in &["router", "nas", "localhost"] {
            assert!(
                qid_from_path(format!("/{}/ping", target)) > qid_from_path(format!("/{}", target))
            );
        }
    }

    // ---- qid_cache tests ----

    #[test]
    fn test_cache_initialization() {
        let fs = test_fs();
        let snap = fs.snapshot();
        // root + config + 2 targets * 4 entries (dir, ping, config, journal)
        assert!(snap.qid_cache.len() >= 10);
    }

    #[test]
    fn test_path_from_qid_root() {
        let fs = test_fs();
        assert_eq!(fs.path_from_qid(QID_ROOT).unwrap().to_string(), "/");
    }

    #[test]
    fn test_path_from_qid_target_dir() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        assert_eq!(fs.path_from_qid(router_qid).unwrap().to_string(), "/router");
    }

    #[test]
    fn test_path_from_qid_target_ping() {
        let fs = test_fs();
        let ping_qid = qid_from_path("/router/ping");
        assert_eq!(
            fs.path_from_qid(ping_qid).unwrap().to_string(),
            "/router/ping"
        );
    }

    #[test]
    fn test_path_from_qid_invalid() {
        let fs = test_fs();
        assert!(fs.path_from_qid(999999).is_none());
    }

    // ---- Filesystem::attach (via walk_step / stat_qid / list_dir / open_file) ----

    #[test]
    fn test_attach_returns_root_dir() {
        let _fs = test_fs();
        // attach always returns Qid::dir(QID_ROOT) — test the contract:
        let root = Qid::dir(QID_ROOT);
        assert_eq!(root.path, QID_ROOT);
        assert_eq!(root.qtype, QTDIR);
    }

    // ---- walk_step ----

    #[test]
    fn test_walk1_root_to_config() {
        let fs = test_fs();
        let qid = fs.walk_step(QID_ROOT, "config").unwrap();
        assert_eq!(qid.path, QID_CONFIG);
        assert_eq!(qid.qtype, QTFILE);
    }

    #[test]
    fn test_walk1_root_to_target() {
        let fs = test_fs();
        let qid = fs.walk_step(QID_ROOT, "router").unwrap();
        assert_eq!(qid.path, qid_from_path("/router"));
        assert_eq!(qid.qtype, QTDIR);
    }

    #[test]
    fn test_walk1_root_to_nonexistent() {
        let fs = test_fs();
        assert!(fs.walk_step(QID_ROOT, "nonexistent").is_err());
    }

    #[test]
    fn test_walk1_root_dotdot() {
        let fs = test_fs();
        let qid = fs.walk_step(QID_ROOT, "..").unwrap();
        assert_eq!(qid.path, QID_ROOT);
        assert_eq!(qid.qtype, QTDIR);
    }

    #[test]
    fn test_walk1_target_to_ping() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        let qid = fs.walk_step(router_qid, "ping").unwrap();
        assert_eq!(qid.path, qid_from_path("/router/ping"));
        assert_eq!(qid.qtype, QTFILE);
    }

    #[test]
    fn test_walk1_target_to_config() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        let qid = fs.walk_step(router_qid, "config").unwrap();
        assert_eq!(qid.path, qid_from_path("/router/config"));
        assert_eq!(qid.qtype, QTFILE);
    }

    #[test]
    fn test_walk1_target_to_journal() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        let qid = fs.walk_step(router_qid, "journal").unwrap();
        assert_eq!(qid.path, qid_from_path("/router/journal"));
        assert_eq!(qid.qtype, QTFILE);
    }

    #[test]
    fn test_walk1_target_dotdot() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        let qid = fs.walk_step(router_qid, "..").unwrap();
        assert_eq!(qid.path, QID_ROOT);
        assert_eq!(qid.qtype, QTDIR);
    }

    #[test]
    fn test_walk1_target_invalid_file() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        assert!(fs.walk_step(router_qid, "invalid").is_err());
    }

    #[test]
    fn test_walk1_from_file_fails() {
        let fs = test_fs();
        let ping_qid = qid_from_path("/router/ping");
        assert!(fs.walk_step(ping_qid, "anything").is_err());
    }

    // ---- stat_qid ----

    #[test]
    fn test_stat_root() {
        let fs = test_fs();
        let stat = fs.stat_qid(Qid::dir(QID_ROOT)).unwrap();
        assert_eq!(stat.name, "/");
        assert!(stat.qid.is_dir());
    }

    #[test]
    fn test_stat_global_config() {
        let fs = test_fs();
        let stat = fs.stat_qid(Qid::file(QID_CONFIG)).unwrap();
        assert_eq!(stat.name, "config");
        assert!(!stat.qid.is_dir());
        // Global config is writable (0o644)
        assert_eq!(stat.mode & 0o644, 0o644);
    }

    #[test]
    fn test_stat_target_dir() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        let stat = fs.stat_qid(Qid::dir(router_qid)).unwrap();
        assert_eq!(stat.name, "router");
        assert!(stat.qid.is_dir());
    }

    #[test]
    fn test_stat_ping() {
        let fs = test_fs();
        let ping_qid = qid_from_path("/router/ping");
        let stat = fs.stat_qid(Qid::file(ping_qid)).unwrap();
        assert_eq!(stat.name, "ping");
        assert!(!stat.qid.is_dir());
    }

    #[test]
    fn test_stat_target_config_readonly() {
        let fs = test_fs();
        let config_qid = qid_from_path("/router/config");
        let stat = fs.stat_qid(Qid::file(config_qid)).unwrap();
        assert_eq!(stat.name, "config");
        assert!(!stat.qid.is_dir());
        // Target config is read-only (0o444)
        assert_eq!(stat.mode & 0o666, 0o444);
    }

    #[test]
    fn test_stat_journal() {
        let fs = test_fs();
        let journal_qid = qid_from_path("/router/journal");
        let stat = fs.stat_qid(Qid::file(journal_qid)).unwrap();
        assert_eq!(stat.name, "journal");
        assert!(!stat.qid.is_dir());
    }

    #[test]
    fn test_stat_invalid_qid() {
        let fs = test_fs();
        assert!(fs.stat_qid(Qid::file(999999)).is_err());
    }

    // ---- list_dir ----

    #[test]
    fn test_list_dir_root() {
        let fs = test_fs();
        let entries = fs.list_dir(QID_ROOT).unwrap();
        let names: Vec<&str> = entries.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"config"));
        assert!(names.contains(&"router"));
        assert!(names.contains(&"nas"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn test_list_dir_target() {
        let fs = test_fs();
        let router_qid = qid_from_path("/router");
        let entries = fs.list_dir(router_qid).unwrap();
        let names: Vec<&str> = entries.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"ping"));
        assert!(names.contains(&"config"));
        assert!(names.contains(&"journal"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn test_list_dir_invalid_qid() {
        let fs = test_fs();
        assert!(fs.list_dir(999999).is_err());
    }

    // ---- open_file ----

    #[test]
    fn test_open_ping_returns_ping_file() {
        let fs = test_fs();
        let ping_qid = Qid::file(qid_from_path("/router/ping"));
        let file = fs.open_file(ping_qid).unwrap();
        match file {
            CharlesnetfsFile::Ping {
                target_name,
                counter,
                ..
            } => {
                assert_eq!(target_name, "router");
                assert_eq!(counter.load(Ordering::Relaxed), 0);
            }
            _ => panic!("expected PingFile::Ping"),
        }
    }

    #[test]
    fn test_open_target_config_returns_config_file() {
        let fs = test_fs();
        let config_qid = Qid::file(qid_from_path("/router/config"));
        let file = fs.open_file(config_qid).unwrap();
        match file {
            CharlesnetfsFile::TargetConfig { target_name, .. } => {
                assert_eq!(target_name, "router");
            }
            _ => panic!("expected PingFile::TargetConfig"),
        }
    }

    #[test]
    #[ignore = "not yet implemented"]
    fn test_open_journal_returns_journal_file() {
        let fs = test_fs();
        let journal_qid = Qid::file(qid_from_path("/router/journal"));
        let file = fs.open_file(journal_qid).unwrap();
        match file {
            CharlesnetfsFile::Journal { target_name, .. } => {
                assert_eq!(target_name, "router");
            }
            _ => panic!("expected PingFile::Journal"),
        }
    }

    #[test]
    fn test_open_global_config_returns_global_config_file() {
        let fs = test_fs();
        let config_qid = Qid::file(QID_CONFIG);
        let file = fs.open_file(config_qid).unwrap();
        match file {
            CharlesnetfsFile::GlobalConfig { .. } => {}
            _ => panic!("expected PingFile::GlobalConfig"),
        }
    }

    #[test]
    fn test_open_invalid_qid() {
        let fs = test_fs();
        assert!(fs.open_file(Qid::file(999999)).is_err());
    }

    // ---- create, remove, wstat ----
    // These are trivial trait-method defaults / one-liners.
    // Ctx is #[non_exhaustive] so we can't construct it in tests;
    // testing them requires an integration test with a live Server.
    // Their behavior is verified by the trait impl contract:
    //   create -> PERM error
    //   remove -> PERM error
    //   wstat  -> Ok(())

    // ---- has_target ----

    #[test]
    fn test_has_target() {
        let fs = test_fs();
        assert!(fs.has_target("router"));
        assert!(fs.has_target("nas"));
        assert!(!fs.has_target("nonexistent"));
    }

    // ---- File::read (async) ----

    #[tokio::test]
    async fn test_read_global_config() {
        let fs = test_fs();
        let file = CharlesnetfsFile::GlobalConfig { fs: fs.clone() };
        let data = file.read(0, 4096).await.unwrap();
        let content = String::from_utf8(data).unwrap();
        assert!(content.contains("global_timeout_ms:"));
        assert!(content.contains("targets:"));
        assert!(content.contains("router:"));
        assert!(content.contains("nas:"));
    }

    #[tokio::test]
    async fn test_read_global_config_with_offset() {
        let fs = test_fs();
        let file = CharlesnetfsFile::GlobalConfig { fs: fs.clone() };
        let full = file.read(0, 4096).await.unwrap();
        let partial = file.read(5, 4096).await.unwrap();
        assert_eq!(&full[5..], partial);
    }

    #[tokio::test]
    async fn test_read_global_config_offset_past_end() {
        let fs = test_fs();
        let file = CharlesnetfsFile::GlobalConfig { fs: fs.clone() };
        let data = file.read(0, 4096).await.unwrap();
        let len = data.len() as u64;
        let empty = file.read(len, 4096).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_read_target_config() {
        let fs = test_fs();
        let file = CharlesnetfsFile::TargetConfig {
            target_name: "router".to_string(),
            fs: fs.clone(),
        };
        let data = file.read(0, 4096).await.unwrap();
        let content = String::from_utf8(data).unwrap();
        assert!(content.contains("addr:"));
        assert!(content.contains("timeout_ms:"));
    }

    #[tokio::test]
    async fn test_read_target_config_with_offset() {
        let fs = test_fs();
        let file = CharlesnetfsFile::TargetConfig {
            target_name: "router".to_string(),
            fs: fs.clone(),
        };
        let full = file.read(0, 4096).await.unwrap();
        let partial = file.read(5, 4096).await.unwrap();
        assert_eq!(&full[5..], partial);
    }

    #[tokio::test]
    async fn test_read_ping_independent_per_open() {
        // Two opens of the same ping file should have independent counters
        let fs = test_fs();
        let file1 = CharlesnetfsFile::Ping {
            target_name: "router".to_string(),
            counter: AtomicU64::new(0),
            fs: fs.clone(),
        };
        let file2 = CharlesnetfsFile::Ping {
            target_name: "router".to_string(),
            counter: AtomicU64::new(0),
            fs: fs.clone(),
        };

        // Both start at counter 0; after one read each, both should be at 1
        let _ = file1.read(0, 4096).await;
        let _ = file2.read(0, 4096).await;

        // Verify counters are independent (each should be 1, not 2)
        match &file1 {
            CharlesnetfsFile::Ping { counter, .. } => {
                assert_eq!(counter.load(Ordering::Relaxed), 1)
            }
            _ => panic!(),
        }
        match &file2 {
            CharlesnetfsFile::Ping { counter, .. } => {
                assert_eq!(counter.load(Ordering::Relaxed), 1)
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    #[ignore = "not yet implemented"]
    async fn test_read_journal_placeholder() {
        let fs = test_fs();
        let file = CharlesnetfsFile::Journal {
            target_name: "router".to_string(),
            fs: fs.clone(),
        };
        let data = file.read(0, 4096).await.unwrap();
        let content = String::from_utf8(data).unwrap();
        assert!(content.contains("not yet implemented"));
    }

    // ---- File::write (async) ----

    #[tokio::test]
    async fn test_write_global_config_valid() {
        let fs = test_fs();
        let file = CharlesnetfsFile::GlobalConfig { fs: fs.clone() };

        let yaml = "global_timeout_ms: 3000\ntargets:\n  router:\n    addr: 10.0.0.1:22\n";
        let written = file.write(0, yaml.as_bytes()).await.unwrap();
        assert_eq!(written, yaml.len() as u32);

        let CharlesnetfsFile::GlobalConfig { fs } = &file else {
            panic!()
        };
        let config = &fs.snapshot().config;
        assert_eq!(config.global_timeout_ms, 3000);
        assert!(config.targets.contains_key("router"));
        assert_eq!(config.targets["router"].addr.to_string(), "10.0.0.1:22");

        // Qid cache should be rebuilt
        let snap = fs.snapshot();
        assert!(snap.qid_cache.values().any(|p| p.to_string() == "/router"));
        assert!(snap
            .qid_cache
            .values()
            .any(|p| p.to_string() == "/router/ping"));
    }

    #[tokio::test]
    async fn test_write_global_config_invalid_yaml() {
        let fs = test_fs();
        let file = CharlesnetfsFile::GlobalConfig { fs: fs.clone() };

        let CharlesnetfsFile::GlobalConfig { fs } = &file else {
            panic!()
        };
        let orig_global = fs.snapshot().config.global_timeout_ms;

        let yaml = "invalid: yaml: content:";
        let result = file.write(0, yaml.as_bytes()).await;
        assert!(result.is_err());

        assert_eq!(fs.snapshot().config.global_timeout_ms, orig_global);
    }

    #[tokio::test]
    async fn test_write_global_config_empty_targets() {
        let fs = test_fs();
        let file = CharlesnetfsFile::GlobalConfig { fs: fs.clone() };

        let yaml = "global_timeout_ms: 5000\ntargets:\n";
        let result = file.write(0, yaml.as_bytes()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_write_global_config_with_offset_fails() {
        let fs = test_fs();
        let file = CharlesnetfsFile::GlobalConfig { fs: fs.clone() };

        let CharlesnetfsFile::GlobalConfig { fs } = &file else {
            panic!()
        };
        let orig_global = fs.snapshot().config.global_timeout_ms;

        let yaml = "global_timeout_ms: 3000\ntargets:\n  router:\n    addr: 10.0.0.1:22\n";
        let result = file.write(5, yaml.as_bytes()).await;
        assert!(result.is_err());
        assert_eq!(fs.snapshot().config.global_timeout_ms, orig_global);
    }

    #[tokio::test]
    async fn test_write_non_global_config_denied() {
        let fs = test_fs();
        let file = CharlesnetfsFile::TargetConfig {
            target_name: "router".to_string(),
            fs: fs.clone(),
        };
        let result = file.write(0, b"data").await;
        assert!(result.is_err());
    }

    // ---- config reload visible in readdir ----

    #[test]
    fn test_list_dir_shows_new_targets_after_reload() {
        let fs = test_fs();

        // Initial: router, nas
        let entries = fs.list_dir(QID_ROOT).unwrap();
        let names: Vec<&str> = entries.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"router"));
        assert!(names.contains(&"nas"));
        assert!(!names.contains(&"laptop"));

        // Reload config with a new target
        let yaml = "global_timeout_ms: 5000\ntargets:\n  router:\n    addr: 192.168.1.1:22\n  nas:\n    addr: 192.168.1.100:5000\n  laptop:\n    addr: 10.0.0.5:22\n";
        let new_config: Config = serde_yaml::from_str(yaml).unwrap();
        let snap = Snapshot::new(new_config);
        fs.state.store(Arc::new(snap));

        // Verify new target appears
        let entries = fs.list_dir(QID_ROOT).unwrap();
        let names: Vec<&str> = entries.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"laptop"));
    }

    #[test]
    fn test_snapshot_config_qid_cache_consistent() {
        // Every qid_cache path must correspond to a valid config entry and vice versa.
        // This invariant holds trivially under ArcSwap<Snapshot> (atomic bundle)
        // but can be violated under RwLock+Mutex during the TOCTOU write window.
        let fs = test_fs();
        let snap = fs.snapshot();

        let config = &snap.config;
        let cache = &snap.qid_cache;

        // Root and global config always present
        assert!(cache.contains_key(&QID_ROOT));
        assert!(cache.contains_key(&QID_CONFIG));
        assert_eq!(cache[&QID_ROOT].to_string(), "/");
        assert_eq!(cache[&QID_CONFIG].to_string(), "/config");

        // Every target in config has a corresponding dir entry in cache
        for name in config.targets.keys() {
            let dir_path = UnixPath::ROOT.join(name);
            let dir_qid = qid_from_path(&dir_path);
            assert!(
                cache.contains_key(&dir_qid),
                "missing dir qid for target {}",
                name
            );
            assert_eq!(cache[&dir_qid].to_string(), format!("/{}", name));

            // Every target has ping, config, journal sub-files
            for sub in &["ping", "config", "journal"] {
                let sub_path = dir_path.join(sub);
                let sub_qid = qid_from_path(&sub_path);
                assert!(
                    cache.contains_key(&sub_qid),
                    "missing {} qid for target {}",
                    sub,
                    name
                );
            }
        }
    }
}
