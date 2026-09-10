---
name: rust-gpui-testing
description: How to write deterministic tests for fleetd's two worlds — GPUI tests (`#[gpui::test]`, `TestAppContext`, `VisualTestContext`, `run_until_parked`, `advance_clock`) in fleet-app/fleet-ui-kit/fleet-lazygit, and tokio tests (`#[tokio::test]`, adapter fakes behind `test-support`, paused clocks) in fleet-daemon/fleet-client/fleet-git. Load it before adding or changing any test, before adding a `Fake*` or a `::test(...)` constructor, before writing a regression test for a bug fix, and whenever a test sleeps, polls a wall clock, spawns the real `fleetd` binary, or is flaky. Also load it when reviewing a diff that adds tests, or when deciding where a new test file belongs (inline `mod tests`, `src/<thing>/tests.rs`, or `crates/<crate>/tests/`).
---

# Rust + GPUI testing in fleetd

How to write tests that fail for one reason only. Patterns verified against Zed v1.18.1,
the GPUI tag fleetd depends on (`Cargo.toml`), and against fleetd's own suites: 105
`#[gpui::test]`, 476 `#[tokio::test]`, ~74k test lines against ~146k production lines.
Tests are ~34% of this repo and are its real specification — treat a test as a contract,
not as coverage.

## When to use

- Adding or changing any test in `fleet-app`, `fleet-ui-kit`, `fleet-lazygit` (GPUI world)
  or `fleet-daemon`, `fleet-client`, `fleet-git`, `fleet-cli`, `fleet-core`, `fleet-proto`,
  `fleet-term` (tokio/pure world).
- Adding a new external dependency (process, socket, filesystem, clock, HTTP) and needing
  a seam: a trait in `adapters/` plus a `Fake*` in `crates/fleet-daemon/src/testing/`.
- Writing the regression test for a bug fix (`crates/<crate>/tests/bugfix_*.rs`).
- A test sleeps, polls, races, hangs, or passes locally and fails under load.
- Deciding where a test file goes, or whether a behaviour needs the real `fleetd` binary.

## When not to

- Pure-function tests over `fleet-core` types: a bare `#[test]` is correct and cheaper.
  195 of `fleet-core`'s tests are plain `#[test]` and should stay that way.
- Design-system *presentation* coverage: a component state is proven by a gallery example
  (`docs/DESIGN-SYSTEM.md:1252` — "If a state is not in a gallery, it is not implemented").
  See `gpui-components`.
- Choosing what a screen shows or which key does what — that is `docs/UX-SPEC.md` and
  `docs/KEYMAP.md`, not a test question.

## Rules

**Annotate every test that touches an `Entity`, a `Window`, or a GPUI executor with `#[gpui::test]`.**
The macro builds one seeded `TestDispatcher` per run, forbids parking, and asserts at
teardown that no entity or task leaked (`zed/crates/gpui_macros/src/test.rs:186-205`,
`zed/crates/gpui/src/app/entity_map.rs:88`). A bare `#[test]` around `cx.new(...)` gets none of that.
fleetd already does this in all 28 of its GPUI test files.

**Take the contexts you need as parameters; nothing else compiles.**
`#[gpui::test]` inspects each parameter's type name and injects `&mut TestAppContext`,
`&mut App`, `BackgroundExecutor` or `StdRng` — any other type is a compile error
(`zed/crates/gpui_macros/src/test.rs:143-148`, `:182`). Two `&mut TestAppContext` parameters give two
apps on one dispatcher, which is how a two-peer test stays deterministic.

**Drive time with `run_until_parked` and `advance_clock`, never with a real sleep.**
`cx.run_until_parked()` pumps until nothing can progress and advances the clock to the next
timer (`zed/crates/gpui/src/executor.rs:213-222`); `cx.executor().advance_clock(D)` makes timers ready
(`:200-204`). `smol::Timer::after` is not tracked by the scheduler and `dispatch_after`
panics on purpose (`zed/crates/gpui/src/platform/test/dispatcher.rs:132-134`). fleetd does this in
`crates/fleet-app/src/terminal/surface/tests.rs:78,98`.

**Never call `allow_parking()` to make a flaky test pass.**
Parking is forbidden by default (`zed/crates/gpui/src/platform/test/dispatcher.rs:29`) precisely so a missing
waker or a real-I/O dependency fails loudly; with parking allowed there is still a 15 s
hard kill (`zed/crates/scheduler/src/test_scheduler.rs:433-436`). If a test needs it, the real fix is
a fake behind a trait. fleetd has zero uses — keep it that way.

**In tokio tests, pause the clock instead of sleeping through a timeout.**
`#[tokio::test(start_paused = true)]` plus `tokio::time::advance` proves a backoff or a
deadline in microseconds. `fleet-daemon` already dev-depends on `tokio` with `test-util`
(`crates/fleet-daemon/Cargo.toml:45`) and the pattern exists once, at
`crates/fleet-daemon/src/machines/link.rs:868`. Extend it; do not add new
`tokio::time::sleep` waits.

**Fake at the trait boundary, in the production crate, behind `#[cfg(any(test, feature = "test-support"))]`.**
`fleet-daemon` gates `pub mod testing` exactly this way (`crates/fleet-daemon/src/lib.rs:10-11`,
`crates/fleet-daemon/Cargo.toml:16`), and `FakeShell`/`FakeProcess`/`FixedClock`/`FakeBackend`
(`crates/fleet-daemon/src/testing/fakes.rs:48,143,210,252`) implement the same traits production uses. Zed does
the same with `FakeFs: Fs` (`zed/crates/fs/src/fs.rs:1398`, trait impl at `:2855`). A fake with a parallel API is
worse than none.

**Default to `FakeFiles` and `FixedClock`; a real tempdir is for testing the real thing.**
Daemon tests still build real trees with `create_dir_all`/`fs::write`
(`crates/fleet-daemon/src/services/sessions/tests.rs:6-16`) — 101 `RealFiles` against 74
`FakeFiles`. Use `RealFiles` + `tempfile` only when the subject *is* the filesystem, or for
git (`crates/fleet-git/tests/support/mod.rs` drives real `git`, correctly).

**Give heavyweight aggregates a `::test(...)` constructor, and each GPUI crate one `init_test(cx)`.**
Zed's `Project::test` (`zed/crates/project/src/project.rs:2087`) and its per-crate `init_test`
are the model. fleetd hand-rolls `cx.set_global(Theme::dark()); keymap::init(cx)` in every
GPUI test (`crates/fleet-app/src/dialogs/input/tests.rs:28-31`). Extract one
`fleet_app::state::test_support::init_test(cx)` next time you touch that setup.

**Test UI through behaviour and a textual projection, never through pixels.**
Open a window, drive it with `simulate_keystrokes` / `dispatch_action` (each pumps the
scheduler for you, `zed/crates/gpui/src/app/test_context.rs:481,498`), then assert on a `Vec<String>` or a pure
view-model function. fleetd has 23 `simulate_keystrokes` and 44 `dispatch_action`, and
`board_screen::counts` is the right shape of projection
(`crates/fleet-app/tests/board_flow.rs:32`). Geometry is the only exception: `debug_selector`
plus `cx.debug_bounds` after `cx.draw` (`zed/crates/gpui/src/app/test_context.rs:893`).

**Split adapter tests from service tests.**
`docs/ARCHITECTURE.md:392-393` is the rule: "adapter tests assert exact argv; service tests
assert domain results and side-effect order". `crates/fleet-daemon/src/adapters/git.rs:486-500`
asserts `shell.calls()` equals an exact `Vec<FakeShellCall>`; a service test must never
assert argv. Adding an argv assertion to a service test couples it to the shell.

**Keep the real-`fleetd` suite thin and put behaviour in process.**
`crates/fleet-app/tests/common/mod.rs` spawns the actual binary into a temp `FLEET_HOME`
with RAII cleanup, and `make test` builds `fleet-daemon` first and exports `FLEET_DAEMON`
because of it (`Makefile:43-46`, `docs/DEVELOPMENT.md:55-63`). That suite proves the socket,
the handshake and the process; everything else belongs in a `tokio::io::duplex` test against
the router and the fakes.

**Every bug fix ships a test that fails on the parent commit.**
fleetd's convention is a dedicated file: `crates/fleet-client/tests/bugfix_connection.rs`,
`bugfix_terminal_spawn.rs`, `crates/fleet-git/tests/bugfix_{mutation,parse_patch,read_watch,rebase}.rs`.
Put the reproduction there, name it after the wrong behaviour it forbids, and reference the
symptom in the module doc.

**Name a test as a sentence stating the invariant.**
fleetd uses full-sentence names (`nudge_wakes_a_sleeping_link_and_restarts_backoff_from_the_floor`,
`crates/fleet-daemon/src/machines/link.rs:869`) — only 5 of ~2,160 test functions carry
Zed's `test_` prefix. Follow the local convention, not Zed's, and keep `test_random_*` for
the randomized ones so `nextest` filters can find them.

**Make randomness a first-class, replayable input.**
`#[gpui::test(iterations = 100)] fn test_random_x(mut rng: StdRng)` honours `SEED` and
`ITERATIONS` (`zed/crates/gpui_macros/src/gpui_macros.rs:171-188`); pair it with a reference model and
a `check_invariants()` as Zed does at `zed/crates/text/src/tests.rs:52`. fleetd has none yet —
`crates/fleet-core/src/board/sync/` (a pure reconciler) and `fleet-term` frame reassembly
are where it pays.

**`#[ignore]` always carries a reason; platform gating is `#[cfg]`, not a runtime return.**
All three of fleetd's uses do this already (`crates/fleet-daemon/src/machines/tailscale.rs:726`,
`crates/fleet-term/src/host/owner/tests.rs:648`, `crates/fleet-term/benches/viewport.rs:9`).
An ignored test with no reason is a deleted test with extra steps.

## Core patterns

Full catalog with Zed citations: `references/patterns.md`.

### 1. The GPUI test skeleton

```rust
#[gpui::test]
fn closing_the_dialog_restores_focus_to_the_body(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/fleet-test", std::time::Instant::now()));
    let window = cx.add_window(|_, cx| InputView { state: state.clone(), focus: cx.focus_handle() });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_keystrokes("escape");
    visual.update(|_, cx| state.read(cx).assert_body_focused());
}
```

`cx.add_window_view(build)` returns `(Entity<V>, &mut VisualTestContext)` in one call
(`zed/crates/gpui/src/app/test_context.rs:288`) and is worth adopting — fleetd uses it zero times.
See `references/patterns.md#gpui-test-skeleton`.

### 2. Virtual time for retries, timeouts and debounces

```rust
#[gpui::test]
fn attach_retries_after_the_backoff_elapses(cx: &mut TestAppContext) {
    // … arrange a surface whose attach was refused …
    cx.run_until_parked();
    cx.executor().advance_clock(ATTACH_RETRY_DELAY);
    cx.run_until_parked();
    assert!(surface.borrow().attach_retry_at.is_none());
}
```

Assert a timeout by stepping past its constant, never by waiting it out. Live example:
`crates/fleet-app/src/terminal/surface/tests.rs:78,96-98`.

### 3. Adapter test: exact argv against `FakeShell`

```rust
let shell = Arc::new(FakeShell::new());
shell.when(|command| command.program == "git", ShellResult { status: 0, ..Default::default() });
let git = ShellGit::new(shell.clone());
git.fetch(Path::new("/repo"), true).await.unwrap_or_else(|error| panic!("{error}"));
assert_eq!(
    shell.calls(),
    vec![FakeShellCall::Run(
        ShellCommand::new("git").args(["fetch", "--prune", "origin"]).cwd(Path::new("/repo")),
    )],
);
```

`FakeShell` is rule-driven and returns status 127 for unmatched commands, so an unexpected
invocation fails instead of silently succeeding (`crates/fleet-daemon/src/testing/fakes.rs:44-87`).
Adapted from `crates/fleet-daemon/src/adapters/git.rs:475-500`.

### 4. Service test: paused tokio clock

```rust
#[tokio::test(start_paused = true)]
async fn nudge_wakes_a_sleeping_link_and_restarts_backoff_from_the_floor() {
    let link = RemoteLink::new(provider, LinkOptions {
        backoff_min: Duration::from_secs(1),
        backoff_max: Duration::from_secs(60),
        hello_timeout: Duration::from_secs(1),
    });
    link.connect().await.expect_err("provider is unreachable");
    tokio::time::advance(Duration::from_secs(30)).await;
    // … assert the link retried at the floor, not at 30s …
}
```

Verbatim shape from `crates/fleet-daemon/src/machines/link.rs:868-880`. Note the injected
`LinkOptions`: a service that reads its own constants cannot be time-tested.

### 5. Textual projection instead of view internals

```rust
// One pure function per list/tree view; assert slices of it.
assert_eq!(board_screen::counts(app.board().unwrap(), "integration"), (1, 1));
```

Zed's `visible_entries_as_strings` (`zed/crates/project_panel/src/project_panel_tests.rs:10541`)
is the fuller form: walk the view model, return `Vec<String>`, compare against a literal
slice. fleetd has the pure-function half (`crates/fleet-app/tests/board_flow.rs:32,57-60`)
but no `*_as_strings` renderer yet — add one when you touch a list view.

### 6. Real-`fleetd` socket test, bounded and RAII

```rust
#[tokio::test]
async fn board_mutations_refresh_the_app_through_default_subscriptions() {
    let daemon = common::Daemon::start("board-flow")
        .expect("build fleet-daemon before running the board socket integration test");
    let writer = daemon.connect().await;
    let observer = Client::connect(daemon.home()).await.unwrap();
    // … exercise exactly the cross-process concern, then let Drop kill the child …
}
```

`crates/fleet-app/tests/board_flow.rs:15-21`; the fixture owns a `TempDir` + `Child` and
kills it in `Drop` (`crates/fleet-app/tests/common/mod.rs:20-55`,
`crates/fleet-daemon/tests/infra/mod.rs:45-52`). One such test per cross-process behaviour,
not per assertion.

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| `std::thread::sleep` / `smol::Timer::after` / unpaused `tokio::time::sleep` in a test | Not tracked by the test scheduler; `run_until_parked` reports "nothing left to run" while the work is still pending | `cx.run_until_parked()`, `cx.executor().advance_clock(D)`, `tokio::time::advance` under `start_paused` |
| `allow_parking()` to unstick a hang | Converts a missing waker or a real-I/O dependency into a 15 s timer (`zed/crates/scheduler/src/test_scheduler.rs:433`) | Put the dependency behind a trait and inject a fake |
| Bare `#[test]` around `cx.new(...)` | No seeded dispatcher, no leak detection, no teardown assertions | `#[gpui::test]` with the contexts in the signature |
| Polling a wall clock for a condition (`timeout` + `sleep` loop) | Slow, load-sensitive, hides the real ordering bug | `cx.condition(&entity, pred).await`, an awaited event, or `advance_clock` |
| Building a real tempdir tree for a service test | Couples domain logic to the OS; 20× slower; leaks on panic | `FakeFiles` + `FixedClock`; real FS only for `RealFiles`/git tests |
| Asserting argv inside a service test | Freezes an adapter's implementation into an unrelated test | Adapter test asserts argv; service test asserts domain result + side-effect order (`docs/ARCHITECTURE.md:392-393`) |
| Spawning the real `fleetd` to test in-process behaviour | Serial, socket-contended, needs `make test`'s prebuild step | In-process harness over `tokio::io::duplex` against `Router` + fakes |
| Reading private view fields to assert UI | Breaks on every refactor and proves nothing a user sees | A pure projection function or `*_as_strings(view, cx) -> Vec<String>` |
| Bare `#[ignore]` | Silently deleted coverage | `#[ignore = "requires nvim on PATH; run with --ignored"]` |
| Assertion helper without `#[track_caller]` | Failure points at the helper, not the failing test | `#[track_caller]` on every helper that asserts |

## fleetd-specific guidance

**Keep doing** (already at or above Zed's bar, do not preach it): `test-support` gating
`pub mod testing`; 15 adapter traits with matching fakes; byte-exact protocol goldens
(`crates/fleet-proto/tests/compatibility.rs`); `unwrap_or_else(|error| panic!("{error}"))`
instead of bare `unwrap()`; `#[ignore = "…"]` with a reason; RAII child-process cleanup.

**Where tests live.** Default to inline `#[cfg(test)] mod tests` (343 modules). When the
suite outgrows the file, use a sibling module — `crates/fleet-app/src/dialogs/input/tests.rs`,
`crates/fleet-daemon/src/services/sessions/tests.rs`, `crates/fleet-core/src/board/sync/tests/`.
Cross-crate or binary-launching suites go in `crates/<crate>/tests/`
(`crates/fleet-daemon/tests/` has 28 files, `crates/fleet-app/tests/` has 2 plus `common/`).
Never add a `mod.rs`.

**The 44 separate integration binaries are a real cost.** Each file under `crates/*/tests/`
links its own binary and there is not one `[[test]]` stanza in the workspace. When you add
a suite to `fleet-daemon`, prefer extending an existing file over adding a 29th; if you
consolidate, follow Zed's shape — `[[test]] name = "integration"`,
`required-features = ["test-support"]`, `path = "tests/integration/<crate>.rs"` with `mod`
declarations (`zed/crates/project/Cargo.toml:16-19`). Incremental, not a sweep.

**Only `fleet-daemon` exports a `test-support` feature** (`crates/fleet-daemon/Cargo.toml:16`).
`fleet-core`, `fleet-git` and `fleet-client` have none, so `fleet-app` tests cannot borrow
their fakes and reach for the live daemon instead. Add the feature to a crate the first time
a downstream test needs one of its fakes — `fleet-git`'s `Runner` is already injectable.

**Missing ergonomics, in priority order.** `pretty_assertions` and `indoc` are not workspace
dependencies (`Cargo.toml` has neither); add them to `[workspace.dependencies]` as dev-deps
when you next compare a `Vec<String>` or a terminal grid. `#[track_caller]` is on no helper
in `crates/fleet-app/src/state/test_support.rs`. There is no `.config/nextest.toml`; `make
test` runs `cargo test --workspace` (`Makefile:43-46`). If you add one, set a 60 s
`slow-timeout` and a `max-threads = 1` test group for the daemon-process suites, which
contend on sockets.

**Randomized coverage is absent** (0 `StdRng`, 0 `seeds(`). The two places it earns its
keep are `crates/fleet-core/src/board/sync/reconcile.rs` (pure function of local doc +
remote snapshot, ADR 0008 forbids I/O and clocks in it) and `fleet-term` frame reassembly.
Both already have a reference-model shape available.

**Running tests.** `make test` (builds `fleetd`, exports `FLEET_DAEMON`, then
`cargo test --workspace`), `make lint` (fmt-check + clippy `-D warnings`), `make check`.
For a single app crate you must prebuild the daemon yourself:
`cargo build -p fleet-daemon && FLEET_DAEMON="$PWD/target/debug/fleetd" cargo test -p fleet-app`
(`docs/DEVELOPMENT.md:60-63`). `fleet-daemon`'s own tests ignore `FLEET_DAEMON` and use
`CARGO_BIN_EXE_fleetd` (`crates/fleet-daemon/tests/infra/mod.rs:20`).

**Docs move with tests.** `docs/ARCHITECTURE.md:390-398` is the authority for the
adapter-vs-service split and for the recorded-capture agent tests; if a change makes that
paragraph wrong, fix it in the same commit. Commit style: `tests: <imperative lowercase
summary>`, or the owning crate's short name when the test ships with the fix
(`daemon:`, `app:`, `client:`).

## Review checklist

Full list with fixes: `references/checklist.md`.

1. Does every test touching an `Entity`, `Window` or executor use `#[gpui::test]`?
2. Is every wait a `run_until_parked` / `advance_clock` / awaited signal — no real sleep?
3. Is `allow_parking()` absent, and every tokio timing test `start_paused`?
4. Does a new external dependency arrive behind a trait with a `Fake*` under `test-support`?
5. Do service tests assert domain results, and adapter tests exact argv — not the reverse?
6. Is UI asserted through a projection function rather than private view fields?
7. Does the bug fix carry a test that fails on the parent commit, in `tests/bugfix_*.rs`?
8. Do assertion helpers carry `#[track_caller]`, and does `#[ignore]` carry a reason?
9. Does any new real-`fleetd` test earn the process, or could it run in-process?
10. Did the owning `docs/` section change in the same commit when the contract moved?

## Related skills

- `rust-async-background-work` — executors, `Task` lifetimes, the bridge's tokio thread; what
  you are making deterministic here.
- `rust-ipc-protocol` — protocol goldens, framing and handshake tests; owns
  `crates/fleet-proto/tests/compatibility.rs`.
- `gpui-state-and-memory` — entities, subscriptions and leaks, which the `#[gpui::test]`
  teardown checks assert.
- `gpui-app-shell` — actions, keymaps and focus, the surface `simulate_keystrokes` and
  `dispatch_action` drive.
- `gpui-components` — gallery examples as the presentation-coverage gate.
- `rust-workspace-architecture` — crate layering and where a `test-support` feature belongs.
- `zed-quality-review` — loads `references/checklist.md` alongside the other skills'.
