//! 9P filesystem server using styx9p.
//!
//! Design differences from the ninep-based implementation (`server/ninep.rs`):
//!
//! 1. **Two-trait split:** `Filesystem` (sync tree ops) + `File` (async I/O)
//!    replaces the single `Serve9p` trait. Tree ops (walk, stat, readdir)
//!    are sync by nature; only reads/writes are async.
//!
//! 2. **Per-open state:** `PingFile` carries its own `ping_counter`, so each
//!    `open()` of `/router/ping` gets an independent counter. The old code
//!    shared one counter across all opens via `ping_results: BTreeMap<Qid, String>`.
//!
//! 3. **Async journal reads:** `File::read` awaits the journal stream directly.
//!    The old code returned `ReadOutcome::Blocked(channel)` from a sync method,
//!    spawning threads and plumbing channels. No more.
//!
//! 4. **`read_at()` replaces `slice_bytes()`:** Same logic, provided by styx9p.
//!
//! 5. **Unix-mode permissions:** `Stat::new(name, qid, 0o555, 0)` replaces
//!    verbose `Perm::DIR | Perm::OWNER_READ | ...` bitflag composition.
//!
//! 6. **Target info carried per-file:** `PingFile` stores the target name and
//!    a reference to `Arc<PingFS>`, so `File::read`/`File::write` can access
//!    config and ping logic without global state lookups.

#![warn(clippy::await_holding_lock)]

use crate::config::Config;
use crate::pinger;
use crate::UnixPath;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::RwLock;
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

/// The filesystem tree: configuration and qid mapping.
/// Shared across all connections (styx9p wraps it in Arc internally).
pub struct PingFS {
    /// Read-write lock on the live config. Only written via `File::write` on
    /// `GlobalConfig`. Safe to hold across `.await` only in read mode; write
    /// mode is never held across `.await` (the write completes synchronously).
    pub config: RwLock<Config>,
    /// Qid path -> UnixPath (the reverse mapping for resolution).
    /// Only accessed from sync `Filesystem` trait methods — never across `.await`.
    qid_cache: Mutex<BTreeMap<u64, UnixPath>>,
}

impl Clone for PingFS {
    fn clone(&self) -> Self {
        Self {
            config: RwLock::new(self.config.read().unwrap().clone()),
            qid_cache: Mutex::new(self.qid_cache.lock().unwrap().clone()),
        }
    }
}

impl PingFS {
    pub fn new(config: Config) -> Self {
        let fs = Self {
            config: RwLock::new(config),
            qid_cache: Mutex::new(BTreeMap::new()),
        };
        fs.rebuild_qid_cache();
        fs
    }

    pub fn rebuild_qid_cache(&self) {
        let mut cache = self.qid_cache.lock().unwrap();
        let config = self.config.read().unwrap();

        cache.clear();
        cache.insert(QID_ROOT, UnixPath::ROOT);
        cache.insert(QID_CONFIG, UnixPath::ROOT.join("config"));

        for name in config.targets.keys() {
            let dir_path = UnixPath::ROOT.join(name);
            cache.insert(qid_from_path(&dir_path), dir_path.clone());
            cache.insert(qid_from_path(dir_path.join("ping")), dir_path.join("ping"));
            cache.insert(
                qid_from_path(dir_path.join("config")),
                dir_path.join("config"),
            );
            cache.insert(
                qid_from_path(dir_path.join("journal")),
                dir_path.join("journal"),
            );
        }
    }

    pub fn path_from_qid(&self, qid_path: u64) -> Option<UnixPath> {
        self.qid_cache.lock().unwrap().get(&qid_path).cloned()
    }

    pub fn has_target(&self, name: &str) -> bool {
        self.config.read().unwrap().targets.contains_key(name)
    }

    /// Walk one step from `dir` to `name`. Returns the qid of the child.
    /// This is the core walk logic, testable without Ctx.
    pub fn walk_step(&self, dir: u64, name: &str) -> Result<Qid> {
        let dir_path = self
            .path_from_qid(dir)
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
                if self.has_target(name) {
                    return Ok(Qid::dir(qid_from_path(UnixPath::ROOT.join(name))));
                }
                Err(Error::from(errors::NOT_FOUND))
            }
            [target] if self.has_target(target) => {
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
        let path = self
            .path_from_qid(qid.path)
            .ok_or_else(|| Error::from(errors::NOT_FOUND))?;

        let components: Vec<_> = path.components().collect();

        match components.as_slice() {
            [] => Ok(Stat::new("/", qid, 0o555, 0)),
            ["config"] => {
                let config = self.config.read().unwrap().clone();
                Ok(Stat::new(
                    "config",
                    qid,
                    0o666,
                    // TODO: serializing to get size might become costly
                    serde_yaml::to_string(&config).unwrap().len() as u64,
                ))
            }
            [name] if self.has_target(name) => Ok(Stat::new(*name, qid, 0o555, 0)),
            [name, "ping"] if self.has_target(name) => Ok(Stat::new("ping", qid, 0o666, 0)),
            [name, "config"] if self.has_target(name) => {
                let config = self.config.read().unwrap();
                let target_config = config.targets.get(*name).unwrap();
                Ok(Stat::new(
                    "config",
                    qid,
                    0o444,
                    // TODO: serializing to get size might become costly
                    serde_yaml::to_string(&target_config).unwrap().len() as u64,
                ))
            }
            [name, "journal"] if self.has_target(name) => Ok(Stat::new("journal", qid, 0o444, 0)),
            _ => Err(Error::from(errors::NOT_FOUND)),
        }
    }

    /// List directory entries. Returns stats for all children.
    /// This is the core readdir logic, testable without Ctx.
    pub fn list_dir(&self, dir: u64) -> Result<Vec<Stat>> {
        let path = self
            .path_from_qid(dir)
            .ok_or_else(|| Error::from(errors::NOT_FOUND))?;

        let components: Vec<_> = path.components().collect();

        match components.as_slice() {
            [] => {
                let mut entries = vec![self.stat_qid(Qid::file(QID_CONFIG))?];
                for name in self.config.read().unwrap().targets.keys() {
                    let qid = Qid::dir(qid_from_path(UnixPath::ROOT.join(name)));
                    entries.push(self.stat_qid(qid)?);
                }
                Ok(entries)
            }
            [name] if self.has_target(name) => {
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
    pub fn open_file(&self, qid: Qid) -> Result<PingFile> {
        let path = self
            .path_from_qid(qid.path)
            .ok_or_else(|| Error::from(errors::NOT_FOUND))?;

        let components: Vec<_> = path.components().collect();

        match components.as_slice() {
            ["config"] => Ok(PingFile::GlobalConfig { fs: self.clone() }),
            [name, "ping"] if self.has_target(name) => Ok(PingFile::Ping {
                target_name: name.to_string(),
                counter: AtomicU64::new(0),
                fs: self.clone(),
            }),
            [name, "config"] if self.has_target(name) => Ok(PingFile::TargetConfig {
                target_name: name.to_string(),
                fs: self.clone(),
            }),
            [name, "journal"] if self.has_target(name) => Ok(PingFile::Journal {
                target_name: name.to_string(),
                fs: self.clone(),
            }),
            _ => Err(Error::from(errors::NOT_FOUND)),
        }
    }
}

/// One open fid: per-open state lives here.
/// Each `open()` creates an independent instance.
pub enum PingFile {
    /// Per-open ping probe: independent counter and last-result cache.
    /// `counter` and `last_result` are only touched in the sync portion of
    /// `File::read` (before the blocking `pinger::ping` call); never held
    /// across an `.await`.
    Ping {
        target_name: String,
        counter: AtomicU64,
        fs: PingFS,
    },
    /// The writable global config file.
    GlobalConfig { fs: PingFS },
    /// Read-only snapshot of a single target's configuration.
    TargetConfig { target_name: String, fs: PingFS },
    /// Journal stream (placeholder).
    Journal { target_name: String, fs: PingFS },
}

impl Filesystem for PingFS {
    type File = PingFile;

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

impl File for PingFile {
    async fn read(&self, offset: u64, count: u32) -> Result<Vec<u8>> {
        match self {
            PingFile::Ping {
                target_name,
                counter,
                fs,
            } => {
                let config = fs.config.read().unwrap();
                let target = config
                    .targets
                    .get(target_name.as_str())
                    .ok_or_else(|| Error::new("target not found"))?;

                let addr = target.addr.to_string();
                let timeout = target.timeout(config.global_timeout_ms);

                let n = counter.fetch_add(1, Ordering::Relaxed);

                let payload = format!("{}_{}", current_timestamp(), n);
                let mut buffer = Vec::new();

                match pinger::ping(&addr, timeout, &payload) {
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

            PingFile::TargetConfig { target_name, fs } => {
                let config = fs.config.read().unwrap();
                let target = config
                    .targets
                    .get(target_name.as_str())
                    .ok_or_else(|| Error::new("target not found"))?;
                let yaml = serde_yaml::to_string(target).map_err(|e| Error::from(e.to_string()))?;

                Ok(read_at(yaml.as_bytes(), offset, count))
            }

            PingFile::Journal { target_name, fs } => {
                let config = fs.config.read().unwrap();
                let target = config
                    .targets
                    .get(target_name.as_str())
                    .ok_or_else(|| Error::new("target not found"))?;

                let _port = target.journal_port();
                let _host = target.addr.host().to_owned();

                // TODO: async journal follow
                // let msg = format!("journal not yet implemented for {}\n", target_name);
                // Ok(read_at(msg.as_bytes(), offset, count))
                Err(Error::from(errors::UNSUPPORTED))
            }

            PingFile::GlobalConfig { fs } => {
                let config = fs.config.read().unwrap();
                let yaml =
                    serde_yaml::to_string(&*config).map_err(|e| Error::new(e.to_string()))?;
                Ok(read_at(&yaml.into_bytes(), offset, count))
            }
        }
    }

    async fn write(&self, offset: u64, data: &[u8]) -> Result<u32> {
        match self {
            PingFile::GlobalConfig { fs } => {
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
                *fs.config.write().unwrap() = new_config;
                fs.rebuild_qid_cache();
                log::info!("config updated");
                log::debug!("config={:#?}", fs.config.read().unwrap());
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

    fn test_fs() -> PingFS {
        PingFS::new(test_config())
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
        let cache = fs.qid_cache.lock().unwrap();
        // root + config + 2 targets * 4 entries (dir, ping, config, journal)
        assert!(cache.len() >= 10);
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
        // Global config is writable (0o666)
        assert_eq!(stat.mode & 0o666, 0o666);
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
            PingFile::Ping {
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
            PingFile::TargetConfig { target_name, .. } => {
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
            PingFile::Journal { target_name, .. } => {
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
            PingFile::GlobalConfig { .. } => {}
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
        let file = PingFile::GlobalConfig { fs: fs };
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
        let file = PingFile::GlobalConfig { fs: fs };
        let full = file.read(0, 4096).await.unwrap();
        let partial = file.read(5, 4096).await.unwrap();
        assert_eq!(&full[5..], partial);
    }

    #[tokio::test]
    async fn test_read_global_config_offset_past_end() {
        let fs = test_fs();
        let file = PingFile::GlobalConfig { fs: fs };
        let data = file.read(0, 4096).await.unwrap();
        let len = data.len() as u64;
        let empty = file.read(len, 4096).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_read_target_config() {
        let fs = test_fs();
        let file = PingFile::TargetConfig {
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
        let file = PingFile::TargetConfig {
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
        let file1 = PingFile::Ping {
            target_name: "router".to_string(),
            counter: AtomicU64::new(0),
            fs: fs.clone(),
        };
        let file2 = PingFile::Ping {
            target_name: "router".to_string(),
            counter: AtomicU64::new(0),
            fs: fs.clone(),
        };

        // Both start at counter 0; after one read each, both should be at 1
        let _ = file1.read(0, 4096).await;
        let _ = file2.read(0, 4096).await;

        // Verify counters are independent (each should be 1, not 2)
        match &file1 {
            PingFile::Ping { counter, .. } => assert_eq!(counter.load(Ordering::Relaxed), 1),
            _ => panic!(),
        }
        match &file2 {
            PingFile::Ping { counter, .. } => assert_eq!(counter.load(Ordering::Relaxed), 1),
            _ => panic!(),
        }
    }

    #[tokio::test]
    #[ignore = "not yet implemented"]
    async fn test_read_journal_placeholder() {
        let fs = test_fs();
        let file = PingFile::Journal {
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
        let file = PingFile::GlobalConfig { fs: test_fs() };

        let yaml = "global_timeout_ms: 3000\ntargets:\n  router:\n    addr: 10.0.0.1:22\n";
        let written = file.write(0, yaml.as_bytes()).await.unwrap();
        assert_eq!(written, yaml.len() as u32);

        let PingFile::GlobalConfig { fs } = &file else {
            panic!()
        };
        let config = fs.config.read().unwrap();
        assert_eq!(config.global_timeout_ms, 3000);
        assert!(config.targets.contains_key("router"));
        assert_eq!(config.targets["router"].addr.to_string(), "10.0.0.1:22");

        // Qid cache should be rebuilt
        let cache = fs.qid_cache.lock().unwrap();
        assert!(cache.values().any(|p| p.to_string() == "/router"));
        assert!(cache.values().any(|p| p.to_string() == "/router/ping"));
    }

    #[tokio::test]
    async fn test_write_global_config_invalid_yaml() {
        let file = PingFile::GlobalConfig { fs: test_fs() };

        let PingFile::GlobalConfig { fs } = &file else {
            panic!()
        };
        let orig_global = fs.config.read().unwrap().global_timeout_ms;

        let yaml = "invalid: yaml: content:";
        let result = file.write(0, yaml.as_bytes()).await;
        assert!(result.is_err());

        assert_eq!(fs.config.read().unwrap().global_timeout_ms, orig_global);
    }

    #[tokio::test]
    async fn test_write_global_config_empty_targets() {
        let file = PingFile::GlobalConfig { fs: test_fs() };

        let yaml = "global_timeout_ms: 5000\ntargets:\n";
        let result = file.write(0, yaml.as_bytes()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_write_global_config_with_offset_fails() {
        let file = PingFile::GlobalConfig { fs: test_fs() };

        let PingFile::GlobalConfig { fs } = &file else {
            panic!()
        };
        let orig_global = fs.config.read().unwrap().global_timeout_ms;

        let yaml = "global_timeout_ms: 3000\ntargets:\n  router:\n    addr: 10.0.0.1:22\n";
        let result = file.write(5, yaml.as_bytes()).await;
        assert!(result.is_err());
        assert_eq!(fs.config.read().unwrap().global_timeout_ms, orig_global);
    }

    #[tokio::test]
    async fn test_write_non_global_config_denied() {
        let fs = test_fs();
        let file = PingFile::TargetConfig {
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
        *fs.config.write().unwrap() = new_config;
        fs.rebuild_qid_cache();

        // Verify new target appears
        let entries = fs.list_dir(QID_ROOT).unwrap();
        let names: Vec<&str> = entries.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"laptop"));
    }
}
