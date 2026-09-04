.PHONY: check build run-app run-daemon test fmt clippy

check:
	cargo check --workspace

build:
	cargo build --workspace

run-app:
	cargo run -p fleet-app

run-daemon:
	cargo run -p fleet-daemon

test:
	cargo test --workspace

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings
