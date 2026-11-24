# ADR 1: Pin nixpkgs via flake input

Date: 2026-11-21

## Status
Accepted

## Context
The original `shell.nix` used `import <nixpkgs> {}`, which pulls whatever nixpkgs revision is in `NIX_PATH`. This is unpinned — builds vary across machines and time, and the shell closure (~2.2 GiB) is rebuilt whenever the system channel changes.

## Decision
Add a `flake.nix` with a pinned `nixpkgs` input (`github:NixOS/nixpkgs/nixos-25.05`). The flake's `devShells` delegate to the existing `shell.nix` by importing it with the pinned pkgs, so the shell definition stays in one place and `flake.lock` records the exact revision.

## Consequences
- `flake.lock` pins nixpkgs; `nix develop` is reproducible.
- First `nix develop` downloads ~461 MiB (nixpkgs tarball) and copies the shell closure. Subsequent calls are quick.
- Updating the pin is `nix flake update` (one command, updates `flake.lock`).
- `shell.nix` stays valid for non-flake users via the `pkgs ? import <nixpkgs>` fallback.
