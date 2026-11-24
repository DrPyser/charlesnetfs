# ADR 2: Manage Rust toolchain via rustup in shell.nix

Date: 2026-11-21

## Status
Accepted

## Context
`shell.nix` originally pulled `rustc`, `cargo`, `rustfmt`, `clippy`, `rust-analyzer` directly from nixpkgs. That's ~1.5 GiB of the nix closure just for Rust. It also locks the toolchain to whatever the pinned nixpkgs ships — can't switch to nightly or add targets easily.

## Decision
Remove the nixpkgs Rust packages from `shell.nix`. Keep only `rustup` (the manager). The `shellHook` sets `RUSTUP_HOME` and `CARGO_HOME` to project-local dirs, prepends `$CARGO_HOME/bin` to PATH, and runs `rustup default stable` (plus `rustup component add rust-analyzer`) on first entry.

## Consequences
- Nix closure drops from ~2.2 GiB to ~700 MiB.
- The toolchain lives in `.rustup/` (project-local, gitignored). No conflict with a system-wide rustup.
- `rustup default nightly`, `rustup target add wasm32-unknown-unknown`, etc. work without touching `shell.nix`.
- `rust-analyzer` is installed as a rustup component, lands in `$CARGO_HOME/bin` so kak-lsp finds it.
- First `nix develop` runs `rustup default stable` (network fetch). After that, the shell opens with no network delay.
