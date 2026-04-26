.PHONY: build test format check clippy clean all

all: check build test

RUST_BACKTRACE := full
build:
	cargo build

./target/debug/%:
	cargo build --bin $*

test: build
	cargo test --lib
	bats --print-output-on-failure tests/charlesnetfs.bats

format:
	cargo fmt

check:
	cargo check
	cargo clippy -- -D warnings

clean:
	cargo clean
