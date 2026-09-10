# Reviewer checklist — Rust + GPUI testing (fleetd)

Standalone. Apply to any fleetd diff that adds or changes tests, adds a fake or a seam, or
fixes a bug. Each item is yes/no; "no" means apply the fix in the same PR or say why not.
Zed references are tag v1.18.1; fleetd paths are repo-relative.

## Determinism

- [ ] **Does every test touching an `Entity`, `Window` or GPUI executor use `#[gpui::test]`?**
      A bare `#[test]` gets no seeded dispatcher, no forbidden parking, no leak detection at
      teardown (`zed/crates/gpui_macros/src/test.rs:185-205`).
      *Fix:* swap the attribute and take `cx: &mut gpui::TestAppContext` in the signature.

- [ ] **Are all waits `run_until_parked`, `advance_clock`, or an awaited signal?**
      `std::thread::sleep`, `smol::Timer::after` and unpaused `tokio::time::sleep` are not
      tracked by the test scheduler, so the pump reports "nothing left to run" while work is
      pending.
      *Fix:* `cx.run_until_parked()` / `cx.executor().advance_clock(D)` / `cx.condition(...)`;
      on the tokio side `tokio::time::advance` under `start_paused`.

- [ ] **Is a timeout or backoff proven by stepping past its constant, not by waiting it out?**
      A test that waits a real 5 s deadline is slow and load-sensitive, and never proves the
      deadline is what fired.
      *Fix:* inject the duration (as `LinkOptions` does,
      `crates/fleet-daemon/src/machines/link.rs:873-877`) and advance the clock past it.

- [ ] **Is `allow_parking()` absent?**
      Parking is forbidden by default so a missing waker or real I/O fails loudly
      (`zed/crates/gpui/src/platform/test/dispatcher.rs:29`). fleetd has zero uses.
      *Fix:* find the untracked wait and put its dependency behind a trait with a fake.

- [ ] **Does every tokio test with a timing assertion use `#[tokio::test(start_paused = true)]`?**
      `test-util` is already enabled in `crates/fleet-daemon/Cargo.toml:45`.
      *Fix:* add `start_paused = true` and replace `sleep` waits with `tokio::time::advance`.

- [ ] **Is the test free of "poll until true with a wall-clock deadline" loops?**
      They convert an ordering bug into a flake and a 5 s tax.
      *Fix:* await the event or notification the code actually emits.

## Fakes and seams

- [ ] **Does a new external dependency (process, socket, filesystem, clock, HTTP) arrive behind a trait in `adapters/`?**
      Services take `Arc<dyn Trait>`; nothing calls `Command::new`, `Utc::now` or `std::fs`
      directly (`crates/fleet-daemon/src/adapters/clock.rs:6-14`).
      *Fix:* add the port before the implementation.

- [ ] **Does its fake live in `crates/fleet-daemon/src/testing/` behind `#[cfg(any(test, feature = "test-support"))]`?**
      Gating in the production crate is what lets downstream crates enable it as a
      dev-dependency feature (`crates/fleet-daemon/src/lib.rs:10-11`, `crates/fleet-daemon/Cargo.toml:16`).
      *Fix:* move it out of the test module and gate `pub mod testing`.

- [ ] **Does the fake implement the *same* trait production uses — no parallel API?**
      A fake that drifts from the real impl is worse than no fake.
      *Fix:* prefer the production adapter over a fake primitive, as
      `type FakeGit = ShellGit<FakeShell>` does (`crates/fleet-daemon/src/testing/fakes.rs`).

- [ ] **Does the fake fail loudly on an unanticipated call?**
      `FakeShell` returns status 127 with `unmatched fake command: …`
      (`crates/fleet-daemon/src/testing/fakes.rs:82-88`); a permissive default hides bugs.
      *Fix:* make the unmatched branch an error, not a success.

- [ ] **Do tests use `FakeFiles` / `FixedClock` unless the subject under test *is* the real filesystem or clock?**
      101 `RealFiles` against 74 `FakeFiles` today; a real tempdir tree is ~20× slower and
      leaks on panic.
      *Fix:* swap to `FakeFiles`; keep `tempfile` for `RealFiles` tests and for git
      (`crates/fleet-git/tests/support/mod.rs`).

- [ ] **Did a new heavyweight aggregate get a `::test(...)` constructor instead of 40 copied setup lines?**
      Zed's model: `Project::test` (`zed/crates/project/src/project.rs:2087`).
      *Fix:* add the constructor gated on `test-support`; do not let it hide logic that
      needed coverage.

- [ ] **Do GPUI tests call a shared `init_test(cx)` rather than repeating global setup?**
      `cx.set_global(Theme::dark()); keymap::init(cx)` is duplicated across all 28 GPUI test
      files (`crates/fleet-app/src/dialogs/input/tests.rs:28-31`).
      *Fix:* add `init_test` to `crates/fleet-app/src/state/test_support.rs` and migrate the
      tests you touch.

## Layering

- [ ] **Do adapter tests assert exact argv, and service tests assert domain results and side-effect order?**
      `docs/ARCHITECTURE.md:392-393` is the authority.
      *Fix:* move the argv assertion into the adapter's own test
      (`crates/fleet-daemon/src/adapters/git.rs:486-500` is the model).

- [ ] **Does the test avoid asserting private view fields or internal struct shape?**
      Those assertions break on refactor and prove nothing a user observes.
      *Fix:* assert through a pure projection — `board_screen::counts`
      (`crates/fleet-app/tests/board_flow.rs:32`) or a new
      `*_as_strings(props, cx) -> Vec<String>`.

- [ ] **Is UI behaviour driven through `simulate_keystrokes` / `dispatch_action` on a real focus chain?**
      That is what proves a binding is reachable, not merely declared in `keymap.rs`.
      *Fix:* mount the element in a minimal `impl Render` host with `key_context` and
      `track_focus`, as `crates/fleet-app/src/dialogs/input/tests.rs:4-22` does.

- [ ] **Are geometry assertions confined to `debug_selector` + `cx.debug_bounds` after `cx.draw`?**
      Screenshot diffing is not a fleetd technique; Zed's own image baselines are `#[ignore]`d
      and gitignored (`zed/crates/zed/src/zed/visual_tests.rs:427,439`).
      *Fix:* assert bounds, or assert the projection instead.

- [ ] **Does a new `#[gpui::test]` or `#[tokio::test]` in `fleet-core` / `fleet-proto` have a justification?**
      Both crates are pure by contract (no I/O, no clock — ADR 0008). Needing a runtime is a
      layering violation, not a testing choice.
      *Fix:* push the I/O out to a caller and test the pure function with `#[test]`.

## Real processes

- [ ] **Does a test that spawns `fleetd` prove a genuinely cross-process concern?**
      Socket, handshake, server-side event filtering, default subscriptions, restart. Anything
      else can run in process.
      *Fix:* move it to a `tokio::io::duplex` harness against `Router` plus fakes.

- [ ] **Is there exactly one real-daemon test per behaviour, not one per assertion?**
      Each costs a process, a `TempDir` and serialization against socket contention.
      *Fix:* fold the assertions into the existing test in
      `crates/fleet-app/tests/` or `crates/fleet-daemon/tests/`.

- [ ] **Does the fixture isolate `FLEET_HOME`, `HOME` and `PATH`, and clean up in `Drop`?**
      `crates/fleet-app/tests/common/mod.rs:26-55` and
      `crates/fleet-daemon/tests/infra/mod.rs:45-52` already do.
      *Fix:* reuse those fixtures rather than spawning a child by hand.

- [ ] **Does the PR description say the suite needs `cargo build -p fleet-daemon` first?**
      `make test` handles it (`Makefile:43-46`); a bare `cargo test -p fleet-app` tests a
      stale binary (`docs/DEVELOPMENT.md:60-63`).

## Shape and hygiene

- [ ] **Is the test named as a sentence stating the invariant?**
      fleetd convention, not Zed's `test_` prefix — 5 of ~2,160 use the prefix. Example:
      `nudge_wakes_a_sleeping_link_and_restarts_backoff_from_the_floor`
      (`crates/fleet-daemon/src/machines/link.rs:869`). Randomized tests keep `test_random_*`.

- [ ] **Do assertion helpers carry `#[track_caller]`?**
      Without it the failure points at the helper, not the failing test
      (`zed/crates/editor/src/test/editor_test_context.rs:388,608`). No helper in
      `crates/fleet-app/src/state/test_support.rs` has it yet.

- [ ] **Do multi-line and collection comparisons use `pretty_assertions::assert_eq` and `indoc!` fixtures?**
      Neither is a workspace dependency yet; add to `[workspace.dependencies]` when first
      needed (all fleetd deps are declared there and inherited with `.workspace = true`).

- [ ] **Does the test use `unwrap_or_else(|error| panic!("{error}"))` rather than bare `unwrap()` where the error carries information?**
      House style (`crates/fleet-proto/src/lib.rs:29-30`); it keeps the message.

- [ ] **Is the test in the right place?**
      Inline `#[cfg(test)] mod tests` by default (343 modules); a sibling `tests.rs` /
      `tests/` module when the suite outgrows the file; `crates/<crate>/tests/` only for
      cross-crate or binary-launching suites. Never a `mod.rs` for a module (the
      `tests/common/mod.rs` form is Cargo's requirement for shared integration helpers and is
      the exception).

- [ ] **Does `#[ignore]` carry a reason string?**
      All three fleetd uses do (`crates/fleet-daemon/src/machines/tailscale.rs:726`,
      `crates/fleet-term/src/host/owner/tests.rs:648`,
      `crates/fleet-term/benches/viewport.rs:9`).
      *Fix:* `#[ignore = "why, and how to run it"]`.

- [ ] **Is platform gating `#[cfg(target_os = "…")]` rather than a runtime early return?**
      A runtime skip reports as a pass.

- [ ] **Does the test leave no `TempDir`, child process or detached task alive?**
      RAII `Drop`, as `crates/fleet-daemon/tests/infra/mod.rs:45-52` does.

## Coverage

- [ ] **Does a non-trivial change ship with a test?**
      Tests are ~34% of this repo and are its specification.

- [ ] **Does a bug fix ship a test that fails on the parent commit, in `crates/<crate>/tests/bugfix_*.rs`?**
      Existing files: `bugfix_connection.rs`, `bugfix_terminal_spawn.rs` (fleet-client);
      `bugfix_mutation.rs`, `bugfix_parse_patch.rs`, `bugfix_read_watch.rs`,
      `bugfix_rebase.rs` (fleet-git). Add to the area's file; do not create `_2`.
      *Fix:* check out the parent commit, apply only the test, confirm it fails.

- [ ] **Does new merge / reconciliation / reassembly logic ship a seeded randomized test?**
      `#[gpui::test(iterations = 50)] fn test_random_*(mut rng: StdRng)` with a reference
      model and a `check_invariants()` (`zed/crates/text/src/tests.rs:51-110`). fleetd has none
      yet; `crates/fleet-core/src/board/sync/reconcile.rs` and `fleet-term` frame reassembly
      are the candidates.

- [ ] **Does a new protocol field or variant have a golden-frame test?**
      `crates/fleet-proto/tests/compatibility.rs` asserts byte-exact frames and that legacy
      payloads still parse. Additive fields need `#[serde(default)]` plus a defaulting test.

- [ ] **Is a new slow or socket-contended test registered in a nextest profile?**
      There is no `.config/nextest.toml` yet (`make test` runs `cargo test --workspace`,
      `Makefile:43-46`). If you add one: 60 s `slow-timeout, terminate-after = 1`, and a
      `max-threads = 1` group for the binary-launching suites.

## Docs

- [ ] **Did the governing `docs/` section change in the same commit when a contract moved?**
      `docs/ARCHITECTURE.md:390-398` governs the testing strategy and the adapter-vs-service
      split; `docs/DEVELOPMENT.md:55-63` governs how the suites are run. `docs/README.md`
      assigns each document its domain, and a change that contradicts a doc is a bug in one
      of the two.

- [ ] **Is the commit message `<area>: <imperative lowercase summary>`?**
      `tests:` for a pure test change, or the owning crate short name (`daemon:`, `app:`,
      `client:`, `core:`) when the test ships with the fix.
