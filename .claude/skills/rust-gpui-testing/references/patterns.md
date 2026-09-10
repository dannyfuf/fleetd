# Testing patterns — Rust + GPUI in fleetd

Zed citations are `zed/crates/<crate>/src/<file>.rs:<line>` against tag **v1.18.1**
(`/Users/danny/.swarm/repos/zed-industries/zed`), the tag `Cargo.toml` pins GPUI to.
fleetd citations are repo-relative to `/Users/danny/.swarm/worktrees/dannyfuf/fleetd/chore-skills`.

Contents:
[1](#the-deterministic-scheduler) ·
[2](#gpui-test-skeleton) ·
[3](#injected-parameters-and-macro-arguments) ·
[4](#run_until_parked-the-universal-barrier) ·
[5](#virtual-time-advance_clock-and-executor-timers) ·
[6](#awaiting-entity-signals) ·
[7](#ui-behaviour-and-textual-projections) ·
[8](#pixel-level-assertions-the-narrow-exception) ·
[9](#fakes-at-the-trait-boundary) ·
[10](#test-constructors-and-init_test) ·
[11](#declarative-world-building) ·
[12](#tokio-daemon-tests-and-the-paused-clock) ·
[13](#adapter-vs-service-test-split) ·
[14](#real-fleetd-socket-tests) ·
[15](#randomized-tests) ·
[16](#two-peer-and-ipc-tests) ·
[17](#test-placement-and-cargo-wiring) ·
[18](#assertion-ergonomics) ·
[19](#regression-test-naming) ·
[20](#nextest-timeouts-and-ignore)

---

## The deterministic scheduler

`#[gpui::test]` expands to a plain `#[test]` whose body calls
`gpui::run_test(iterations, &[seeds], retries, &mut |dispatcher, seed| …)`
(`zed/crates/gpui/src/test.rs:95`). Each run builds a fresh `TestDispatcher::new(seed)`:

```rust
// zed/crates/gpui/src/platform/test/dispatcher.rs:23-33 (trimmed)
impl TestDispatcher {
    pub fn new(seed: u64) -> Self {
        let scheduler = Arc::new(TestScheduler::new(TestSchedulerConfig {
            seed,
            randomize_order: true,
            allow_parking: false,
            capture_pending_traces: std::env::var("PENDING_TRACES")
                .map_or(false, |var| var == "1" || var == "true"),
            timeout_ticks: 0..=1000,
        }));
        Self::from_scheduler(scheduler)
    }
```

Task interleaving, `rng()` and the clock all derive from that one seed, so a failing seed
replays the whole run. Teardown is the macro's, not yours: after the body it drops the
executor, runs the per-context teardowns and calls `dispatcher.drain_tasks()`
(`zed/crates/gpui_macros/src/test.rs:185-205`); the comment there names the leak it is
hunting ("task <-> entity cycles"). Leaked entities surface because the harness holds
`ref_counts_drop_handle()` (`zed/crates/gpui/src/app/entity_map.rs:88`).

Parking is a failure, not a wait. `dispatch_after` panics outright:

```rust
// zed/crates/gpui/src/platform/test/dispatcher.rs:132-134
fn dispatch_after(&self, _duration: Duration, _runnable: RunnableVariant) {
    panic!(
        "dispatch_after should not be called in tests. \
```

and blocking with no runnable task and no timer hits a hard kill after 15 s
(`zed/crates/scheduler/src/test_scheduler.rs:433-436`: *"Test timed out after 15 seconds
while parking. This may indicate a deadlock or missing waker."*).

## GPUI test skeleton

fleetd's canonical shape, from `crates/fleet-app/src/dialogs/input/tests.rs:24-50` (trimmed):

```rust
#[gpui::test]
fn shared_actions_edit_the_selected_buffer_and_preserve_clear_semantics(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/input", std::time::Instant::now()));
    let window = cx.add_window(|_, cx| InputView { state: state.clone(), focus: cx.focus_handle() });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |view, window, cx| window.focus(&view.focus, cx))
        .expect("focus input");
    visual.simulate_keystrokes("left backspace");
    visual.update(|_, cx| with_host(&state, cx, |host| assert_eq!(host.context.name.text(), "hélo")));
}
```

The test defines a minimal `impl Render` host (`tests.rs:4-22`) whose only job is to mount
the element under test with a `key_context` and `track_focus` — that is what makes
`simulate_keystrokes` resolve bindings through the real focus chain.

Entry points (`zed/crates/gpui/src/app/test_context.rs`):

| Call | Line | Returns |
| --- | ---: | --- |
| `cx.add_window(build)` | 220 | `WindowHandle<V>` |
| `cx.add_empty_window()` | 267 | `&mut VisualTestContext` |
| `cx.add_window_view(build)` | 288 | `(Entity<V>, &mut VisualTestContext)` |
| `VisualTestContext::from_window(handle, cx)` | 757 | `VisualTestContext` |

`add_window_view` collapses the fleetd four-line dance above into one call and is used
**zero** times in fleetd. Prefer it for new tests.

Three context flavours: `TestAppContext` (app + executors, no window), `VisualTestContext`
(`test_context.rs:738` — a *simulated* window: keystrokes, mouse, `draw`, `debug_bounds`),
and `VisualTestAppContext` (`zed/crates/gpui/src/app/visual_test_context.rs`) which opens a
**real** offscreen window and exists only for Zed's macOS screenshot tests. fleetd has no
use for the third.

## Injected parameters and macro arguments

The attribute inspects each parameter's type name to decide what to inject; anything else
is a compile error (`zed/crates/gpui_macros/src/test.rs:182`, `"invalid function signature"`):

```rust
// zed/crates/gpui_macros/src/test.rs:143-150 (trimmed)
Some("StdRng") => inner_fn_args.extend(quote!(rand::SeedableRng::seed_from_u64(_seed),)),
Some("BackgroundExecutor") => inner_fn_args
    .extend(quote!(gpui::BackgroundExecutor::new(std::sync::Arc::new(dispatcher.clone())),)),
```

Arguments, verbatim from `zed/crates/gpui_macros/src/gpui_macros.rs:171-177`:

> - `#[gpui::test]` with no arguments runs once with the seed `0` or `SEED` env var if set.
> - `#[gpui::test(seed = 10)]` runs once with the seed `10`.
> - `#[gpui::test(seeds(10, 20, 30))]` runs three times with seeds `10`, `20`, and `30`.
> - `#[gpui::test(iterations = 5)]` runs five times, providing as seed the values in the range `0..5`.
> - `#[gpui::test(retries = 3)]` runs up to four times if it fails to try and make it pass.
> - `#[gpui::test(on_failure = "crate::test::report_failure")]` will call the specified function after the tests fail…

Env knobs (`gpui_macros.rs:186-188`): `SEED` sets the first seed, `ITERATIONS` overrides the
`iterations` argument.

Multiple `&mut TestAppContext` parameters produce multiple apps sharing one dispatcher — the
mechanism behind Zed's two-client collab tests. `retries = N` exists but treat it as a bug
report, not a fix.

## `run_until_parked`, the universal barrier

```rust
// zed/crates/gpui/src/executor.rs:213-222 (doc comment trimmed)
/// Under the scheduler-backed test dispatcher, `tick()` will not advance the clock, so a pending
/// timer can keep `has_pending_tasks()` true even after all currently-runnable tasks have been
/// drained. … we advance the clock to the next timer when no runnable tasks remain.
pub fn run_until_parked(&self) {
    let scheduler = self.dispatcher.as_test().unwrap().scheduler();
    scheduler.run();
}
```

`dispatch_action` (`test_context.rs:481`), `simulate_keystrokes` (`:498`) and
`simulate_input` (`:514`) each call it for you, so an explicit pump after them is redundant.
fleetd uses it 106 times, e.g. `crates/fleet-lazygit/src/drive/support/mod.rs:302,309`.

Skip it only when you need to observe an intermediate state — then await a specific signal
(see [6](#awaiting-entity-signals)).

## Virtual time: `advance_clock` and executor timers

```rust
// zed/crates/gpui/src/executor.rs:200-204
/// In tests, move time forward. This does not run any tasks, but does make `timer`s ready.
#[cfg(any(test, feature = "test-support"))]
pub fn advance_clock(&self, duration: Duration) {
    self.dispatcher.as_test().unwrap().advance_clock(duration)
}
```

Always pair it with a pump. fleetd, `crates/fleet-app/src/terminal/surface/tests.rs:96-99`:

```rust
cx.run_until_parked();
cx.executor().advance_clock(ATTACH_TIMEOUT);
cx.run_until_parked();
assert!(surface.borrow().attached.is_none());
```

Zed's root `.rules` states the corollary explicitly:

> * In GPUI tests, prefer GPUI executor timers over `smol::Timer::after(...)` when you need
>   timeouts, delays, or to drive `run_until_parked()`… Avoid `smol::Timer::after(...)` for
>   test timeouts when you rely on `run_until_parked()`, because it may not be tracked by
>   GPUI's scheduler and can lead to "nothing left to run" when pumping.

fleetd complies: 0 `smol::Timer` anywhere, and its 22 `advance_clock` sites are all in
`fleet-app` / `fleet-lazygit`. The gap is coverage — extend it to every retry, backoff and
debounce constant, which means those constants must be injectable rather than read from a
`const` inside the unit under test.

`allow_parking()` (`executor.rs:226-234`) exists and even warns under
`GPUI_RUN_UNTIL_PARKED_LOG=1`. fleetd has zero uses; do not introduce the first one.

## Awaiting entity signals

Four helpers turn entity activity into futures/streams (`zed/crates/gpui/src/app/test_context.rs`):

| Helper | Line | Use |
| --- | ---: | --- |
| `cx.notifications(&entity)` | 546 | `Stream<Item = ()>` of `cx.notify()` |
| `cx.events(&entity)` | 566 | `UnboundedReceiver<Evt>` for an `EventEmitter` |
| `cx.condition(&entity, pred).await` | 586 | polls on notification, 3 s **virtual** timeout |
| `entity.next_event(cx).await` | 625 | one event |

```rust
// zed/crates/gpui/src/app/test_context.rs:583-592 (trimmed)
/// Runs until the given condition becomes true. (Prefer `run_until_parked` if you
/// don't need to jump in at a specific time).
pub async fn condition<T: 'static>(
    &mut self,
    entity: &Entity<T>,
    mut predicate: impl FnMut(&mut T, &mut Context<T>) -> bool,
) {
    let timer = self.executor().timer(Duration::from_secs(3));
    let mut notifications = self.notifications(entity);
```

The doc comment is the rule: `run_until_parked` first, `condition` only when you must
inspect a mid-flight state. fleetd uses none of these four — a `Bridge`-driven flow that
currently pumps and re-asserts can often say what it means with `cx.condition`.

`start_waiting()` / `finish_waiting()` from older Zed lore **do not exist at v1.18.1**
(0 hits). Do not port that idiom.

## UI: behaviour and textual projections

Zed's model, `zed/crates/project_panel/src/project_panel_tests.rs` (`visible_entries_as_strings`
defined at `:10541`):

```rust
let cx = &mut VisualTestContext::from_window(window.into(), cx);
let panel = workspace.update_in(cx, ProjectPanel::new);
cx.run_until_parked();
assert_eq!(
    visible_entries_as_strings(&panel, 0..50, cx),
    &["v root1", "    > .git", "    > a", "      .dockerignore", "v root2"],
);
```

The helper walks the *view model*, not the pixels. `crates/editor` does the same with marked
text: `cx.set_state("abˇc")` (`zed/crates/editor/src/test/editor_test_context.rs:389`) and
`cx.assert_editor_state("aˇbc")` (`:609`), built on
`zed/crates/util/src/test/marked_text.rs:113`.

fleetd has the pure-projection half but no `*_as_strings` renderer. Its shape today
(`crates/fleet-app/tests/board_flow.rs:32,57-60`):

```rust
assert_eq!(board_screen::counts(app.board().unwrap(), ""), (0, 0));
// … after a card is created and the board view re-fetched …
assert_eq!(board_screen::counts(app.board().unwrap(), "integration"), (1, 1));
```

**Recommended increment.** When you next touch a list or tree view in `fleet-app` or
`fleet-lazygit`, add one `fn <view>_as_strings(props, cx) -> Vec<String>` next to the render
function, feed it the same borrowed `Props` the render takes, and assert against a literal
slice. It costs nothing at runtime (test-only or `pub(crate)`) and turns a 12-line field
assertion into a readable diff.

Keystroke/action entry points: `visual.simulate_keystrokes("cmd-shift-p enter")`,
`visual.dispatch_action(SomeAction)` (`test_context.rs:770,794`). fleetd has 23 and 44 of
them. New dialogs should get `key_context` coverage the same way — that is what proves a
binding is reachable through the focus chain rather than merely declared in `keymap.rs`.

## Pixel-level assertions: the narrow exception

Only where geometry *is* the behaviour:

```rust
// zed/crates/gpui/src/elements/div.rs:5177-5200 (trimmed)
div().debug_selector(move || format!("cell-{index}")).w(width).h(px(10.))
// …
cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx)).unwrap();
let bounds = |selector| window.rendered_frame.debug_bounds.get(selector).copied();
assert_eq!(bounds("cell-0").origin.x, px(0.));
```

`cx.draw(origin, space, |window, cx| element)` (`zed/crates/gpui/src/app/test_context.rs:893`)
draws a single element for unit tests and is **required** before `simulate_event` (the
assertion message says so: *"Make sure you've called `VisualTestContext::draw` first!"*).

fleetd's terminal grid is the one place this earns its keep: cell geometry is computed in
`crates/fleet-app/src/terminal/geometry.rs` and painted by
`crates/fleet-ui-kit/src/components/terminal_grid/painter.rs`. Screenshot diffing has no
place here — Zed's own image baselines are `#[ignore]`d and gitignored
(`zed/crates/zed/src/zed/visual_tests.rs:427,439`).

## Fakes at the trait boundary

The rule: the fake implements the **production trait**, lives in the **production crate**,
and is gated by `#[cfg(any(test, feature = "test-support"))]` so downstream crates enable it
as a dev-dependency feature. Zed has 433 such gates; `FakeFs: Fs`
(`zed/crates/fs/src/fs.rs:1398` (trait impl `:2855`)) is the flagship, and `Fs::as_fake()` is a trait method that
panics on the real impl (`zed/crates/fs/src/fs.rs:178-181`) so any layer holding an
`Arc<dyn Fs>` can reach the fake.

fleetd already does this well:

```rust
// crates/fleet-daemon/src/lib.rs:3-11 (trimmed)
pub mod adapters;
pub mod services;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
```

```toml
# crates/fleet-daemon/Cargo.toml:15-16, 42-45
[features]
test-support = []

[dev-dependencies]
fleet-daemon = { workspace = true, features = ["test-support"] }
tempfile.workspace = true
tokio = { workspace = true, features = ["test-util"] }
```

`crates/fleet-daemon/src/testing/`: `fakes.rs` (`FakeShell:48`, `FakeProcess:143`,
`FixedClock:210`, `FakeBackend:252`, plus `type FakeGit = ShellGit<FakeShell>` and
`type FakeGithub = GhCli<FakeShell>` — the *production* adapters over a fake shell, which is
the strongest form of this pattern), `files.rs` (`FakeFiles`), `machines.rs`
(`FakeMachine`, `FakeRemote`).

`FakeShell` is rule-driven with an ordered call log and a hostile default:

```rust
// crates/fleet-daemon/src/testing/fakes.rs:60-87 (trimmed)
pub fn when<F>(&self, predicate: F, result: ShellResult)
where F: Fn(&ShellCommand) -> bool + Send + Sync + 'static { … }

pub fn calls(&self) -> Vec<FakeShellCall> { lock(&self.calls).clone() }

fn matching(&self, command: &ShellCommand) -> ShellResult {
    lock(&self.rules).iter().find(|rule| (rule.predicate)(command))
        .map(|rule| rule.result.clone())
        .unwrap_or_else(|| ShellResult { status: 127, stdout: String::new(),
            stderr: format!("unmatched fake command: {}", command.program) })
}
```

Status 127 for an unmatched command is the important detail: a call the test did not
anticipate fails rather than silently succeeding. Copy that when you write a new fake.

`FixedClock` (`fakes.rs:210-232`) is a `Mutex<DateTime<Utc>>` with `new` and `set`, behind
the `Clock` trait (`crates/fleet-daemon/src/adapters/clock.rs:6-14`). Every service that
stamps a timestamp takes `Arc<dyn Clock>`; none should call `Utc::now()` directly.

**Gap.** Only `fleet-daemon` declares a `test-support` feature. `fleet-core`, `fleet-git`
and `fleet-client` do not, so `fleet-app` tests cannot borrow their fakes and reach for a
live daemon instead. `fleet-git`'s `Runner` is already injectable
(`crates/fleet-git/tests/support/mod.rs:20`), so that crate is one `[features]` stanza away.

**Fake↔real parity.** A fake that drifts is worse than none. Zed pins it with
`zed/crates/fs/tests/integration/fake_git_repo_tests.rs`. fleetd's equivalent lever is that
`FakeGit`/`FakeGithub` are the *real* adapters over `FakeShell`, so only the shell can drift.

## `::test(...)` constructors and `init_test`

Zed gives every heavyweight aggregate a feature-gated test constructor:

```rust
// zed/crates/project/src/project.rs:2086-2093 (trimmed)
#[cfg(feature = "test-support")]
pub async fn test(fs: Arc<dyn Fs>, root_paths: impl IntoIterator<Item = &Path>,
                  cx: &mut gpui::TestAppContext) -> Entity<Project> {
    Self::test_project(fs, root_paths, false, cx).await
}
```

and starts each test file with a crate-local `init_test(cx)` (135 definitions across the
tree) that installs the globals a window needs.

fleetd has neither. Every GPUI test repeats:

```rust
// crates/fleet-app/src/dialogs/input/tests.rs:28-31
cx.update(|cx| {
    cx.set_global(fleet_ui_kit::Theme::dark());
    crate::keymap::init(cx);
});
```

**Recommended increment.** Add `pub(crate) fn init_test(cx: &mut TestAppContext)` to
`crates/fleet-app/src/state/test_support.rs` (which already exists and holds frame/session
builders) and call it from new tests; migrate old ones as you touch them. A real app boot
does more than those two lines (`crates/fleet-app/src/shell/root/bootstrap.rs:44-58` also
calls `fleet_lazygit::keymap::init`), and a shared `init_test` is where that stays in sync.

Caveat (`[J]`): do not add a `::test()` that hides construction logic you meant to cover —
it becomes an untested path.

## Declarative world building

Zed builds fixtures declaratively rather than by mutating a temp dir:

```rust
// zed/crates/project/tests/integration/project_tests.rs (trimmed)
let fs = FakeFs::new(cx.executor());
fs.insert_tree(path!("/dir"), json!({
    ".zed": { "settings.json": r#"{ "tab_size": 8 }"# },
    "a": { "a.rs": "fn a() {\n    A\n}" },
})).await;
let project = Project::test(fs.clone(), [path!("/dir").as_ref()], cx).await;
```

`insert_tree` is at `zed/crates/fs/src/fs.rs:1986`; `FakeFs` also drives git state
(`set_head_for_repo` at `:2323`) and watcher behaviour (`pause_events` `:1920`,
`flush_events` `:1962`).

fleetd builds trees imperatively:

```rust
// crates/fleet-daemon/src/services/sessions/tests.rs:7-13
let temp = tempfile::tempdir().expect("temp home");
let home = temp.path();
let repos = home.join("repos");
let worktrees = home.join("worktrees");
let worktree_path = worktrees.join("owner/repo/feature");
std::fs::create_dir_all(&repos).expect("repos directory");
std::fs::create_dir_all(&worktree_path).expect("worktree directory");
```

**Recommended increment.** Add `FakeFiles::insert_tree(path, serde_json::Value)` mirroring
`zed/crates/fs/src/fs.rs:1986` in `crates/fleet-daemon/src/testing/files.rs`, and default
service tests to `FakeFiles` + `FixedClock`. Keep `RealFiles` + `tempfile` for tests of
`RealFiles` itself and for git, where the subject *is* the filesystem — Zed keeps the same
carve-out with `util::test::TempTree`. `crates/fleet-git/tests/support/mod.rs` (`TestRepo`)
is already the right shape for the git case; an `insert_tree`-style convenience on it would
shorten most of `crates/fleet-git/tests/repository.rs`.

fleetd is macOS-only, so Zed's `path!()` normalization has no analogue to adopt.

## tokio daemon tests and the paused clock

476 `#[tokio::test]` across the workspace (331 in `fleet-daemon`). The dependency is already
there: `tokio = { workspace = true, features = ["test-util"] }` in
`crates/fleet-daemon/Cargo.toml:45`, which is what `start_paused` requires.

The one existing use, `crates/fleet-daemon/src/machines/link.rs:868-880` (trimmed):

```rust
#[tokio::test(start_paused = true)]
async fn nudge_wakes_a_sleeping_link_and_restarts_backoff_from_the_floor() {
    let provider = Arc::new(UnreachableProvider::new());
    let link = RemoteLink::new(
        Arc::clone(&provider) as Arc<dyn MachineProvider>,
        LinkOptions {
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            hello_timeout: Duration::from_secs(1),
        },
    );
    link.connect().await.expect_err("provider is unreachable");
```

Two things make it work and are the transferable lesson: the backoff bounds are **injected**
through `LinkOptions`, and the provider is a fake that always fails. Under `start_paused`
the runtime auto-advances when every task is idle, and `tokio::time::advance(d).await` steps
it explicitly.

**Gap.** 171 `tokio::time::timeout` and 58 `sleep` calls sit in test code, mostly as
"wait for the daemon to notice". Under `start_paused` a `timeout` becomes instant and a
`sleep` becomes a step; without it they are wall-clock waits that flake under load. Convert
suites as you touch them, keeping real time only where a real child process is involved
(see [14](#real-fleetd-socket-tests)).

`fleet-core` and `fleet-proto` have zero `#[tokio::test]` and zero `#[gpui::test]` by design
— they are pure. Keep it that way: a pure crate that needs a runtime in tests has grown an
I/O dependency.

## Adapter vs service test split

`docs/ARCHITECTURE.md:392-393`, verbatim:

> Ports/adapters with fakes as in swarm §8: adapter tests assert exact argv; service tests
> assert domain results and side-effect order; core helpers are pure-tested; proto has
> round-trip tests…

Adapter side — `crates/fleet-daemon/src/adapters/git.rs:475-500` (trimmed):

```rust
let git = ShellGit::new(shell.clone());
let cwd = Path::new("/repo");
git.fetch(cwd, true).await.unwrap_or_else(|error| panic!("{error}"));
git.fetch_pull_request(cwd, 42).await.unwrap_or_else(|error| panic!("{error}"));
assert_eq!(
    shell.calls(),
    vec![
        FakeShellCall::Run(ShellCommand::new("git").args(["fetch", "--prune", "origin"]).cwd(cwd)),
        FakeShellCall::Run(ShellCommand::new("git")
            .args(["fetch", "origin", "+refs/pull/42/head:refs/swarm/pulls/42/head"]).cwd(cwd)),
    ],
);
```

Note `unwrap_or_else(|error| panic!("{error}"))` rather than `unwrap()` — the fleetd house
style, so a failure prints the error instead of `Err(..)`.

Service side asserts the domain outcome and the *order* of side effects, never argv. The
agent providers go further and replay recorded captures
(`crates/fleet-daemon/tests/fixtures/agents/`) through the reducer, asserting thread state
and attention with no live provider and no window (`docs/ARCHITECTURE.md:395-398`).

`fleet-core`'s reducer (`ThreadProjection::apply`) and `board::sync` are pure functions and
are tested as such — 195 plain `#[test]`s and a dedicated
`crates/fleet-core/src/board/sync/tests/` directory (`apply.rs`, `push.rs`, `reconcile.rs`,
`schema.rs`). That is correct; do not wrap them in a runtime.

## Real-`fleetd` socket tests

`crates/fleet-app/tests/common/mod.rs` is the fixture. It creates an isolated `FLEET_HOME`,
overrides child `PATH` and `HOME` (so a stray `gh` on the developer's machine cannot leak
in), and cleans up through `Drop`:

```rust
// crates/fleet-app/tests/common/mod.rs:26-55 (trimmed)
pub fn start(label: &str) -> Result<Self> {
    let executable = fleetd_path()
        .context("fleetd is missing; run cargo build -p fleet-daemon or set FLEET_DAEMON")?;
    let home = tempfile::Builder::new().prefix(&format!("fleet-app-{label}-")).tempdir()?;
    let child = Command::new(&executable)
        .arg("--home").arg(home.path())
        .env("PATH", path).env("HOME", home.path())
        .stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(errors))
        .spawn()?;
    Ok(Self { home, child })
}
```

The daemon-side twin, `crates/fleet-daemon/tests/infra/mod.rs:19-52`, uses
`env!("CARGO_BIN_EXE_fleetd")` (so both halves of a loopback link are the same build) and
kills the child in `Drop`:

```rust
// crates/fleet-daemon/tests/infra/mod.rs:45-52
impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
```

Build coupling, from `Makefile:43-46`:

```make
test: ## Run workspace tests
	# App socket tests launch target/debug/fleetd; cargo test only builds its test harness.
	cargo build -p fleet-daemon
	FLEET_DAEMON="$(abspath $(CARGO_TARGET_DIR))/debug/fleetd" cargo test --workspace
```

`docs/DEVELOPMENT.md:55-63` spells out the consequence: run `cargo build -p fleet-daemon`
before `cargo test -p fleet-app`, or you test an older binary.

**What belongs here.** Exactly the cross-process concerns: the socket, the handshake, server
event filtering, default client subscriptions, daemon restart. `board_flow.rs` is a good
example — it proves that `Event::BoardChanged` survives *server filtering and default client
subscriptions* (`crates/fleet-app/tests/board_flow.rs:15-52`), which no in-process test can
prove.

**What does not.** Everything that is really about the router, a service, or a view.
`fleet-client` already decodes into `serde_json::Value` over any `AsyncRead + AsyncWrite`
(`crates/fleet-client/src/connection.rs`), and `MachineProvider::open_stream` yields an
`AsyncDuplex` — so a `tokio::io::duplex` harness wiring a real `Router` to fakes is
available today and is deterministic, fast and parallel-safe.

## Randomized tests

Zed's operations-based shape (`zed/crates/text/src/tests.rs:51-110`, trimmed):

```rust
#[gpui::test(iterations = 100)]
fn test_random_edits(mut rng: StdRng) {
    let operations = env::var("OPERATIONS").map(|i| i.parse().unwrap()).unwrap_or(10);
    let mut reference_string = RandomCharIter::new(&mut rng).take(len).collect::<String>();
    let mut buffer = Buffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), reference_string.clone());
    for _ in 0..operations {
        let (edits, _) = buffer.randomly_edit(&mut rng, 5);
        for (old_range, new_text) in edits.iter().rev() {
            reference_string.replace_range(old_range.clone(), new_text);
        }
        assert_eq!(buffer.text(), reference_string);
        buffer.check_invariants();
    }
}
```

Four conventions worth copying: a **reference model** the system is compared against, a
`check_invariants()` method on the data structure, `OPERATIONS` as an env knob beside `SEED`
and `ITERATIONS`, and `log::info!` per step so a failing seed replays readably.

For multi-peer plans Zed defines a `RandomizedTest` trait whose `Operation` is `Serialize`,
dumps a failing plan to JSON via `#[gpui::test(iterations = 100, on_failure = "…")]`, and
shrinks it with a delta-debugging script.

`#[gpui::property_test]` (proptest-backed) is documented at
`zed/crates/gpui_macros/src/gpui_macros.rs:207-273` and is preferred when inputs can be
generated up front, because it shrinks. `StdRng` parameters there are
*"**explicitly forbidden**, since they break shrinking, and are a common footgun"* (`:241-243`).
There are no in-tree call sites at v1.18.1, so treat it as unproven.

**fleetd has zero randomized tests** (0 `StdRng`, 0 `seeds(`, 2 `iterations =`). The two
places worth the investment:

- `crates/fleet-core/src/board/sync/reconcile.rs` — a pure function of (local doc, remote
  snapshot) that ADR 0008 forbids from doing I/O or reading a clock. Perfect for a random
  operation stream plus a naive reference reconciler.
- `fleet-term` frame reassembly — `FrameUpdate` has a documented apply order (shift before
  row replacement) and a `seq`; random dirty-row streams against a full-repaint reference
  grid would pin it.

Both are `#[test]`-only crates today, so the first would use `#[gpui::test(iterations = 50)]`
purely for the seeded `StdRng` injection, or hand-roll the seed loop as
`zed/crates/sum_tree/src/sum_tree.rs` does for a non-GPUI test.

## Two-peer and IPC tests

Zed: one `TestServer::start(executor)` (`zed/crates/collab/tests/integration/test_server.rs:89`),
N clients each with its own `TestAppContext`, the seed interleaving them:

```rust
#[gpui::test]
async fn test_channel_guests(executor: BackgroundExecutor, cx_a: &mut TestAppContext, cx_b: &mut TestAppContext) {
    let mut server = TestServer::start(executor.clone()).await;
    let client_a = server.create_client(cx_a, "user_a").await;
    let client_b = server.create_client(cx_b, "user_b").await;
    executor.run_until_parked();
    assert_eq!(project_b.read_with(cx_b, |p, _| p.remote_id()), Some(project_id));
}
```

`TestServer` also exposes `simulate_long_connection_interruption(…)` and tears down its
resources in `impl Drop`.

fleetd's analogue is real: `crates/fleet-daemon/tests/infra/mod.rs` has `RemoteDaemon`
(a second daemon plus a loopback `CommandMachine` and `HostConfigEntry`, `:56-59`), used by
`remote_link.rs`, `remote_mirror.rs`, `remote_sessions.rs`, `remote_lifecycle.rs`. That is
the correct place to prove protocol lockstep (ADR 0011: the remote link opens `Hello` as
`ClientKind::Proxy` and incompatible versions fail the handshake).

Behavioural two-peer coverage — routing, id remapping, fanout partitioning — should instead
run in process over `tokio::io::duplex`, because the seam already exists
(`MachineProvider::open_stream` → `AsyncDuplex`).

## Test placement and Cargo wiring

fleetd's three shapes, in order of preference:

| Shape | fleetd usage | Use when |
| --- | --- | --- |
| inline `#[cfg(test)] mod tests` | 343 modules | default |
| sibling `tests.rs` / `tests/` module | `crates/fleet-app/src/dialogs/input/tests.rs`, `crates/fleet-daemon/src/services/sessions/tests.rs`, `crates/fleet-core/src/board/sync/tests/` | the suite outgrew the file, or needs `use super::*` privates |
| `crates/<crate>/tests/*.rs` | 44 files (28 in `fleet-daemon`) | cross-crate wiring, or a test that launches a binary |

Never create `mod.rs` for a *module* — but note `tests/common/mod.rs` and `tests/infra/mod.rs`
are the Cargo-mandated form for a shared integration helper (a bare `tests/common.rs` would
be compiled as its own test binary), and both already exist.

Zed consolidates integration tests into **one** binary per crate:

```toml
# zed/crates/project/Cargo.toml:13-19
test = false

[[test]]
name = "integration"
required-features = ["test-support"]
path = "tests/integration/project_tests.rs"
```

with `mod` declarations inside, and 5 crates additionally set `[lib] test = false` so
`cargo test -p <crate>` cannot silently skip the `test-support`-gated suite.

fleetd has **no `[[test]]` stanza anywhere**: 44 separate integration binaries, each linked
independently. At this file count that is a measurable share of test build time.
**Incremental fix**: prefer extending an existing file over adding another; when a crate's
`tests/` grows past ~30 files, consolidate that crate alone into
`tests/integration/<crate>.rs` with `mod` declarations and
`required-features = ["test-support"]`. Do not sweep the workspace in one commit.

## Assertion ergonomics

Zed's toolkit, none of which fleetd has yet:

- `use pretty_assertions::{assert_eq, assert_matches};` at the top of 140 files, shadowing
  the std macros wherever a `Vec<String>` or a multi-line string is compared.
- `indoc!` (3,731 uses) / `unindent` (1,367) for text-shaped fixtures.
- `#[track_caller]` on every assertion helper, so a failure points at the test rather than
  the helper (`zed/crates/editor/src/test/editor_test_context.rs:388,608`).
- `assert_set_eq!` (`zed/crates/util/src/test/assertions.rs:44`) for order-insensitive
  collection comparison, and domain macros like `assert_hunks!`.

fleetd has 0 `pretty_assertions`, 0 `indoc`, and no `#[track_caller]` in
`crates/fleet-app/src/state/test_support.rs`. **Recommended increment**: add both crates to
`[workspace.dependencies]` (every fleetd dependency is declared there and inherited with
`.workspace = true`) and use them where they pay — terminal-grid fixtures, markdown
rendering, board projections. Add `#[track_caller]` to every helper that asserts, starting
with `state/test_support.rs`; it is a one-line, zero-risk change.

Keep fleetd's own good habit: `unwrap_or_else(|error| panic!("{error}"))` over bare
`unwrap()` in tests, so the message survives.

## Regression test naming

`crates/*/tests/bugfix_*.rs` is a live fleetd convention:

- `crates/fleet-client/tests/bugfix_connection.rs`
- `crates/fleet-client/tests/bugfix_terminal_spawn.rs`
- `crates/fleet-git/tests/bugfix_mutation.rs`
- `crates/fleet-git/tests/bugfix_parse_patch.rs`
- `crates/fleet-git/tests/bugfix_read_watch.rs`
- `crates/fleet-git/tests/bugfix_rebase.rs`

Rules for a new one: it must fail on the parent commit; its name is a sentence stating the
behaviour that is now forbidden; the module `//!` doc names the symptom a user saw. Group by
area, not per bug — add to `bugfix_connection.rs` rather than creating
`bugfix_connection_2.rs`.

Test naming generally: fleetd uses full-sentence names, not Zed's `test_` prefix — only 5 of
~2,160 test functions carry it (e.g.
`proxied_ensure_uses_executing_daemon_layout_and_only_degrades_lazygit`,
`crates/fleet-daemon/src/services/sessions/tests.rs:6`). Follow the local convention. The one
exception is randomized tests: keep `test_random_*` so a `nextest` filter can target them.

## nextest, timeouts and `#[ignore]`

Zed's policy file, `zed/.config/nextest.toml`:

```toml
[profile.default]
slow-timeout = { period = "60s", terminate-after = 1 }

[test-groups]
sequential-db-tests = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'package(db)'
test-group = 'sequential-db-tests'

[[profile.default.overrides]]           # explicit, named exemptions
filter = 'test(test_rainbow_bracket_highlights) or test(test_basic_following) or …'
slow-timeout = { period = "300s", terminate-after = 1 }
```

CI runs `cargo nextest run --workspace --no-fail-fast --failure-output immediate-final`, and
Zed's docs recommend nextest even locally because plain `cargo test` hits
`Too many open files (os error 24)` on macOS.

fleetd has no `.config/nextest.toml`, no `.github/`, and `make test` runs
`cargo test --workspace` (`Makefile:43-46`); `make ci` = `lint test test-scripts` but nothing
runs it automatically. **Recommended increment**, in this order:

1. Add `.config/nextest.toml` with a 60 s `slow-timeout, terminate-after = 1`.
2. Put the binary-launching suites in a `max-threads = 1` group —
   `package(fleet-app) and test(board_flow)`, `package(fleet-daemon) and test(remote_*)`,
   `test(server_process)` — they contend on sockets and on a single `FLEET_HOME` layout.
3. Switch `make test`/`make ci` to `cargo nextest run --workspace --no-fail-fast`, keeping
   the `cargo build -p fleet-daemon` + `FLEET_DAEMON` preamble.

`#[ignore]` policy: rare, and always with a reason string. All three fleetd uses comply:

```rust
// crates/fleet-daemon/src/machines/tailscale.rs:726
#[ignore = "requires a real Tailscale peer named by FLEET_TAILSCALE_HOST"]
// crates/fleet-term/src/host/owner/tests.rs:648
#[ignore = "requires nvim on PATH; run explicitly with --ignored"]
// crates/fleet-term/benches/viewport.rs:9
#[ignore = "manual timing benchmark; run with cargo test -p fleet-term --bench viewport -- --ignored --nocapture"]
```

Zed's 16 uses read the same way. Platform gating uses `#[cfg(target_os = "…")]` on the test,
not a runtime early return — fleetd is macOS-only so this rarely arises, but
`scripts/tests/bootstrap-zig-test.sh` is guarded that way in the `Makefile` (`test-scripts`).
