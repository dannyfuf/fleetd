# Build-time follow-ups

What is left from the compile-time investigation of 2026-09-24 (branch `chore/compile-time`),
after the changes that landed with it:

- Zed-shaped dev profile — dependencies at `opt-level = 0` except a hot list, `debug = "limited"`
  (`Cargo.toml`, ADR 0001).
- `fleet-drive` lost its `default` feature list, which forked four crates into two builds each.
- One integration-test binary per crate (`tests/integration.rs`, guarded by `workspace_layering`).
- Makefile targets `release`, `timings`, `build-info`, `clean-release`, `clean-incremental`, and
  the "Building" section of `docs/DEVELOPMENT.md` that explains them.
- Per machine, not in the repo: sccache shares compiled dependencies between worktrees
  (`docs/DEVELOPMENT.md`, "Sharing dependencies between worktrees").

Measured on a 12-core, 16 GB Linux machine:

| | Before | After |
| --- | --- | --- |
| Cold `cargo build --workspace` | 14m28s, swap to 12 GB | 4m52s, no swap growth |
| Rebuild after an edit in `fleet-app` | 37 s | 11 s |
| Rebuild after an edit in `fleet-core` | 56 s | 38 s |
| `make test`'s `cargo build -p …` after `make build` | 4m33s (4 crates rebuilt) | 1 s |
| `cargo test --workspace --no-run` after a build | 14m53s, 10.3 GB peak | 7m09s, 8.6 GB peak |
| First build of a new worktree, sccache warm | 14m28s | ~4m (74% of `rustc` calls hit) |

Each item below is independent. They are ordered by payoff.

## 1. Split `fleet-app` into crates

**Why.** After the profile change, an edit in the app costs ~11 s, and ~9 s of that is
type-checking and codegen for the single 112k-line `fleet-app` crate; linking is ~2 s. Rust
compiles a crate as a unit, so every edit anywhere in the app re-checks all of it, and one crate
cannot use more than a slice of the 12 cores. It also sits last on the cold-build critical path
(gpui → ui-kit → lazygit → app, ~230 s for the app alone under load).

**Shape today** (lines of Rust under `crates/fleet-app/src/`):

| Module | Lines |
| --- | --- |
| `screens/` | 31.3k |
| `dialogs/` | 28.2k |
| `views/` | 14.1k |
| `state/` | 12.8k |
| `shell/` | 7.6k |
| `terminal/`, `action_catalogue/`, `bridge/`, `presentation/`, `watches/`, `drive/` | 11.1k together |
| top-level files (`keymap.rs`, `drive.rs`, `actions.rs`, …) | 6.1k |

**Plan.** Cut along the direction the code already has. Leaves first, so each step is buildable
and shippable on its own:

1. `fleet-app-state` — `state/`, the bridge types and the presentation projections: the model
   everything else reads. No rendering.
2. `fleet-app-views` — `views/` and `presentation/`, depending on state and `fleet-ui-kit`.
3. `fleet-app-dialogs` and `fleet-app-screens` — the two largest trees, both leaves over views.
4. `fleet-app` keeps `shell/`, the keymap, actions and `main`: the composition root.

**Watch for.**

- `docs/APP-CONTRACTS.md` makes the single `Entity<AppState>` and free
  `render(props, …, cx) -> AnyElement` functions deliberate. The split is by crate, not by
  entity; it must not introduce per-screen entities.
- `docs/ARCHITECTURE.md` owns the crate graph, and `workspace_layering`
  (`crates/fleet-core/tests/workspace_layering.rs`) enforces it: add the new crates and their
  forbidden edges (screens must not depend on dialogs' internals, state must not depend on views).
- `Dialogs::` is matched at ~140 sites and `RequestBody::` is built at ~139 sites inside
  views/screens/dialogs (`rust-workspace-architecture`, items 4 and 6). A split forces those
  seams to become real APIs; do the `Dialog` trait extraction first or alongside.
- Each new crate gets the workspace manifest preamble and `[lints] workspace = true`.

**Payoff.** An edit in a screen re-checks one crate of ~30k lines instead of 112k, and
independent crates build in parallel on a cold build. Expect the edit loop well under 10 s.
This is the largest item, so do it one crate per PR.

## 2. Stop rebuilding Ghostty in every worktree

**Why.** `libghostty-vt-sys`'s build script `git clone`s Ghostty and runs a full `zig build`
into its `OUT_DIR` in every worktree. With sccache serving the other dependencies, it is the
critical path of a new worktree's first build: ~100 s of the ~205 s a fully cached build takes,
with `fleet-term` and so `fleet-daemon` waiting on it. sccache cannot help: it caches `rustc`
invocations, not build-script work.

**Options, cheapest first.**

- Set `GHOSTTY_SOURCE_DIR` (read by the build script) to one pinned local checkout, so no
  worktree clones. Saves the clone, not the compile, since `--cache-dir` stays in `OUT_DIR`.
- Set `GHOSTTY_ZIG_SYSTEM_DIR` so Zig resolves Ghostty's packages from a prepared store instead
  of fetching them.
- Build libghostty-vt once and use the crate's `pkg-config` feature (`try_pkg_config` in its
  `build.rs`) to link the installed library. Needs the version pinned to `libghostty-vt =0.2.1`
  (`Cargo.toml`) and a `make bootstrap` step on Linux as well as macOS
  (`scripts/bootstrap-zig.sh` is aarch64-macOS only today).

**Payoff.** Up to ~100 s off every new worktree's first build, the largest single item left there.

## 3. Make GPUI optional in `fleet-drive`

**Why.** `fleet-drive` depends on `gpui` unconditionally, so `fleet-harness` (a CLI that speaks
a socket protocol) and, through its dev-dependency on the harness, `fleet-daemon`'s tests wait
for all of GPUI to compile. `cargo test -p fleet-daemon` from a cold tree builds GPUI for nothing.

**Shape.** `fleet-harness` imports only `fleet_drive::protocol` (`client.rs`, `fault.rs`,
`rundir.rs`, `scenario.rs`). GPUI is used only by `input.rs` and the `legacy/` driver.

**Plan.** Make `gpui` an optional dependency; add a `protocol` feature (serde types only) that
`socket` implies together with `dep:gpui`, or split the GPUI-free protocol types into a tiny
`fleet-drive-protocol` crate. Point `fleet-harness` at the protocol half.
`docs/TESTING-HARNESS.md` is frozen: the protocol types move, the wire does not change.

**Payoff.** `cargo test -p fleet-daemon` and `cargo build -p fleet-harness` no longer compile
GPUI, gpui_platform, wgpu, naga and the Wayland/X11 stacks (several minutes cold).

## 4. Test-only features fork most of the graph

**Why.** `cargo test` builds a second copy of much of the workspace because dev-dependencies add
features: `tokio/test-util` (in `fleet-daemon`, `fleet-client`, `fleet-harness`) re-builds tokio,
hyper, reqwest, tower and every Fleet crate; `gpui/test-support` re-builds gpui, its platform
crates, `fleet-ui-kit`, `fleet-lazygit` and `fleet-app`. After `make build`, `cargo test` had to
recompile 43 crates.

**Options.**

- `tokio/test-util` is small and harmless in a production build. Adding it to the workspace
  `tokio` entry unifies the build and test variants of tokio and everything above it. Measure the
  gain before taking it.
- `gpui/test-support` should stay out of production binaries. Leave it.

**Payoff.** Estimated a few minutes off the first `make test` after each `make build`; not
measured.

## 5. Tune the release profile

**Why.** `[profile.release]` is Cargo's default: `opt-level = 3`, 16 codegen units, no LTO, no
debuginfo. Nothing was measured for it in this pass beyond confirming `make release` works
(6m12s cold, 4.1 GB peak, on the machine above).

**Options.** Zed ships `lto = "thin"`, `codegen-units = 1` and `debug = "limited"`, with a
`release-fast` profile (`lto = false`, 16 units) for local performance work
(`.claude/skills/gpui-performance/references/patterns.md`, p14). `codegen-units = 1` on the
112k-line `fleet-app` makes its release compile serial and memory-heavy on a 16 GB machine; thin
LTO alone is the cheaper half. `debug = "line-tables-only"` would give release crash backtraces
file and line numbers.

**Measure first:** release build time and peak memory, binary size, and a frame-time profile of
the app for each option. Keep `make release` for day-to-day optimized builds either way.

## 6. Two harness scenarios race under heavy load

**Why.** `agents/composer-editing` and `agents/subagent-blocked-child-paints-caller` failed in
`make test`'s headless subset while another worktree's `cargo test` held the machine at a load
average of 35–42 on 12 cores. On an idle machine they passed 20 of 20 runs with the new profile;
the old profile failed `composer-editing` once in 20. Not caused by the build changes.

**Shape.** In the delegation one, the scripted Claude peer waits 20 s for a frame that never comes
after a client connection resets (`crates/fleet-harness/src/agent/peer.rs`), and the delegation
lands `cancelled` instead of `working`. `composer-editing` asserts `AgentWorking` after the scripted
turn has already finished. Both are timing assumptions in the scenario or the fake, worth a look
under `rust-gpui-testing` before they flake in CI.

## 7. Machine hygiene (per developer, not the repo)

- Each old worktree's `target/` still holds 13–100 GB of artifacts built with the old profile,
  ~370 GB across the worktrees measured. None of it is reused after the profile change; delete
  them with `cargo clean` in each worktree (or remove the worktree).
- sccache is configured at `~/.fleet/worktrees/<owner>/fleetd/.cargo/config.toml`, which covers
  every worktree below it. A developer on another machine sets the same `rustc-wrapper` there.
