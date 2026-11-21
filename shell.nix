# { pkgs ? (import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/master.tar.gz") {})}:
{ pkgs ? (import <nixpkgs> { }) }:

let
  basePackages = with pkgs; [
    fuse
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
  buildInputs = basePackages;
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
  command -v bats rustc cargo act 9
  '';
}
