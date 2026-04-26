# { pkgs ? (import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/master.tar.gz") {})}:
{ pkgs ? (import <nixpkgs> { }) }:

let
  basePackages = with pkgs; [
    fuse
    pkg-config
    rustc
    cargo
    rustfmt
    clippy
    rust-analyzer
    jq
    plan9port
  ];

in pkgs.mkShell {
  buildInputs = basePackages;
  RUST_BACKTRACE = "1";
  RUST_LOG = "debug";
  shellHook = ''
  export PATH=$PWD/tests/bats/bin:$PATH
  '';
}
