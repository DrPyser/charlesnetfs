# CharlesNetFS

A network filesystem that exposes network hosts as a filesystem using the 9P protocol.

## Quick Start

```bash
# see shell.nix for system & dev dependencies
nix-shell

# Build
cargo build

# Run with defaults
cargo run
```

## Configuration

Create `config.yaml`.
See [config.yaml.sample](config.yaml.sample).
Specify file path using `--config`.

```yaml
global_timeout_ms: 5000
targets:
  router:
    addr: 192.168.1.1:22
  nas:
    addr: 192.168.1.100:5000
    timeout_ms: 3000
```

Each target create a directory with `ping` and `config` files.

## Usage

- Read `target/ping` to see TCP ping results
- Read `target/config` to see target-specific config
- Write to `config` at root to reload global config

## Tests

```bash
make test
```
