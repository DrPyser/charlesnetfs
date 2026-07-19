.PHONY: build test tests/cargo tests/bats format check clippy clean all

all: check build test

RUST_BACKTRACE := full
build:
	cargo build

./target/debug/%:
	cargo build --bin $*

test/cargo: build
	cargo test --lib

test/bats: build
	bats --print-output-on-failure tests

test: build test/cargo test/bats
	@echo Ran all tests

format:
	cargo fmt

check:
	cargo check
	cargo clippy -- -D warnings

clean:
	cargo clean
