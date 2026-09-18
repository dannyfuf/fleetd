SHELL := /bin/bash
.DEFAULT_GOAL := run

RELEASE ?= 0
CARGO_TARGET_DIR ?= target
FLEET_HOME ?= $(HOME)/.fleet
SWEEP_DAYS ?= 7

ifeq ($(RELEASE),1)
PROFILE := release
CARGO_PROFILE_FLAG := --release
else
PROFILE := debug
CARGO_PROFILE_FLAG :=
endif

BIN_DIR := $(abspath $(CARGO_TARGET_DIR))/$(PROFILE)
TEST_BIN_DIR := $(abspath $(CARGO_TARGET_DIR))/debug
FLEET := $(BIN_DIR)/fleet
FLEETD := $(BIN_DIR)/fleetd
RUNTIME_ENV := FLEET_HOME="$(FLEET_HOME)" FLEET_DAEMON="$(FLEETD)"

# The GUI harness (docs/TESTING-HARNESS.md). The runner walks a corpus directory recursively
# and reads both *.scenario and *.txt, so a new corpus directory needs no Makefile change.
HARNESS := $(BIN_DIR)/fleet-harness
HARNESS_ENV := FLEET_APP="$(FLEET)" FLEET_DAEMON="$(FLEETD)"
HARNESS_DIR ?= scenarios
LANE ?= virtual
# Directives the headless lane cannot honour, whatever the scenario does: `shot` has no
# compositor surface to photograph and `clipboard` cannot read its own write back
# (docs/TESTING-HARNESS.md, sections 1 and 4). A scenario naming either is pixel-bound and runs
# only in the `virtual` lane. crates/fleet-app/tests/harness_headless.rs applies this same rule
# and no other, so the two selectors cannot drift apart.
HARNESS_PIXEL_BOUND := ^[[:space:]]*(shot|clipboard)([[:space:]]|$$)

.PHONY: run run-release restart daemon build check test test-scripts harness harness-headless harness-one harness-prune fmt fmt-check clippy lint ci clean prune fresh bootstrap doctor help

run: restart ## Build, restart fleetd, and open Fleet (ARGS="..." supported)
	$(RUNTIME_ENV) "$(FLEET)" $(ARGS)

run-release: ## Build and run Fleet with the release profile
	$(MAKE) run RELEASE=1 ARGS="$(ARGS)"

restart: build ## Restart fleetd from this build (run it after changing daemon code)
	$(RUNTIME_ENV) "$(FLEET)" daemon restart

daemon: restart ## Restart fleetd from this build and follow its log
	@echo "Following $(FLEET_HOME)/logs/fleetd.log (Ctrl-C stops following logs)."
	tail -n 100 -F "$(FLEET_HOME)/logs/fleetd.log"

build: prune ## Build the workspace (set RELEASE=1 for release)
	cargo build --workspace $(CARGO_PROFILE_FLAG)

check: ## Check the workspace
	cargo check --workspace --all-targets

test: ## Run workspace tests
	# App socket tests launch target/debug/fleetd, and the headless harness subset launches
	# fleet-harness, fleet and fleetd; cargo test only builds its own test harnesses.
	cargo build -p fleet-daemon -p fleet-app -p fleet-harness
	FLEET_DAEMON="$(TEST_BIN_DIR)/fleetd" FLEET_APP="$(TEST_BIN_DIR)/fleet" \
		FLEET_HARNESS_BIN="$(TEST_BIN_DIR)/fleet-harness" cargo test --workspace

harness: build ## Run the whole GUI corpus in the virtual lane (HARNESS_ARGS=--update-baselines)
	$(HARNESS_ENV) "$(HARNESS)" run "$(HARNESS_DIR)" --lane virtual $(HARNESS_ARGS)

harness-headless: build ## Run the GUI corpus scenarios that need no pixels
	@files=$$(grep -rLE --include='*.scenario' --include='*.txt' '$(HARNESS_PIXEL_BOUND)' "$(HARNESS_DIR)"); \
	if [ -z "$$files" ]; then \
		echo "No pixel-free scenario under $(HARNESS_DIR)/: all $$(grep -rlE --include='*.scenario' --include='*.txt' '$(HARNESS_PIXEL_BOUND)' "$(HARNESS_DIR)" | wc -l) of them take a"; \
		echo "shot, and a shot fails in the headless lane (docs/TESTING-HARNESS.md section 4)."; \
		echo "Run the corpus with 'make harness'."; \
		exit 0; \
	fi; \
	status=0; \
	for file in $$files; do \
		echo "== $$file"; \
		$(HARNESS_ENV) "$(HARNESS)" run "$$file" --lane headless || status=1; \
	done; \
	exit $$status

harness-prune: ## Delete harness run directories older than SWEEP_DAYS (default 7)
	@root=$${HARNESS_RUNS:-$${TMPDIR:-/tmp}/fleet-harness}; \
	if [ ! -d "$$root" ]; then echo "No harness runs under $$root/."; exit 0; fi; \
	echo "$$root before: $$(du -sh "$$root" 2>/dev/null | cut -f1), $$(find "$$root" -mindepth 1 -maxdepth 1 -type d | wc -l) runs"; \
	find "$$root" -mindepth 1 -maxdepth 1 -type d -mtime +$(SWEEP_DAYS) -exec rm -rf {} +; \
	echo "$$root after:  $$(du -sh "$$root" 2>/dev/null | cut -f1), $$(find "$$root" -mindepth 1 -maxdepth 1 -type d | wc -l) runs"

harness-one: build ## Run one GUI scenario (SCENARIO=path, LANE=virtual|headless|attach)
	@test -n "$(SCENARIO)" || { echo "SCENARIO is required, for example: make harness-one SCENARIO=scenarios/hub/help.scenario"; exit 2; }
	$(HARNESS_ENV) "$(HARNESS)" run "$(SCENARIO)" --lane $(LANE) $(HARNESS_ARGS)

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

prune: ## Drop build artifacts unused for SWEEP_DAYS days or built by uninstalled toolchains
	@[ -d "$(CARGO_TARGET_DIR)" ] || exit 0; \
	if ! cargo sweep --version >/dev/null 2>&1; then \
		echo "Installing cargo-sweep (one-time)..."; \
		cargo install cargo-sweep --locked || { echo "cargo-sweep unavailable; skipping prune."; exit 0; }; \
	fi; \
	cargo sweep --installed >/dev/null && cargo sweep --time $(SWEEP_DAYS) >/dev/null; \
	echo "target/ after prune: $$(du -sh "$(CARGO_TARGET_DIR)" 2>/dev/null | cut -f1)"

fresh: ## Delete target/ and rebuild from scratch (set RELEASE=1 for release)
	cargo clean
	$(MAKE) build RELEASE=$(RELEASE)

bootstrap: ## Install and verify the pinned Zig toolchain
	./scripts/bootstrap-zig.sh

doctor: build ## Build Fleet and run its diagnostics
	$(RUNTIME_ENV) "$(FLEET)" doctor

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; print "Fleet development targets:"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-18s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
