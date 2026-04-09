.PHONY: build test format check clippy clean all

all: build test

build:
	cargo build

test: build
	cargo test --lib
	bats --print-output-on-failure tests/charlesnetfs.bats

format:
	cargo fmt

check:
	cargo check

clippy:
	cargo clippy -- -D warnings

clean:
	cargo clean
