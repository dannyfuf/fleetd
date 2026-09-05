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

.PHONY: run run-release daemon build check test fmt fmt-check clippy lint ci clean bootstrap doctor help

run: build ## Build, restart fleetd, and open Fleet (ARGS="..." supported)
	$(RUNTIME_ENV) "$(FLEET)" daemon restart
	$(RUNTIME_ENV) "$(FLEET)" $(ARGS)

run-release: ## Build and run Fleet with the release profile
	$(MAKE) run RELEASE=1 ARGS="$(ARGS)"

daemon: build ## Restart fleetd from this build and follow its log
	$(RUNTIME_ENV) "$(FLEET)" daemon restart
	@echo "Following $(FLEET_HOME)/logs/fleetd.log (Ctrl-C stops following logs)."
	tail -n 100 -F "$(FLEET_HOME)/logs/fleetd.log"

build: ## Build the workspace (set RELEASE=1 for release)
	cargo build --workspace $(CARGO_PROFILE_FLAG)

check: ## Check the workspace
	cargo check --workspace

test: ## Run workspace tests
	cargo test --workspace

fmt: ## Format all Rust code
	cargo fmt --all

fmt-check: ## Check Rust formatting
	cargo fmt --all -- --check

clippy: ## Run Clippy with warnings denied
	cargo clippy --workspace --all-targets --all-features -- -D warnings

lint: fmt-check clippy ## Run formatting and Clippy checks

ci: lint test ## Run lint and test targets

clean: ## Remove Cargo build artifacts
	cargo clean

bootstrap: ## Install and verify the pinned Zig toolchain
	./scripts/bootstrap-zig.sh

doctor: build ## Build Fleet and run its diagnostics
	$(RUNTIME_ENV) "$(FLEET)" doctor

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; print "Fleet development targets:"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
