# ADR 3: Shared filesystem state via Arc<PingFS>

Date: 2026-11-21

## Status
Accepted

## Context
The `File` trait takes `&self` on `read`/`write` and requires `Send + Sync` (reads run on separate tasks). This rules out `RefCell` and holding a lock guard across open→close. `PingFile` needs interior mutability through a shared reference.

Current state: `PingFS` owns `config: Arc<RwLock<Config>>` and `qid_cache: Arc<Mutex<BTreeMap<u64, UnixPath>>>`; `Clone` is two `Arc::clone` calls. `PingFile` variants (`Ping`, `GlobalConfig`, `TargetConfig`, `Journal`) each hold `fs: PingFS`. `File::read` acquires a read-lock per call; `GlobalConfig::write` acquires the write-lock, updates config, calls `fs.rebuild_qid_cache()`. All 147 tests pass.

## Decision
Put shared mutable state on `PingFS`, not on `PingFile`. Each `PingFile` holds a shallow `PingFS` clone (`Arc` bumps). Reads/writes go through the locks — interior mutability in action.

## Consequences
- Open fids share config; writes propagate immediately to all clones.
- Read path takes a brief read-lock per call; write path takes the write-lock.
- TOCTOU gap between config write and qid_cache rebuild remains (microsecond window). `list_dir` reads config directly, so listings stay consistent.
