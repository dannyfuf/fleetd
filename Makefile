SHELL := /bin/bash
.DEFAULT_GOAL := run

RELEASE ?= 0
CARGO_TARGET_DIR ?= target
FLEET_HOME ?= $(HOME)/.fleet

ifeq ($(RELEASE),1)
PROFILE := release
CARGO_PROFILE_FLAG := --release
else
PROFILE := debug
CARGO_PROFILE_FLAG :=
endif

BIN_DIR := $(abspath $(CARGO_TARGET_DIR))/$(PROFILE)
FLEET := $(BIN_DIR)/fleet
FLEETD := $(BIN_DIR)/fleetd
RUNTIME_ENV := FLEET_HOME="$(FLEET_HOME)" FLEET_DAEMON="$(FLEETD)"

.PHONY: run run-release restart daemon build check test test-scripts fmt fmt-check clippy lint ci clean bootstrap doctor help

run: restart ## Build, restart fleetd, and open Fleet (ARGS="..." supported)
	$(RUNTIME_ENV) "$(FLEET)" $(ARGS)

run-release: ## Build and run Fleet with the release profile
	$(MAKE) run RELEASE=1 ARGS="$(ARGS)"

restart: build ## Restart fleetd from this build (run it after changing daemon code)
	$(RUNTIME_ENV) "$(FLEET)" daemon restart

daemon: restart ## Restart fleetd from this build and follow its log
	@echo "Following $(FLEET_HOME)/logs/fleetd.log (Ctrl-C stops following logs)."
	tail -n 100 -F "$(FLEET_HOME)/logs/fleetd.log"

build: ## Build the workspace (set RELEASE=1 for release)
	cargo build --workspace $(CARGO_PROFILE_FLAG)

check: ## Check the workspace
	cargo check --workspace --all-targets

test: ## Run workspace tests
	# App socket tests launch target/debug/fleetd; cargo test only builds its test harness.
	cargo build -p fleet-daemon
	FLEET_DAEMON="$(abspath $(CARGO_TARGET_DIR))/debug/fleetd" cargo test --workspace

test-scripts: ## Run platform-specific shell-script tests
ifeq ($(shell uname -s),Darwin)
	./scripts/tests/bootstrap-zig-test.sh
else
	@echo "Skipping script tests: bootstrap-zig is only supported on Darwin."
endif

fmt: ## Format all Rust code
	cargo fmt --all

fmt-check: ## Check Rust formatting
	cargo fmt --all -- --check

clippy: ## Run Clippy with warnings denied
	cargo clippy --workspace --all-targets --all-features -- -D warnings

lint: fmt-check clippy ## Run formatting and Clippy checks

ci: lint test test-scripts ## Run lint and test targets

clean: ## Remove Cargo build artifacts
	cargo clean

bootstrap: ## Install and verify the pinned Zig toolchain
	./scripts/bootstrap-zig.sh

doctor: build ## Build Fleet and run its diagnostics
	$(RUNTIME_ENV) "$(FLEET)" doctor

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; print "Fleet development targets:"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
