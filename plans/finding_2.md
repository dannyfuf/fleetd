# Review 2 — `test-harness` branch against `881162c5`

Scope: everything on `test-harness`, with emphasis on `crates/fleet-drive`, `crates/fleet-harness`,
the `UiSnapshot` projection, the bridge idle accounting, and `docs/TESTING-HARNESS.md` as a frozen
contract.

### F2-1: `idle` reports quiescence before the daemon's answer has reached the UI
- **Severity**: P1
- **Location**: `crates/fleet-app/src/bridge/requests.rs:76`, `crates/fleet-app/src/bridge/requests.rs:122`
- **Problem**: The `InFlight` claim is released on the *runtime* thread as soon as `client.request(...)`
  resolves, not when the application has applied the answer. For `Bridge::send` (every fire-and-forget
  mutation) the resulting `SnapshotChanged` still has to travel the event channel and be applied by the
  shell's drain loop — none of which is counted by any of the five `idle` constituents.
- **Impact**: `await idle` — used 62 times across `scenarios/`, and the only synchronisation the corpus
  has after a mutating action — can return while the app is still holding pre-mutation state. The two
  outcomes are a flaky red (the next `assert` races the update) and, worse, a silent green (the next
  `assert` happens to match the *old* state and passes). This is the harness's primary quiescence
  signal, so it degrades every scenario rather than one.
- **Fix**: Hold the claim until the app has consumed the answer, not until the runtime has produced it.
  Concretely: move the `InFlight` guard into the reply envelope so it is dropped by the UI-side
  consumer of `Bridge::request`'s `Receiver`, and for `Bridge::send` release it only after the
  `BridgeEvent` the mutation produces has been drained and applied (or add a sixth counter —
  `pending_events` — fed by the shell's event-drain loop and included in `IdleSnapshot::new`).
  `docs/TESTING-HARNESS.md` §3 says `idle` is derived from the counters, so a new counter is additive
  and does not bump the snapshot version.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/bridge/requests.rs:76
  async fn run_mutations(mutations: Receiver<Mutation>, events: Sender<BridgeEvent>) {
      while let Ok(Mutation { client, body, _in_flight }) = mutations.recv().await {
          if let Some(client) = client {
              if let Err(error) = client.request(*body).await { … }   // `_in_flight` drops here
  ```
  ```rust
  // crates/fleet-app/src/bridge/requests.rs:122
  tokio::spawn(async move {
      let _in_flight = in_flight;
      let result = client.request(body).await;
      …
      let _ignored = reply.send(result).await;          // task ends → claim released
  });
  ```
  Reachable path: `scenarios/hub/pane-focus.scenario:14` (`await idle` immediately after a key that
  mutates daemon state) → `apply()` → `Harness::wait_for` → `IdleSnapshot::new(in_flight=0, …)` →
  `satisfied` → the following `assert` reads a snapshot the daemon's reply has not yet reached.

### F2-2: a Fleet that aborts during shutdown passes the run — the exit-status check is unreachable
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/scenario.rs:561`, `crates/fleet-harness/src/scenario.rs:251`
- **Problem**: The orderly-shutdown assertion is guarded by `app.try_wait()` returning `Some(status)`,
  but it is polled microseconds after the `quit` *response* was written — before the app has run
  `cx.quit()`, torn down its window and released the GPU context. `try_wait` therefore returns `None`
  essentially always, the `break` never fires, and teardown's `stop()` then discards the status on
  both of its success paths.
- **Impact**: Exactly the failure this branch fixes elsewhere (`shell/root/bootstrap.rs`'s
  `log::set_max_level(Off)`, added because Fleet aborted with "fatal runtime error: failed to initiate
  panic" on every quit) would regress green: the report would say "19 lines in 401 ms · passed" while
  `app.log` ends in an abort. A false green on the one thing every scenario's last line checks.
- **Fix**: After a `quit` line, *wait* for the process rather than polling it — `tokio::time::timeout(QUIT_GRACE, app.wait())` — and fail on a non-success status; and make `stop()` return the status (or at least fail on a non-success one) instead of `waited.map(|_status| ())`.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/scenario.rs:561
  match app.try_wait().context("poll the Fleet process")? {
      Some(status) if matches!(line.instruction, Instruction::App(Command::Quit(_))) => {
          anyhow::ensure!(status.success(), "Fleet answered `quit` and then exited with {status}; …");
  ```
  ```rust
  // crates/fleet-harness/src/scenario.rs:251
  async fn stop(child: &mut Child, what: &str, grace: Duration) -> Result<(), String> {
      match child.try_wait() {
          Ok(Some(_status)) => return Ok(()),        // status discarded
      …
          return waited.map(|_status| ())            // status discarded
  ```
  Reachable path: every scenario ending in `quit` (all of them).

### F2-3: the jobs overlay's `focused` and `lists.jobs.selected` are invented from a cursor nothing moves
- **Severity**: P2
- **Location**: `crates/fleet-app/src/state/harness/projection.rs:319`, `crates/fleet-app/src/state/harness/projection.rs:433`
- **Problem**: Both read `AppState::cursors.jobs`, which is written by exactly one line in the whole
  workspace — `state/snapshot.rs:87`'s `clamp_cursor` — and never moved by the jobs panel. The real
  selection lives in `JobsPanelState::cursor` (`screens/jobs.rs:44`, "an index into the **visible**
  rows"), which `AppState` cannot see. The module header claims "nothing here is a second source of
  truth"; this is one, and it is stuck at 0.
- **Impact**: While the Jobs overlay is open the snapshot always reports `focused == "jobs.row[0]"`
  and always reports the first daemon job as `lists.jobs.selected`, whatever the user selected. A
  scenario that moves the cursor and asserts `focused` fails for the wrong reason; a scenario that
  asserts it *did not* move passes without checking anything. `scenarios/hub/jobs-panel.scenario:15`
  and `:29` are already in the second category — `focused == jobs.row[0]` is true by construction, so
  the line that exists to prove `Esc` collapsed a log without closing the panel proves nothing about
  focus. `lists.jobs` is also built from the unfiltered `snapshot.jobs` while the panel renders a
  `JobFilter`-filtered list, so `jobs.row[N]` (a click target) and `lists.jobs.rows[N]` (the oracle)
  address different jobs whenever a filter is on.
- **Fix**: Derive both from something the panel actually owns — mirror the panel's visible-row cursor
  (or its selected `JobId`) into `AppState` on every cursor move and filter change, and build
  `lists.jobs` from the same filtered row set the panel renders, so `jobs.row[N]` and
  `lists.jobs.rows[N]` are the same row. Add the mirrored value to `ProjectionKey`.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/state/harness/projection.rs:319
  Overlay::Jobs => format!("jobs.row[{}]", self.cursors.jobs),
  // crates/fleet-app/src/state/harness/projection.rs:433
  list(self.job_rows(snapshot), self.cursors.jobs, String::new()),
  ```
  `rg 'cursors\.jobs' crates/` returns three hits: the clamp above and these two reads. No write.

### F2-4: `rail-collapse.scenario` never asserts the collapse it exists to pin, on a false premise
- **Severity**: P2
- **Location**: `scenarios/hub/rail-collapse.scenario:7`
- **Problem**: The scenario's own header says the rail width "is in the snapshot … but no predicate
  can read it, because a target name contains the `.` a dotted path splits on". That is untrue:
  `docs/TESTING-HARNESS.md` §2 freezes the quoted-step form for exactly this case and
  `fleet-drive/src/predicate.rs:413` (`parse_bracket`) implements it —
  `scenarios/hub/pointer-tabs.scenario:13` already asserts `targets["repos.rail"].w == 240`.
- **Impact**: `H` is the one Hub affordance whose entire effect is geometry, and §3 names the rail's
  own `w` (240 ↔ 44) as its oracle. The scenario asserts only what a collapse *preserves* (rows,
  cursor, key context), all of which stay true if `H` stops collapsing the rail altogether. The
  remaining evidence is a `shot`, and §11 records that no baseline has ever been recorded, so the
  screenshot is compared against nothing. The behaviour is effectively untested.
- **Fix**: Add `await targets["repos.rail"].w == 44` after the first `key H` and
  `await targets["repos.rail"].w == 240` after the second, and delete the incorrect paragraph from the
  header comment.
- **Evidence**:
  ```text
  # scenarios/hub/rail-collapse.scenario:7
  # 240 to 44 in `targets` — but no predicate can read it, because a target name contains the `.`
  # scenarios/hub/pointer-tabs.scenario:13
  assert targets["hub.tab[0]"] exists && targets["repos.rail"].w == 240
  ```

### F2-5: `dialog.fields`, `dialog.buttons` and `dialog.message` are always empty, against frozen §3
- **Severity**: P2
- **Location**: `crates/fleet-app/src/state/harness/projection.rs:367`
- **Problem**: §3 freezes `dialog` as `{name,fields:[{name,value,focused}],buttons:[string],message:string|null}`,
  and §3's target table spells out which field index each dialog's tab cycle assigns. The builder
  returns `fields: Vec::new(), buttons: Vec::new(), message: None` unconditionally, and §11 ("Known
  gaps", which is meant to list every place reality is short of the frozen surface) does not mention
  it.
- **Impact**: Every predicate over `dialog.fields[…]` resolves to nothing, so a scenario that tries to
  verify what a dialog's inputs contain or which one has focus fails with "nothing at that path" and
  the author has no document telling them why. The dialogs are the surface this branch invested the
  most target naming in (`dialog.field[N]` is painted by six dialogs) and the snapshot side of that
  pairing does not exist.
- **Fix**: Either mirror the open dialog's field name/value/focused triples into `AppState` from the
  dialog host on each edit (and add that to `ProjectionKey`), or — if that is deliberately deferred —
  add a §11 entry saying `dialog.fields`, `dialog.buttons` and `dialog.message` are always empty today
  and that a dialog's content is asserted through `targets["dialog.field[N]"]` and keystrokes instead.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/state/harness/projection.rs:371
  Some(DialogSnapshot { name: dialog.context_name().to_owned(),
      fields: Vec::new(), buttons: Vec::new(), message: None })
  ```

### F2-6: in the headless lane an `await` never repaints, so `targets` and `window` are frozen for its whole duration
- **Severity**: P3
- **Location**: `crates/fleet-app/src/drive.rs:651`, `crates/fleet-app/src/drive.rs:481`
- **Problem**: `wait_for` wakes on `cx.observe(state)` and re-runs `project()`, but `project()` only
  *copies* `fleet_ui_kit::harness::painted(window)` and `frame(window)` — it never paints. In a
  compositor lane a `cx.notify()` invalidates the window and a frame follows, so the table refreshes;
  in the headless lane nothing paints unless `Harness::paint` is called, and it is called once, before
  the loop. The doc comment claims the two fields are "read as of the moment this call projects them",
  which is only true where a frame loop exists.
- **Impact**: A predicate over `targets[…]` or `window.*` that becomes true *during* an await (a
  surface opened by an arriving daemon event, a resize granted late) can never be satisfied headlessly
  and times out, while the same scenario passes in `virtual`. §9.7 tells authors to run every scenario
  in both lanes, so this presents as an unexplained lane-specific failure. No scenario hits it today.
- **Fix**: Call `self.paint(cx)` at the top of each `wait_for` iteration when `self.headless`, or
  document the restriction beside `targets`/`window` in §11.
- **Evidence**: `wait_for` at `drive.rs:674` loops `let projection = self.project(state, cx)?;` /
  `tokio::select! { … changed_rx.recv() … }` with no `paint`; `settle()` at `drive.rs:482` is the only
  headless painter and runs before the loop (`drive.rs:366`).

### F2-7: the run-directory length guard measures the wrong socket — the app's is two bytes longer
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/rundir.rs:29`, `crates/fleet-harness/src/env.rs:117`
- **Problem**: `LONGEST_SOCKET_NAME` is `"home/fleetd.sock"` (16 bytes) and is documented as "the
  longest socket path any run creates inside its own directory". `HarnessEnv` puts the app's command
  socket at `<root>/fleet-harness.sock` — 18 bytes — so the guard passes for a root where the app's
  own `bind` will fail with `ENAMETOOLONG`.
- **Impact**: For a `--run-dir` in the two-byte window the guard reports success, the fixture seeds,
  `fleetd` starts, and Fleet then fails to open its socket — surfacing as "Fleet did not open … within
  60s; see app.log", which is precisely the buried failure this constant exists to prevent.
- **Fix**: Measure the longest of the two names (`"fleet-harness.sock"`), or compute the maximum over
  both paths.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/rundir.rs:29
  const LONGEST_SOCKET_NAME: &str = "home/fleetd.sock";
  // crates/fleet-harness/src/env.rs:117
  socket: root.join("fleet-harness.sock"),
  ```

### F2-8: two runs of the same scenario in the same second share one run directory
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/rundir.rs:55`
- **Problem**: The default root is `<UTC seconds>-<stem>` and `create_dir_all` treats an existing
  directory as success, so a second `make harness-one` started inside the same second reuses the first
  run's `home/`, `run.jsonl`, `shots/` and `dumps/`.
- **Impact**: Two `fleetd` processes race the same `FLEET_HOME` and pid lock, the journals interleave
  (breaking `report::align`'s positional pairing), and the first run's `remove_home()` deletes the
  second's home mid-run. The evidence the run directory is supposed to be becomes unreadable.
- **Fix**: Include the pid (as `run_id()` already does) or millisecond precision in the directory name,
  or create the directory with `create_dir` and retry with a suffix on `AlreadyExists`.
- **Evidence**: `rundir.rs:58` `format!("{}-{}", stamp(utc(SystemTime::now())), sanitize(stem))` with
  `stamp` rendering `YYYYmmdd-HHMMSS` (`rundir.rs:240`), then `create_dir_all` at `rundir.rs:77`.

### F2-9: `daemon kill` seals the home only after the kill, leaving a window for an unowned fleetd
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/fault.rs:199`
- **Problem**: `kill()` sends `SIGKILL`, waits for the exit, and only then calls `seal()` to make
  `fleetd.pid` a directory. `fleet_client::ensure_daemon` restarts a daemon whenever the socket is
  dead; anything that reaches that path inside the window starts a `fleetd` the runner never spawned,
  cannot signal again and does not tear down. `restart()` has the mirror-image window between
  `unseal()` and `start_replacement()`.
- **Impact**: A leaked daemon holding the run's `FLEET_HOME` after the run directory is reclaimed, and
  a `daemon kill` that silently does not reach the daemon-down surface. Narrow (Fleet notices the loss
  on a 2 s health tick) but unbounded in consequence.
- **Fix**: Seal before signalling — create the `fleetd.pid` directory first, then kill — so no restart
  can win the race; and in `restart()` spawn the replacement before unsealing, or hold the seal across
  the spawn.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/fault.rs:199
  async fn kill(process: &mut Daemon) -> anyhow::Result<()> {
      let pid = running_pid(process, "kill").await?;
      …                                   // process dies here
      seal(process)?;                     // …and only now can it not come back
  ```

### F2-10: the fake `gh`/`acli` embed their data path in a single-quoted shell literal with no quoting guard
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/fixture/tools.rs:77`, `crates/fleet-harness/src/fixture/tools.rs:197`
- **Problem**: `GH.replace("@DATA@", &data.to_string_lossy())` substitutes into `data='@DATA@'`. A
  `--run-dir` containing a `'` closes the literal and the rest of the path is executed as shell words.
  `agent::launcher::shell_word` (`agent/launcher.rs:295`) refuses exactly this case for the agent
  launcher; the fixture scripts do not.
- **Impact**: A run directory with a quote in it produces a broken or executing fake `gh`, so a
  "hermetic" run can run arbitrary words from a path. Only reachable through a path the operator
  supplies, but the inconsistency with the guard two modules away is the bug.
- **Fix**: Route both templates through the same `shell_word` guard (or reject a `'` in
  `RunDirectory::create`, which already sanitises the generated name but not a supplied `--run-dir`).
- **Evidence**: `tools.rs:197` `data='@DATA@'`, `tools.rs:253` `data='@DATA@'`, substituted at
  `tools.rs:77` and `tools.rs:101` with no validation.

### F2-11: `await <path> exists` is rejected when the path is literally `idle`
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/scenario.rs:1121`
- **Problem**: `predicate_and_timeout` classifies the trailing atom by shape. The arm `["idle", _] => 1`
  matches any two-token atom starting with the word `idle`, so `await idle exists` is read as the bare
  `idle` clause plus a timeout of `"exists"`, and `"exists".parse::<u64>()` fails.
- **Impact**: `await idle exists` and `await idle absent` — legal under the §2 grammar, since `idle`
  is also a snapshot object — fail the scenario at parse time with "invalid digit found in string",
  which names neither the line's real problem nor the grammar.
- **Fix**: Match the two-token `idle` form only when the second token parses as a number, or check the
  `exists`/`absent` arm before the `idle` arm.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/scenario.rs:1121
  ["idle", _] => 1,
  [_, "exists" | "absent", _] => 2,
  …
  let timeout = parts[atom + atom_words].parse()?;
  ```

### F2-12: §5 says the `agents` fixture embeds all three starter transcripts; it embeds two
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/fixture/plan.rs:455`, `docs/TESTING-HARNESS.md` §5
- **Problem**: §5 ends "The starter transcripts ship in `crates/fleet-harness/transcripts/`
  (`two-turns.json`, `edit-approval.json`, `error-mid-stream.json`) and are what the `agents` fixture
  embeds." `fn agents()` embeds `two-turns.json` (Claude) and `edit-approval.json` (Codex) only;
  `error-mid-stream.json` is validated by a unit test and used by no preset.
- **Impact**: An author reading the frozen document writes a scenario against a mid-stream error the
  `agents` preset cannot produce. `scenarios/agents/blocked/error-mid-stream.blocked` exists, which
  suggests this was already hit once.
- **Fix**: Either give the preset a third scripted provider carrying `error-mid-stream.json`, or amend
  §5 to say which two the fixture embeds and that the third is a starter for hand-written fixtures.
- **Evidence**: `plan.rs:421-445` (`fn agents()`) lists two `Agent` entries; `include_str!` appears
  only at `plan.rs:455` and `plan.rs:463`.

### F2-13: §3 says target indices are "current visual order"; the list surfaces record the model index
- **Severity**: P3
- **Location**: `crates/fleet-app/src/views/worktrees_list.rs:347`, `crates/fleet-app/src/views/prs_screen.rs:293`, `crates/fleet-app/src/views/repos_rail.rs:317`, `crates/fleet-app/src/screens/jobs/presentation.rs:245`
- **Problem**: These call `harness_target_indexed(part, index)` with the index GPUI's
  `uniform_list`/`list` hands the builder, which is the position in the row model, not the position on
  screen. §3 says "Indices describe current visual order".
- **Impact**: Once a list is scrolled, `worktrees.row[0]` is not the top visible row — it is the model
  row 0, which is not painted at all, so `click worktrees.row[0]` fails with "unknown target … nearest
  of the N painted". The model index is arguably the more useful key (it matches `lists.*.rows[N]`),
  so the drift is in the document rather than the code, but one of the two has to move before a
  scrolled-list scenario is written.
- **Fix**: Change §3 to "indices are positions in the list the snapshot reports under `lists`, which
  for a scrolled list is not the on-screen position", and say plainly that a row scrolled out of view
  has no target.
- **Evidence**: `worktrees_list.rs:345-349` — the closure parameter `index` is `uniform_list`'s item
  index, and the same value indexes `rows`, the full filtered model, not the visible window.

---

Checked and found clean: no `unwrap`/`expect`/`todo!`/`dbg!` on a reachable path in `fleet-drive` or
`fleet-harness` production code; the three `let _ =` additions are fire-and-forget channel sends with
the required comment; the hand-rolled PNG/inflate decoder bounds every index it takes
(`baseline.rs:723-891`, `:940-1130`); `server.rs`'s drop ordering releases the socket thread before it
joins it; `Daemon`, `HyprlandBackend` and the seeding daemon each reap their children on every error
path; `ToastStack`'s render cap equals `AppState`'s `MAX_TOASTS`, so `toasts.toast[N]` and `toasts[N]`
agree; `parse_click`'s count/point ambiguity resolves correctly because a lone numeric token is not a
valid location.
