# { pkgs ? (import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/master.tar.gz") {})}:
{ pkgs ? (import <nixpkgs> { }), withFuse ? true }:

let
  basePackages = with pkgs; [
    pkg-config
    jq
    plan9port
    rustup
    fossil
    # github action local runner
    act
  ];

  rustupHome = "$PWD/.rustup";
  cargoHome = "$PWD/.cargo";

in pkgs.mkShell {
  # omit fuse to reuse fusermount3 from environment
  buildInputs = basePackages ++ (if withFuse then with pkgs; [ fuse ] else []);
  RUST_BACKTRACE = "1";
  RUST_LOG = "debug";
  shellHook = ''
  export RUSTUP_HOME=${rustupHome}
  export CARGO_HOME=${cargoHome}
  export PATH=${cargoHome}/bin:$PWD/tests/bats/bin:$PATH

  if ! rustup toolchain list | grep -q stable; then
    rustup default stable
  fi
  rustup component add rust-analyzer 2>/dev/null
  command -v bats rustc cargo act 9 fusermount3 >&2
  '';
}
