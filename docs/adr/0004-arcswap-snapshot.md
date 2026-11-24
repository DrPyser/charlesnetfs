# ADR 4: Replace RwLock+Mutex with ArcSwap<Snapshot>

Date: 2026-11-21

## Status
Proposed

## Context
ADR 3 put shared state behind `Arc<RwLock<Config>>` + `Arc<Mutex<qid_cache>>` on `PingFS`. This works, but readers take a lock per call and the config/qid_cache pair can desync for microseconds during writes (TOCTOU gap). The read path — pings, stat, readdir — is the hot one; lock contention there is wasteful.

## Decision
Bundle `Config` + `qid_cache` into one immutable `Snapshot`, stored in `arc_swap::ArcSwap<Snapshot>` on `PingFS`. Readers call `fs.snapshot()` → lock-free `Arc` clone. Writers build a new `Snapshot` and atomically swap it in. The `RwLock` and `Mutex` disappear.

```rust
struct Snapshot { config: Config, qid_cache: BTreeMap<u64, UnixPath> }
pub struct PingFS { state: ArcSwap<Snapshot>, last_ctl_result: Mutex<String> }
```

`PingFile` variants hold `fs: PingFS` unchanged. — `File::read` calls `fs.snapshot()` instead of `fs.config.read()`.

## Consequences
- Reads are lock-free (~1ns `Arc` clone vs ~50ns lock acquire). No reader ever blocks a writer.
- No TOCTOU — `Snapshot` bundles config + qid_cache, always consistent.
- Writers still serialize (only one swap at a time) but build the full snapshot off the critical path.
- New dependency: [`arc-swap`](https://crates.io/crates/arc-swap) crate (~2KB).
- Supersedes ADR 3's lock model; ADR 3 stays as the accepted fallback if `arc-swap` is rejected.

## Alternatives considered

**Per-target slots** — decompose the single global `Snapshot` into per-target
`ArcSwap<TargetConfig>` entries so writes to `/<target>/config` only clone
that one target, not the entire Config + qid_cache:

```rust
pub struct PingFS {
    global_config: ArcSwap<GlobalConfig>,
    targets: RwLock<BTreeMap<String, ArcSwap<TargetConfig>>>,
    qid_cache: Mutex<BTreeMap<u64, UnixPath>>,
}
```

This eliminates qid_cache rebuilds on single-target writes and avoids
contention between writes to unrelated targets.

**Deferred.** The single global `Snapshot` is simpler and sufficient for
current scale (<100 targets, writes on human timescale). Per-target slots
add complexity (nested `ArcSwap` in a `RwLock<BTreeMap>`, cross-slot
consistency considerations) for a contention scenario that doesn't exist yet.
The refactoring path is clear if it becomes necessary: extract per-target
state from the global snapshot.
