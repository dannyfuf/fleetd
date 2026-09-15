# Test-harness review fixes — Contracts (authoritative for the parallel build)

> Roadmap: ./harness-review-fixes-2026-09-14-roadmap.md · Issues: ./issues.md
> This document fixes every cross-file decision and signature so that one agent per file can
> implement its share against the same shapes. It is **authoritative** for the build: an agent
> that needs a different shape records a DEVIATION in its report; it does not change the shape.

## 0. Rules every agent works under

- Repo: `/home/df/.fleet/worktrees/dannyfuf/fleetd/test-harness`, branch `test-harness`.
- **No git operations** (no add/commit/stash/checkout/branch). The orchestrator commits.
- **No `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` or `Makefile` edits.** `P4-T12` in
  particular is doc-only by decision.
- **Edit only the files you own** (§6 ownership table). Anything you need elsewhere goes in your
  report under `INTEGRATION NOTES` as an exact, copy-pasteable edit (file, location, code).
- Never reformat, rewrite or "tidy" a file you do not own. Format only your own files with
  `rustfmt --edition 2024 <file>`. During the Implement stage never run `cargo fmt --all`,
  `make lint`, `make test` or `make harness`: other agents are editing the tree concurrently.
- Verification you *may* run: `cargo check -p <crate> --all-targets`,
  `cargo clippy -p <crate> --all-targets --all-features -- -D warnings`,
  `cargo test -p <crate> <filter>`. A compile error inside a file you do not own is another
  agent mid-edit: wait 60–120 s and retry, up to five times; if it persists, say so in your report
  and deliver your file with the best verification you could get.
- `cargo test -p fleet-app` socket tests and the harness's headless subset spawn binaries; if a
  test you run needs them, set `FLEET_DAEMON`, `FLEET_APP`, `FLEET_HARNESS_BIN` to
  `target/debug/{fleetd,fleet,fleet-harness}` after `cargo build -p fleet-daemon -p fleet-app -p fleet-harness`.
- Repo non-negotiables (`CLAUDE.md`): no `unwrap`, `todo!`, `unimplemented!`, `dbg!`, `TODO` in
  production code; `expect` only for static invariants with a message; never `let _ =` a
  fallible call (use `?`, log, or match; a fire-and-forget channel send needs a comment saying the
  receiver is gone only during shutdown); every spawned fallible task ends in
  `.detach_and_log_err(cx)` or is stored; render prepares nothing; `docs/` is authoritative.
- Read the project skill for your area before editing (`.claude/skills/<name>/SKILL.md`):
  `gpui-state-and-memory` + `rust-async-background-work` for `fleet-app` state/bridge/shell work,
  `gpui-performance` for `projection.rs`, `rust-gpui-testing` for every test,
  `rust-workspace-architecture` for error handling and file layout.
- Every latent fix carries a unit test written with it. Tests go in an inline
  `#[cfg(test)] mod tests` (or `mod <topic>_tests`) at the bottom of the file you own. The shared
  helpers in `state/harness/tests.rs` and `agent/tests.rs` are `pub(super)` after the Contracts
  stage, so `use super::tests::{...}` / `use crate::agent::tests::{...}` works from a sibling.
- Report format, ≤ 40 lines, exactly these headings in this order:
  `STATUS: GREEN|RED|BLOCKED — <why>` / `FILES:` / `TESTS:` (names + the command you ran and its
  last summary line) / `DEVIATIONS:` / `INTEGRATION NOTES:` / `DECISIONS:` (anything a tracker's
  decisions log asks for) / `FOLLOW-UPS:`.

## 1. Decisions (do not re-litigate)

| Issue | Decision |
| --- | --- |
| I3 | Two mechanisms. `Bridge::send` (mutation lane): a **settle counter** — the claim moves from `in_flight_requests` into `settling_mutations` when the daemon replies, and is released when the shell applies the next `SnapshotChanged`/`Connected`, or after `MUTATION_SETTLE_GRACE = 250 ms` (5 × the daemon's 50 ms coalesce window) if no snapshot follows. `Bridge::request` (reply lane): the `InFlight` claim is held until the consumer has taken the reply, via `async_channel::Sender::closed()` on the reply sender (async-channel 2.5.0 has it). No new wire fields. F1-11 is not implemented. |
| I11 | Every busy→idle edge **nudges** through the existing bridge event channel: `InFlight`, `ArmedDebounce` and the settle expiry each call an `IdleWake` on release; the wake is `try_send(BridgeEvent::Nudge)`; the shell drain loop turns a `Nudge` into `cx.notify()` when `fleet_ui_kit::harness::is_recording()`, and ignores it otherwise. `HarnessState::finish_request`/`begin_request` are removed if they have no production caller (tests use the guard API instead). |
| I12 | **Code.** `IdleSnapshot` gains `link_opening: bool` = `matches!(state.daemon, DaemonLink::Starting)`. Only `Starting` counts: `Lost`/`Failed` are states a scenario asserts on, not busy-ness. |
| I2 | After a `quit` line the runner awaits Fleet's exit under the existing `QUIT_GRACE` (5 s, same order as `DEFAULT_AWAIT_TIMEOUT_MS`); non-success, signal, or timeout fails the step naming the status. `stop()` returns the `ExitStatus` so teardown can fail on it. |
| I17 | The `["idle", _]` arm matches only when the second token parses as a number; `exists`/`absent` are checked first. |
| I4 | Mirror **index + filter**, not `JobId`: `AppState::jobs_panel: JobsPanelMirror { cursor: usize, filter: JobFilter }`. `JobFilter` moves from `views/jobs_panel.rs` to `state/jobs_filter.rs` (re-exported from its old path). `Cursors::jobs` and its clamp are deleted. |
| I5 | **Doc.** `dialog.fields`/`dialog.message` stay empty (dialog content lives in the dialog host entity; a mirror would be a second source of truth). §11 gets an entry; `projection.rs` gets a rot test asserting they are empty for an open dialog. `buttons` untouched. |
| I28 | **Doc.** §11 entry naming `window.frame` specifically; `drive.rs`'s `wait_for` doc comment corrected. No paint per iteration. |
| I31 | `renamed_terminals` added to `ProjectionKey`; one line plus a test. |
| I7 | Deadline on the **gate wait only**: `Peer::read_frame_within(GATE_BUDGET)`; `GATE_BUDGET = Duration::from_millis(DEFAULT_AWAIT_TIMEOUT_MS)`-equivalent = 5 s, defined in `agent.rs`. The between-turns dispatch loop keeps the unbounded `read_frame`. |
| I22 | One shared id namespace: a `tool_call` id may not equal any gate id or any other `tool_call` id. |
| I18 | Wire a third scripted agent carrying `error-mid-stream.json` **if** the fixture can serve a third transcript without a new `AgentKind` (e.g. the launcher shim selecting a transcript by an env var or working directory); otherwise amend §5 to "two" and update the `.blocked` diagnosis. The owner of `fixture/plan.rs` decides and reports. |
| I14 | `std::fs::create_dir` (not `create_dir_all`) and retry with a `-2`, `-3`… suffix on `AlreadyExists`; a run that never claimed a directory unlinks nothing. |
| I15 | Hold the seal across `restart()`: spawn the replacement, then unseal/re-seal so `Daemon::adopt`'s guarantee holds literally. |
| I25 | New `BaselineOutcome::NotRecorded { lane: String }` (serde tag `not-recorded`, `passed() == true`, `status() == "not-recorded"`, `journal_event` is `Some`, summary `not recorded (lane: <lane>)`), returned by `check` when `lane != Virtual && update`. `Skipped` stays for `!update`. |
| I26 | Diff images are excluded from every screenshot section; a `Differed` shot's row shows its diff **labelled** beside the screenshot. The `:1434` test filename becomes `002-hub-diff.png`. |
| I9 | `RunDirectory::record(line, request, response)` writes `"data": {"line": line}`; `align` requires `entry.line() == Some(step.line)`. |
| I13 | `env.rs` exposes `APP_SOCKET_NAME` and `DAEMON_SOCKET_RELATIVE`; `rundir.rs` derives its guard from the longer one. |
| I19 | Per-run ordinal counter threaded through `inject`; no filesystem-derived ordinal; no new corpus scenario (follow-up). |
| I33 | Doc sentence only. `Cargo.toml` untouched. |

## 2. `fleet-app` contracts

### 2.1 `state/harness.rs` (owner: A1; skeleton landed by C1)

```rust
/// Wakes the harness waiter from any thread. Installed by the bridge; a no-op in production.
#[derive(Clone)]
pub struct IdleWake(Arc<dyn Fn() + Send + Sync>);
impl IdleWake {
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self;
    pub fn wake(&self);
}

/// Mutations whose daemon reply has arrived but whose snapshot the shell has not yet applied.
#[derive(Debug, Default)]
pub struct SettleCounter { pending: AtomicU32, generation: AtomicU64 }
impl SettleCounter {
    /// The daemon answered one mutation. Returns the generation to hand to `expire`.
    pub fn begin(&self) -> u64;
    /// The shell applied a snapshot: every pending mutation is covered.
    pub fn settled(&self);
    /// The grace elapsed for a mutation begun at `generation`; releases it only if no snapshot
    /// has been applied since. Returns true if it released something.
    pub fn expire(&self, generation: u64) -> bool;
    pub fn pending(&self) -> u32;
}
pub const MUTATION_SETTLE_GRACE: Duration = Duration::from_millis(250);

pub struct IdleSnapshot {
    pub idle: bool,
    pub in_flight_requests: u32,
    pub running_jobs: u32,
    pub pending_frame: bool,
    pub live_toast_timers: u32,
    pub armed_debounces: u32,
    pub settling_mutations: u32,   // new, after armed_debounces
    pub link_opening: bool,        // new, last
}
impl IdleSnapshot {
    pub fn new(in_flight_requests: u32, running_jobs: u32, pending_frame: bool,
               live_toast_timers: u32, armed_debounces: u32,
               settling_mutations: u32, link_opening: bool) -> Self; // idle = all clear
}

impl HarnessState {
    /// Replaces `track_requests`: the bridge hands over its counter, its settle counter and the wake.
    pub fn attach_bridge(&mut self, in_flight: Arc<AtomicU32>, settle: Arc<SettleCounter>, wake: IdleWake);
    pub fn settle(&self) -> &Arc<SettleCounter>;   // the shell calls .settled() on it
    pub fn settling_mutations(&self) -> u32;
    pub fn arm_debounce(&self) -> ArmedDebounce; // guard now also carries the wake
}
// ArmedDebounce::drop → decrement, then wake (if a wake is installed).
```

`IdleSnapshot` field order is frozen as above (serialization test in `state/harness/tests.rs`
pins it). §2's "all six idle fields" becomes "all eight idle fields"; §2's idle definition
appends "no mutation still awaiting its snapshot, and a daemon link past its first connection".

### 2.2 `bridge.rs`, `bridge/requests.rs`, `bridge/runtime.rs` (owner: A2; skeleton by C1)

```rust
pub enum BridgeEvent { /* existing… */ 
    /// A busy→idle edge somewhere off the foreground; carries nothing, wakes the harness waiter.
    Nudge,
}
struct InFlight { counter: Arc<AtomicU32>, wake: Option<IdleWake> } // drop: decrement, wake
impl Bridge {
    pub fn in_flight_requests(&self) -> Arc<AtomicU32>;   // existing
    pub fn settle_counter(&self) -> Arc<SettleCounter>;    // new
    pub fn idle_wake(&self) -> IdleWake;                   // new: try_send(BridgeEvent::Nudge)
}
```
Behaviour A2 implements: in `run_mutations`, after a successful `client.request(...)`,
`let generation = settle.begin(); drop(in_flight); tokio::spawn(async move { sleep(MUTATION_SETTLE_GRACE).await; if settle.expire(generation) { wake.wake(); } })`.
In `dispatch` (reply lane), after `reply.try_send(result)`, keep `in_flight` alive until
`reply.closed().await` resolves, then drop it (a spawned task; bounded by the consumer dropping
its `Receiver`). `Bridge::send` and `Bridge::request` signatures do not change.

### 2.3 `shell/root.rs` (C1) and `shell/root/events.rs` (owner: A3)

`shell/root.rs:85` becomes
`state.harness.attach_bridge(bridge.in_flight_requests(), bridge.settle_counter(), bridge.idle_wake())`.
`events.rs`: `BatchDamage` gains `nudged: bool`; `apply` sets it on `BridgeEvent::Nudge`;
on `BridgeEvent::Connected(_)` or `BridgeEvent::Daemon(Event::SnapshotChanged(_))` it calls
`state.harness.settle_counter().settled()` (A3 adds the accessor use; the accessor
`HarnessState::settle(&self) -> &Arc<SettleCounter>` is part of 2.1). After the loop:
`if damage.state { cx.notify() } else if is_recording() && (damage.nudged || damage.affects_visible_terminal(state)) { cx.notify() }`.

### 2.4 Jobs mirror (C1 skeleton; A4 reads, A5 writes)

```rust
// state/jobs_filter.rs (moved verbatim from views/jobs_panel.rs; old path re-exports it)
pub enum JobFilter { All, Running, Failed }  // + next(), label(), matches(&JobRecord)

// state.rs
pub struct JobsPanelMirror { pub cursor: usize, pub filter: JobFilter } // Debug, Clone, Copy, Default, PartialEq, Eq
pub struct AppState { /* … */ pub jobs_panel: JobsPanelMirror, /* … */ }
impl AppState {
    /// Mirrors the Jobs panel's own cursor and filter. Returns true if anything changed.
    pub fn set_jobs_panel(&mut self, mirror: JobsPanelMirror) -> bool;
}
```
`Cursors::jobs` is deleted along with `snapshot.rs:87`. `projection.rs` reads
`self.jobs_panel.cursor` at the two former `cursors.jobs` sites, builds `lists.jobs` from
`snapshot.jobs.iter().filter(|job| self.jobs_panel.filter.matches(job))` (A4), and
`ProjectionKey` gains `jobs_panel: JobsPanelMirror` and `renamed_terminals: <same type as the field>` (A4).
A5 calls `state.update(cx, |state, cx| { if state.set_jobs_panel(JobsPanelMirror { cursor: panel.cursor, filter: panel.filter }) { cx.notify(); } })`
at every site that writes `panel.cursor` or `panel.filter` (`screens/jobs.rs`,
`screens/jobs/actions.rs`, `screens/jobs/presentation.rs`), in the same closure as the write.

## 3. `fleet-harness` contracts

### 3.1 `agent/peer.rs` + `agent.rs` (owner: H12; skeleton by C2)

```rust
// agent.rs
pub(crate) const GATE_BUDGET: Duration = Duration::from_secs(5);
// agent/peer.rs — no new dependency, no FFI: the reader moves to a thread on first use.
enum Source<R> { Direct(R), Threaded(std::sync::mpsc::Receiver<std::io::Result<String>>) }
impl<R: BufRead + Send + 'static, W: Write> Peer<R, W> {
    pub(crate) const fn new(reader: R, writer: W, pace: Pace) -> Self;   // unchanged signature
    pub(crate) fn read_frame(&mut self) -> anyhow::Result<Option<Value>>;  // unchanged; works on either Source
    /// `read_frame`, but fails with a named error if no frame arrives within `budget`.
    /// On first call a `Direct` reader is moved into a reader thread (`read_line` loop sending
    /// each line; the sender drops at EOF so `recv` reports `Ok(None)`); the wait is
    /// `recv_timeout(budget)`. Error text: `no frame from the client within {budget:?}`.
    pub(crate) fn read_frame_within(&mut self, budget: Duration) -> anyhow::Result<Option<Value>>;
}
```
The pipe stays blocking; the thread is a daemon thread that dies with the process. `Peer`'s
type arity does not change, so `Session<R, W>` in `codex.rs`/`claude.rs` only widens its `R`
bound to `BufRead + Send + 'static` (the compiler asks for it).

### 3.2 `agent/codex.rs` (H10), `agent/claude.rs` (H11)

Gate loops call `self.peer.read_frame_within(GATE_BUDGET)?`; on `Err` the turn settles as
`Settlement::Interrupted` and the error is returned after the terminal frame is emitted. After
`answer_simple`/`answer_control` inside a gate loop: `if self.interrupted { return Ok(Decision::Cancel) }`
(codex) / `return Ok(Decision::Withdrawn)` with `Settlement::Interrupted` at the call site
(claude). Top of `run_turn`: `let mut settlement = if std::mem::take(&mut self.interrupted) { Settlement::Interrupted } else { <current initial> };`.

### 3.3 `agent/transcript.rs` (H14)

`validate` collects every `Permission`/`Approval`/`ToolCall` id into one set; a repeat fails
with `duplicate id {id:?}: ids are shared between gates and tool calls`.

### 3.4 `rundir.rs` (H2), `env.rs` (C2 adds consts), `scenario.rs` (H1), `report.rs` (H3)

```rust
// env.rs
pub const APP_SOCKET_NAME: &str = "fleet-harness.sock";      // used at env.rs:117
pub const DAEMON_SOCKET_RELATIVE: &str = "home/fleetd.sock"; // documented beside daemon_socket()
// rundir.rs
pub async fn record(&self, line: usize, request: &Request, response: &Response) -> anyhow::Result<()>;
// journal shape: {"at","kind":"command","request","response","data":{"line":N}}
```
`scenario.rs:504` passes `line.number`. `report.rs` `align` uses `entry.line() == Some(step.line)`.
`stop(child, what, grace) -> Result<Option<ExitStatus>, String>`; teardown fails the run on a
non-success status from Fleet.

### 3.5 `baseline.rs` (H4)

`BaselineOutcome::NotRecorded { lane: String }` per §1. `expand`/`inflate` take a `ceiling: usize`
(`checked_mul` in `u64`, `bytes_per_row + 1` × `height`, rejected with a named error when it
overflows or exceeds `MAX_DECODED_BYTES = 64 MiB`), and `inflate` checks the ceiling at every
push site.

### 3.6 `fixture/plan.rs` + `fixture/tools.rs` + `agent/launcher.rs` (H9)

`shell_word` becomes `pub(crate)` (C2). Both `@DATA@` substitutions go through it. The third
scripted agent decision is H9's (see §1 I18) and is reported under `DECISIONS`.

## 4. `docs/TESTING-HARNESS.md` (owner: D1) — what changes, by section

- §2: idle definition + "all eight idle fields"; the five `await idle …` forms parse as
  documented; `type`/`clipboard set` trim leading/trailing whitespace and preserve interior.
- §3: `idle` fields (add `settling_mutations`, `link_opening` in order); target indices are model
  positions and a scrolled-out row has no target; `terminal.rows` two trims and
  `rows.len() != viewport.rows`; `lists.jobs` is the filtered row set and `selected` is the
  panel cursor.
- §5: leave the transcript count for the Integrate stage (depends on H9's decision).
- §6: `--update-baselines` records only in the `virtual` lane; other lanes report `not-recorded`.
- §8: `command` journal entries carry `data.line`; a report whose journal lost a line is marked
  untrustworthy; a `shots/` directory that cannot be listed is reported as such; a diff image is
  shown labelled beside its screenshot; a Fleet that exits non-zero after `quit` fails the run.
- §11: append (existing entry shape: verified limit, mechanism, workaround): `dialog.fields` and
  `dialog.message` are always empty (assert via `targets["dialog.field[N]"]` and keystrokes);
  headless `await` does not repaint so `window.frame` freezes (use `assert`/`dump`).

## 5. Scenarios (owner: S1)

- `scenarios/hub/idle-accounting.scenario`: the two `assert idle.*` lines gain
  `idle.settling_mutations == 0` and `idle.link_opening == false`; the header comment's "five"
  becomes "seven".
- `scenarios/hub/idle-accounting.scenario` and `scenarios/agents/prefix-inside-a-thread.scenario`:
  replace the prose explaining that `await idle` is not enough with the assertion it stood in for
  where one exists; keep the `state == working`/`idle` edge waits (they assert turn edges, which
  `idle` does not cover).
- `scenarios/hub/jobs-panel.scenario`: assert a non-zero `lists.jobs.selected` after a cursor
  move and the filtered `lists.jobs.rows` after `f`, so `Esc` closing the panel turns it red.
- `scenarios/hub/rail-collapse.scenario`: `await targets["repos.rail"].w == 44` after the first
  `key H`, `== 240` after the second; delete the false paragraph in the header.

## 6. Ownership table (path → agent)

| Path | Agent |
| --- | --- |
| `crates/fleet-app/src/state/harness/tests.rs`, `state.rs`, `state/navigation.rs`, `state/snapshot.rs`, `state/jobs_filter.rs` (new), `views/jobs_panel.rs`, `shell/root.rs`, skeleton lines in the files below | C1 (contracts, app) |
| `crates/fleet-harness/src/env.rs`, `agent/tests.rs`, `fixture/tests.rs`, skeleton lines in the files below | C2 (contracts, harness) |
| `crates/fleet-app/src/state/harness.rs` | A1 |
| `crates/fleet-app/src/bridge.rs`, `bridge/requests.rs`, `bridge/runtime.rs`, `bridge/tests.rs` | A2 |
| `crates/fleet-app/src/shell/root/events.rs` | A3 |
| `crates/fleet-app/src/state/harness/projection.rs` | A4 |
| `crates/fleet-app/src/screens/jobs.rs`, `screens/jobs/**` | A5 |
| `crates/fleet-app/src/drive.rs` | A6 |
| `crates/fleet-harness/src/scenario.rs` | H1 |
| `crates/fleet-harness/src/rundir.rs` | H2 |
| `crates/fleet-harness/src/report.rs` | H3 |
| `crates/fleet-harness/src/baseline.rs` | H4 |
| `crates/fleet-harness/src/fault.rs` | H6 |
| `crates/fleet-harness/src/fixture/jobs.rs` | H8 |
| `crates/fleet-harness/src/fixture/plan.rs`, `fixture/tools.rs`, `agent/launcher.rs`, `transcripts/*.json`, `scenarios/agents/blocked/error-mid-stream.blocked` | H9 |
| `crates/fleet-harness/src/agent/codex.rs` | H10 |
| `crates/fleet-harness/src/agent/claude.rs` | H11 |
| `crates/fleet-harness/src/agent/peer.rs`, `agent.rs` | H12 |
| `crates/fleet-harness/src/agent/transcript.rs` | H14 |
| `crates/fleet-drive/src/lib.rs` | D2 |
| `docs/TESTING-HARNESS.md` | D1 |
| `scenarios/hub/*.scenario`, `scenarios/agents/prefix-inside-a-thread.scenario` | S1 |
| everything, after Implement | Integrate |
| `plans/*-tracker.md` | Trackers (last) |

## 7. Definition of done for the whole initiative

- `make lint` clean, `make test` green, `make harness` corpus green (or the tracker says exactly
  why not and what was verified instead).
- Every task in the four trackers ticked with pasted output; every decision in §1 recorded in the
  matching tracker's decisions log.
- `docs/TESTING-HARNESS.md` matches the code section by section (§4 above).
- `rail-collapse.scenario` and `jobs-panel.scenario` demonstrated red with the behaviour removed.
