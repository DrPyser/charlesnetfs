# ADR 5: mDNS Discovery via External 9P Client

Date: 2026-11-21

## Status
Proposed

## Context
charlesnetfs requires manual config editing to add targets. On a home network
where hosts announce themselves via mDNS (Avahi/Bonjour), automatic discovery
would eliminate this friction. Hard targets from `config.yaml` must remain
supported and must not be overwritten by discovery.

## Decision
The mDNS updater is a **standalone process** separate from the charlesnetfs
server. It connects via the 9P Unix socket and writes `/ctl` commands — just
like any other 9P client. The server treats all `/ctl` writes identically
regardless of source.

```
mDNS Updater (standalone binary)
    │
    │ 9P write /ctl "add pi addr=..."
    ▼
charlesnetfs server
    │
    ├── /ctl handler (acquires config.write())
    └── all other 9P clients
```

Supporting decisions:

1. **No internal channels or actors.** The server already handles concurrent
   `/ctl` writes via the config `RwLock`. The mDNS updater adds no new
   coordination requirements.

2. **Grace timer lives in the updater process.** The server has no knowledge
   of mDNS TTLs or grace periods. The mDNS updater tracks these internally
   and only sends `remove` after the grace period expires.

3. **Server does not manage the updater's lifecycle.** The user starts both
   processes independently (or via systemd/s6).

## Consequences
- No server code changes needed for mDNS. The existing `/ctl add` handler
  already rejects writes to existing names, which prevents mDNS from
  overwriting user-configured targets.
- No mDNS library dependency in the server. The updater owns all mDNS logic.
- Consistency model is the same as any other `/ctl` client: last-write-wins.
  mDNS updates and user updates are ordered by the config RwLock.
- Latency of mDNS updates is one 9P round-trip (~ms) — negligible for events
  on human timescales (seconds to minutes).
- Easy to test: the updater is an independent binary with its own unit tests.
  Integration tests use a real 9P connection to a server.
- Easy to disable: don't run the updater.
- The `/ctl` command format (ADR-000-topics) serves as the protocol contract
  between the server and any automated client.
