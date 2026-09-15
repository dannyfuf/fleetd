# Deep review — `test-harness` vs `881162c5`

**Scope.** All 10 commits on `test-harness` against base `881162c5c283cefb4319bbfdb79629ef31960745`:
176 files, ~27.6k insertions. Two new crates (`fleet-drive`, `fleet-harness`), the `UiSnapshot`
projection and bridge idle accounting in `fleet-app`, the named-target registry in `fleet-ui-kit`,
the scripted mock peer in `fleet-daemon`, `Makefile` targets, `docs/TESTING-HARNESS.md` (frozen
contract) and the `scenarios/` corpus. Working tree was clean; nothing uncommitted.

## Process

Two independent reviewers, contexts isolated, neither seeing the other's output → `plans/finding_1.md`
(25 findings) and `plans/finding_2.md` (13 findings). A third reviewer (Codex `gpt-5.6-sol`) died at
11m on an out-of-credits error before writing; its one recorded partial conclusion — that the idle
counters can reach zero without emitting the notification `await idle` waits on — independently
corroborates C2 below and is recorded here rather than in a findings file.

38 raw findings deduplicated to **33 candidates** (4 merge pairs where both reviewers found the same
failure mode; 1 merge of two *contradictory* readings of the same code). Each candidate then went to
adversarial validation: batches `idle` and `false-green` and `agent` got two validators each, one
instructed to refute and one to prove, working independently; batches `png`, `projection` and `env`
got one dual-role validator instructed to refute first and prove only on failure. Nine validators total.

**1 candidate dropped. 32 survived, 20 of them narrowed** — in most narrowings a specific sub-claim
was disproved and has been struck from the issue below. Validators reproduced C5, C8, C10, C11, C12,
C13, C16, C17, C18 and C29 with working repros, and C1 3/3 with a passing control.

One **P0 blocker was found incidentally** by two validators and then confirmed directly by me; it is
not from either reviewer.

Severity is the validator's call where it differed from the reviewer's, and several were lowered.

---

## P0

### I1 — `fleet-app`'s test suite does not compile at HEAD, so `make test` cannot pass
- **Sources**: none (incidental; found by two validators, verified directly)
- **Verdict**: confirmed by direct reproduction — high confidence
- **Problem**: `DaemonLink::Reconnected` gained a third field, `reattached: usize`
  (`crates/fleet-app/src/state/connection.rs:43-48`), when merge commit `0f02991` brought
  `45297f3 daemon: keep terminals alive across daemon restarts` in from `main`.
  `crates/fleet-app/src/state/harness/tests.rs:440` still constructs the variant with two fields.
- **Evidence**:
  ```
  $ cargo check -p fleet-app --tests
  error[E0063]: missing field `reattached` in initializer of `state::connection::DaemonLink`
     --> crates/fleet-app/src/state/harness/tests.rs:440:20
  error: could not compile `fleet-app` (lib test) due to 1 previous error
  ```
- **Impact**: no `fleet-app` test binary builds, so `make test` fails outright and every test this
  branch adds — `harness_headless.rs`'s corpus slice, the `idle` regression tests, the projection
  tests — is currently unrun. It also means the evidence for several issues below cannot be
  exercised by CI. This is a merge-resolution slip, not a design defect, and is a one-line fix.
- **Fix**: add `reattached: <n>` to the initializer at `state/harness/tests.rs:440` and assert on it.

---

## P1

### I2 — a Fleet that aborts during shutdown reports a fully green run
- **Sources**: F2-2 · **Verdict**: 2/2 survives (refuter: "got stronger, not weaker") — high confidence
- **Problem**: the orderly-shutdown assertion at `crates/fleet-harness/src/scenario.rs:561` is guarded
  by `app.try_wait()` returning `Some(status)`, polled microseconds after the `quit` *response* was
  written. It never observes an exit, so `ensure!(status.success(), …)` never runs; teardown's `stop()`
  then discards the status on both success paths (`scenario.rs:253`, `:260`).
- **Evidence**:
  - `scenario.rs:561` `match app.try_wait().context("poll the Fleet process")?` — the `Some(status)`
    arm is the only place the exit status is checked.
  - `scenario.rs:251` `async fn stop(…)` → `Ok(Some(_status)) => return Ok(())` and
    `waited.map(|_status| ())` — status discarded on both paths.
  - `crates/fleet-app/src/shell/root/bootstrap.rs:103-111` documents that the abort this assertion
    exists to catch happens in a thread-local destructor *after `main` returns* — "on every quit, in
    every Wayland session". There is no timing in which `try_wait` sees it.
  - **Repro** (validator): a fake `fleet` speaking the real drive protocol, answering `quit` then
    `os._exit(134)`, driven through the real `fleet-harness` + real `fleetd` on `--lane headless`
    yields `EXIT=0`, `ok: 2 steps`, `**Passed** — 2 lines in 1 ms`, while `app.log` ends in the abort.
    Clean-exit and 2s-delayed-abort control runs produce byte-identical output.
- **Impact**: the exact regression this branch fixes elsewhere (the `log::set_max_level(Off)` in
  `bootstrap.rs`, added because Fleet aborted with "fatal runtime error: failed to initiate panic" on
  every quit) would silently return green. All 41 corpus scenarios end in `quit`, so this is the one
  assertion every scenario relies on. Nothing reads `app.log`, no test covers it, §11 does not record it.
- **Fix**: after a `quit` line, *wait* rather than poll — `tokio::time::timeout(QUIT_GRACE, app.wait())`
  — and fail on a non-success status; make `stop()` propagate the status instead of discarding it.

### I3 — `await idle` returns before the daemon's answer has reached the UI
- **Sources**: F2-1 (+ F1-11, whose opposite reading is refuted — see Dropped/Narrowed)
- **Verdict**: 2/2 survives narrowed — high confidence; reproduced 3/3 with a passing control
- **Problem**: the `InFlight` claim is released on the runtime thread when `client.request(…)`
  resolves, not when the app has applied the answer. For `Bridge::send` the resulting
  `SnapshotChanged` still has to cross the event channel and be drained by the shell — which no
  `idle` constituent counts.
- **Evidence**:
  - `crates/fleet-app/src/bridge/requests.rs:76` — `run_mutations`: `_in_flight` drops when
    `client.request(*body).await` returns.
  - `crates/fleet-app/src/state/harness.rs:192` — `idle` is the AND of five counters, none of which
    covers "an event is queued but not yet applied".
  - `crates/fleet-daemon/src/server/broadcast.rs:13` — `SNAPSHOT_COALESCE_WINDOW = 50 ms` coalesces
    the broadcast, which makes this **deterministic rather than a race**: the validator measured
    `await idle` returning 4 ms after a mutating keystroke with all five counters zero, the next line
    then acting on stale state.
  - The corpus already works around it in prose: `scenarios/agents/prefix-inside-a-thread.scenario:16-17`
    and `scenarios/hub/idle-accounting.scenario:4-7`.
- **Impact**: `await idle` is the corpus's only synchronisation after a mutating action (62 uses). A
  scenario written the obvious way — mutate, `await idle`, assert daemon-derived state — can read
  pre-mutation state: a flaky red, or a silent green when the old value happens to match.
- **Narrowed**: today's corpus does not actually break. A validator enumerated all 14
  `<action> → await idle` pairs and none asserts on daemon-derived state, and a parked waiter is
  rescued by `apply_batch`'s notify. So this is a loaded gun rather than a live false green — but it
  is the harness's primary quiescence primitive, and the next scenario written naturally will hit it.
- **Fix**: hold the claim until the app has consumed the answer — move the guard into the reply
  envelope so the UI-side consumer drops it, or add a sixth `pending_events` counter fed by the
  shell's drain loop and included in `IdleSnapshot::new`. §3 derives `idle` from the counters, so an
  additional counter is additive and does not bump the snapshot version.

---

## P2

### I4 — the Jobs overlay's `focused` and `lists.jobs.selected` are provably pinned at 0
- **Sources**: F2-3 · **Verdict**: survives, strengthened — high confidence
- **Problem**: `projection.rs:319` (`focused` → `format!("jobs.row[{}]", self.cursors.jobs)`) and
  `:433` (`lists.jobs.selected`) both read `AppState::cursors.jobs`. The validator re-searched
  independently: `cursors.jobs` has exactly three references workspace-wide, and its single write is
  `clamp_cursor` (`state/snapshot.rs:87`) — a monotonically non-increasing function on a field that
  starts at 0. It is therefore **provably pinned at 0 forever**, stronger than the reviewer claimed.
  The real selection lives in `JobsPanelState::cursor` (`screens/jobs.rs:44`), which `AppState`
  cannot see. Separately, `lists.jobs` is built from the unfiltered `snapshot.jobs` while the panel
  renders a `JobFilter`-filtered list, so `jobs.row[N]` (click target) and `lists.jobs.rows[N]`
  (oracle) address different jobs whenever a filter is on.
- **Impact**: while the overlay is open the snapshot always reports `focused == "jobs.row[0]"` and the
  first daemon job as selected. `scenarios/hub/jobs-panel.scenario:15` and `:29` are vacuous — true
  by construction, so the line that exists to prove `Esc` collapsed a log without closing the panel
  proves nothing. The module header's claim that "nothing here is a second source of truth" is false.
- **Which side moves**: code.
- **Fix**: mirror the panel's visible-row cursor (or its selected `JobId`) into `AppState` on every
  cursor move and filter change, build `lists.jobs` from the same filtered row set the panel renders,
  and add the mirrored value to `ProjectionKey`.

### I5 — `dialog.fields` and `dialog.message` are always empty, and `absent` over them is an unconditional pass
- **Sources**: F1-1 + F2-5 (both reviewers) · **Verdict**: survives narrowed — high confidence
- **Problem**: `projection.rs:367` returns `fields: Vec::new(), … message: None` unconditionally,
  while §3 freezes `dialog` as `{name,fields:[{name,value,focused}],buttons:[string],message:string|null}`.
  §11 "Known gaps" — which is meant to record every place reality falls short of the frozen surface —
  has no dialog entry.
- **Impact**: the dangerous half is that every `assert dialog.fields[0] absent` and
  `dialog.message absent` passes unconditionally, including on a dialog that *does* show a populated
  field or an error message. Six dialogs paint clickable `dialog.field[N]` targets, so a scenario can
  click a field it can never read.
- **Narrowed**: the `buttons` third is **refuted** — §3's own target table already states that
  `dialog.button[N]` is painted nowhere, so an empty `buttons` is accurate. Strike it from the fix.
- **Which side moves**: doc (a §11 entry), unless the projection is wired up.
- **Fix**: add a §11 entry stating `dialog.fields` and `dialog.message` are always empty today and
  that dialog content is asserted via `targets["dialog.field[N]"]` plus keystrokes — or mirror the
  open dialog's field triples into `AppState` and add them to `ProjectionKey`.

### I6 — an interrupt arriving while a gate is open wedges the Codex player forever
- **Sources**: F1-3 · **Verdict**: 2/2 survives narrowed — high confidence; reproduced on both providers
- **Problem**: `turn/interrupt` during an open gate is dispatched into `answer_simple`, which sets
  `self.interrupted = true` and returns; the gate loop (`agent/codex.rs:562`, same shape
  `agent/claude.rs:498`) goes straight back to `read_frame` awaiting an answer that never comes. The
  `if self.interrupted` check lives in `run_turn`'s step loop (`codex.rs:329`), reached only after
  `ask` returns. `agent.rs:16-18` promises "a mid-stream `interrupt` is noticed at the next gate".
- **Evidence**: validator reproduced zero `result`/`turn/completed` frames, ever, on both providers.
  Both proposed refutations failed: neither adapter withdraws the gate (`claude/mod.rs:308`,
  `codex/mod.rs:505`), and the withdrawal frames travel the other way.
- **Narrowed**: the reviewer's trigger is **wrong** — `Esc` on an approval card is `DenyAndStop`
  (`agent_thread/actions.rs:315-324`, `keymap.rs:512`), which answers the gate, and `Esc` is the only
  caller. The reachable triggers are `fleet agent interrupt <thread>` (CLI, deterministic) and an
  `Esc` landing in the millisecond window before the app's mirror learns of the gate. "Forever" is
  also false for Claude: the 10 s `RESULT_DEADLINE` escalation closes stdin and the player bails.
  **Only Codex wedges indefinitely.**
- **Fix**: consult the flag inside both gate loops — in `codex.rs`, after `answer_simple` returns,
  `if self.interrupted { return Ok(Decision::Cancel); }`; in `claude.rs` take the flag after
  `answer_control` and return a withdrawn decision, setting `Settlement::Interrupted` at the call
  site so the turn still emits its terminal frame.

### I7 — `Peer::read_frame` has no deadline, so a stuck gate is an unbounded hang
- **Sources**: F1-4 · **Verdict**: survives (prover, direct repro) / survives narrowed (refuter) — high confidence
- **Problem**: `agent/peer.rs:51` is a bare `read_line` with no timeout, and the player runs inside
  `spawn_blocking` (`agent.rs:217`), so tokio cannot cancel it either. `agent.rs:210-214` documents
  `run_transcript` as *failing* when a gate is never answered; it hangs instead.
- **Evidence**: with the client half of a real socket pair still open, the player sits in `read_line`
  past a 3 s watchdog. Every existing test's `drive()` ends in EOF, which is why the suite cannot see it.
- **Impact**: this is the mechanism that turns I6 — and any future stuck-gate bug on the Fleet side —
  from a loud failure into a wedge. In a harness whose premise is determinism, the one process with no
  read deadline is the one speaking a provider protocol.
- **Fix**: give the gate wait a bounded budget sized like the scenario `await` default, settle the turn
  as `Interrupted` when it expires, and correct the doc comment if the budget lands elsewhere.

### I8 — a stale `interrupted` flag silently truncates a later turn
- **Sources**: F1-15 · **Verdict**: 2/2 survives — high confidence; reproduced
- **Problem**: the flag is consumed only inside the per-step loop, so an interrupt read by the
  between-turns dispatch loop (`claude.rs:54`, `codex.rs:69`) stays set and aborts the *next* turn
  after its first step.
- **Evidence**: `prompt₁, interrupt, prompt₂` reproducibly truncates turn two to
  `aborted_streaming`/`interrupted`. The daemon documents the triggering race itself at
  `fleet-daemon/.../manager/commands.rs:353-357` (Stop vs settle).
- **Impact**: non-deterministic truncation of a later turn — the worst failure mode for a harness,
  since it presents as a flaky agent scenario rather than as a harness bug.
- **Narrowed**: the finding's "an exhausted turn retains the flag indefinitely" clause is true but
  inconsequential (the playback cursor never rewinds) — strike it.
- **Fix**: consume the flag at the top of `run_turn`:
  `let mut settlement = if std::mem::take(&mut self.interrupted) { Settlement::Interrupted } else { script.settlement };`
  keeping the per-step check for the in-turn case.

### I9 — `align`'s trustworthiness guard is inert, so a lost journal line mis-attributes every later report row
- **Sources**: F1-7 · **Verdict**: survives (prover, repro) / survives narrowed (refuter) — high confidence on the defect, low on the trigger
- **Problem**: `report.rs:239`'s guard is `entry.line().is_none_or(|line| line == step.line)`, and
  `JournalEntry::line()` reads `data["line"]` — but `RunDirectory::record` (`rundir.rs:113`) writes
  `command` entries with `at`/`kind`/`request`/`response` and no `data` object. `command` is the kind
  covering almost every scenario line, so `line()` is always `None` and `trustworthy` never goes false.
- **Evidence**: validator's focused test — drop line 3's `command` entry and the report prints line 4's
  predicate and verdict under line 3, drops line 4 from `## Assertions`, and still says
  "Passed — 3 lines". A *lost* (as opposed to malformed) line leaves no trace at all, not even the
  `report.rs:202` footnote. The existing drift test (`report.rs:1555`) uses a `runner` entry — the one
  kind that does carry `data.line`.
- **Narrowed**: the impact chain has **no known trigger today**. `main.rs` has no `report` subcommand,
  so the journal is always parsed by the process that wrote it (the "newer binary can't deserialize"
  mode is impossible), and `append` is append-only and single-task, so a kill can only lose the tail —
  which sets `trustworthy = false` at the last row without misaligning anything earlier. The dead guard
  and its demonstrated consequence are real; the route to a dropped *middle* line is not established.
- **Fix**: give the command exchange a line number (`rundir.rs:113` gains `"data": {"line": line}`,
  threaded from `scenario.rs:488`) and tighten the filter to `entry.line() == Some(step.line)` so a
  missing line number is a mismatch, not a pass.

### I10 — `rail-collapse.scenario` never asserts the collapse it exists to pin, on a false premise
- **Sources**: F2-4 · **Verdict**: survives — high confidence
- **Problem**: the scenario's header (`scenarios/hub/rail-collapse.scenario:7`) says the rail width
  "is in the snapshot … but no predicate can read it, because a target name contains the `.` a dotted
  path splits on". That is false: §2 freezes a quoted-step form for exactly this, `parse_bracket`
  (`fleet-drive/src/predicate.rs:413`) implements it, and `scenarios/hub/pointer-tabs.scenario:13`
  already asserts `targets["repos.rail"].w == 240`. The rail target is painted on both branches (44/240).
- **Impact**: `H` is the one Hub affordance whose entire effect is geometry, and §3 names the rail's own
  `w` as its oracle. The scenario asserts only what a collapse *preserves* (rows, cursor, key context),
  all of which stay true if `H` stops collapsing altogether. The remaining evidence is a `shot`, and
  §11 records that no baseline has ever been recorded — so it is compared against nothing. The
  behaviour is effectively untested.
- **Which side moves**: the scenario.
- **Fix**: add `await targets["repos.rail"].w == 44` after the first `key H` and `… == 240` after the
  second; delete the incorrect paragraph from the header.

### I11 — a busy→idle transition does not notify, so `await idle` can sleep to its timeout
- **Sources**: F1-10 (+ independent corroboration from the Codex reviewer before it died)
- **Verdict**: 2/2 survives narrowed — medium-high confidence
- **Problem**: `drive::Harness::wait_for` is woken solely by `cx.observe(state)`, i.e. by `cx.notify()`.
  `InFlight`/`ArmedDebounce` decrement in `Drop` (`state/harness.rs:253`) with no notification, and
  `wait_for` has no fallback poll — so a decrement that is not followed by an unrelated notify leaves
  the waiter asleep until its timeout, then reports the last evaluated snapshot.
- **Narrowed**: the headline overstates it considerably. The mechanism is false for three of the five
  counters — `pending_frame` notifies (`shell/root/focus.rs`), `live_toast_timers` via `tick` +
  `spawn_ticker`, `running_jobs` via `apply_batch`. One cited location (`HarnessState::finish_request`)
  has **no production caller at all**, and both quoted early-return paths (`clone_repo.rs:284`,
  `hub/cache.rs:573`) are guarded and do not produce the hang. What survives is `InFlight::Drop` on the
  bridge thread and `schedule_inspection`'s no-target return.
- **Impact**: compounds I3 — the same primitive, failing in the other direction (a hang instead of an
  early return), and presenting as a Fleet bug rather than a harness one.
- **Fix**: make the decrement itself the notification — have `ArmedDebounce`/`InFlight` carry a weak
  `Entity<AppState>` and notify on drop, so every busy→idle edge raises the signal `wait_for` listens
  for.

### I12 — `idle` says nothing about the daemon link
- **Sources**: F1-12 · **Verdict**: survives (refuter could not refute it; one refutation backfired) /
  survives narrowed (prover) — high confidence on the structural claim
- **Problem**: all five counters are zero on a freshly launched Fleet whose bridge is still opening its
  connection (`state/harness.rs:184`); `DaemonLink::Starting` is not an input. So `idle` is
  structurally true on an unattached Fleet.
- **Narrowed**: the cold-start manifestation does **not** reproduce today, and the reviewer's stated
  reason for why it doesn't is itself wrong — the refuter found that `synchronize` issues no request
  while disconnected (`cache.rs:419-426`, `:583-593`), which makes the finding's mechanism stronger but
  its "it happens not to fire only because…" explanation false. The residue is a silent-green contract
  gap, not a live failure.
- **Impact**: the first `await idle` of a scenario can be satisfied on the cold-start splash, so the
  next line is the one that fails — `scenarios/daemon/first-run.scenario:16` and every
  `assert lists.* …` straight after a first `await idle`.
- **Fix**: fold "the bridge has not finished opening" into the `idle` derivation as a sixth input, or
  state in §2 that `idle` is silent about the link and have the corpus pair the first `await idle` with
  `await daemon.link == connected`, as `link-recovers.scenario` already does by accident.

---

## P3

### I13 — the run-directory socket-length guard measures the wrong socket
- **Sources**: F1-9 + F2-7 (both reviewers) · **Verdict**: survives — high confidence
- `rundir.rs:29`'s `LONGEST_SOCKET_NAME = "home/fleetd.sock"` (16 bytes) is documented as the longest
  socket path a run creates, but `env.rs:117` puts the app's socket at `<root>/fleet-harness.sock`
  (18 bytes). The uncovered window is exactly `len(root) ∈ {89, 90}`; neither socket has a fallback
  (only PTY sockets do). In that window the guard passes, `fleetd` starts, and Fleet's own `bind`
  fails with `ENAMETOOLONG`, surfacing after 60 s as "Fleet did not open … see app.log" — precisely
  the buried failure the guard exists to prevent.
- **Fix**: derive the constant from `HarnessEnv`'s two names so a rename cannot desynchronise them.

### I14 — two runs in the same second share one run directory
- **Sources**: F2-8 · **Verdict**: survives — high confidence
- `rundir.rs:55` names the default root `<UTC seconds>-<stem>` and `create_dir_all` treats an existing
  directory as success. `run_id()` is *not* used for the directory name (its own doc says why). The
  flock does not prevent the collision — it converts it into "run 2 fails **and** its `Daemon::drop`
  unlinks run 1's live socket", killing a healthy run.
- **Fix**: include the pid or millisecond precision in the directory name, or `create_dir` and retry
  with a suffix on `AlreadyExists`.

### I15 — `daemon restart` can leave a daemon the runner does not own
- **Sources**: F2-9 · **Verdict**: survives narrowed — medium-high confidence
- **Narrowed**: the `kill()` half is effectively **unreachable** — there is no `.await` between reap
  and `seal()`, the window is microseconds, and the app is not yet attempting a restart. The
  `restart()` half (`fault.rs:199`, between `unseal()` and `start_replacement()`) is a real ~1–5% race
  per line, but its consequence is weaker than claimed: `wait_until_ready` succeeds against the
  app-spawned daemon and teardown still stops it via the socket. What actually breaks is
  `Daemon::adopt`'s documented "cannot outlive the runner" guarantee.
- **Fix**: in `restart()`, spawn the replacement before unsealing, or hold the seal across the spawn.

### I16 — the fake `gh`/`acli` paste an unquoted path into a single-quoted shell literal
- **Sources**: F1-21 + F2-10 (both reviewers) · **Verdict**: survives as robustness, **not** security — high confidence
- `fixture/tools.rs:77` and `:101` substitute `data='@DATA@'` with no quoting guard, while
  `agent/launcher.rs`'s `shell_word` refuses exactly this case for the agent shim. The only input is
  the operator's own `--run-dir` and there is no privilege boundary, so this is **not** a security
  finding: the real bug is an ordinary apostrophe in a home directory breaking every `gh` invocation.
- **Fix**: route both templates through `shell_word`, or escape as `'\''`.

### I17 — `await idle exists` fails to parse, contradicting a test that pins the opposite
- **Sources**: F2-11 · **Verdict**: survives, stronger than argued — high confidence; reproduced
- `scenario.rs:1121`'s arm `["idle", _] => 1` matches any two-token atom starting with `idle`, so
  `await idle exists` is read as the bare `idle` clause plus a timeout of `"exists"`, failing with
  "invalid digit found in string". `fleet-drive/src/predicate.rs:996` has a test *named*
  `idle_is_a_path_when_it_is_followed_by_an_operator` pinning `idle exists` → `Clause::Exists`, so the
  two sides of the boundary disagree. `await idle exists 3000` parses fine, confirming the arm-ordering
  diagnosis exactly.
- **Fix**: match the two-token `idle` form only when the second token parses as a number, or check the
  `exists`/`absent` arm first.

### I18 — §5 says the `agents` fixture embeds three starter transcripts; it embeds two
- **Sources**: F1-16 + F2-12 (both reviewers) · **Verdict**: survives — high confidence
- `fn agents()` has only two `include_str!` sites (`fixture/plan.rs:455`, `:463`) — `two-turns.json`
  and `edit-approval.json`. `error-mid-stream.json` is used by a unit test and no preset, so no
  scenario can reach a provider error or the `failed` thread state.
  `scenarios/agents/blocked/error-mid-stream.blocked` already diagnoses this. P3 by impact, but
  non-optional under the repo's docs-are-authoritative rule.
- **Fix**: give the preset a third scripted provider carrying the transcript, or amend §5.

### I19 — `job success` hardcodes ordinal 0, so a second one in a scenario fails
- **Sources**: F1-20 · **Verdict**: survives (latent) — high confidence
- `fixture/jobs.rs:92` has one call site, pinned to `0`, producing the slug `injected-0` every time;
  `assert_create_conflicts` confirms the conflict, and `repeated()` avoids it only by deleting each
  time. The frozen grammar admits `job success` twice, so a scenario that reads correct cannot be
  written. No shipped scenario does this today.
- **Fix**: thread a per-run counter into `inject`, or derive the ordinal from existing `injected-*`
  worktrees.

### I20 — the hand-rolled PNG decoder panics on a hostile IHDR
- **Sources**: F1-5 · **Verdict**: survives narrowed — high confidence; reproduced
- A hand-built **69-byte** PNG (`width = height = 0xFFFFFFFF`, colour 6, depth 16, correct CRCs)
  panics at `baseline.rs:733:20` with `attempt to multiply with overflow`. Build profile confirmed:
  no `[profile.release]`, no `.cargo/config.toml`, `Makefile:10` `RELEASE ?= 0`, so `make harness`
  builds `dev` with `overflow-checks` on. `read_image`'s contract (`baseline.rs:594`) is to return a
  named `anyhow` error.
- **Narrowed**: the release-build half is **refuted** — a wrap needs `bytes_per_row ≥ 4 GiB`, so the
  first scanline slice at `:745` panics on `row == 0`; a release build can never silently accept
  garbage. The `update_baseline` (`:508`) reachability claim is also refuted: it decodes the fresh
  capture, never the stored baseline. The sole attacker-controlled decode is `compare`'s
  `read_image(baseline)` at `:385`, virtual lane only.
- **Severity**: P3, not P2 — the chunk CRC gate stops bit-flips, so this needs a deliberately crafted
  file, and the only carrier is a committed baseline on a branch you are already building and running.
  `scenarios/baselines/` holds only `README.md` and `.gitkeep`, and there is no `.github/workflows`
  at all, so nothing decodes PR-supplied pixels automatically. What justifies fixing it is the
  contract, not the threat: every other malformed shape in this decoder is a named error.

### I21 — `inflate` has no output bound
- **Sources**: F1-6 · **Verdict**: survives narrowed — high confidence; reproduced
- A 162 KB PNG whose IHDR says `1×1` inflates to 25.8 MB before `expand` rejects it, proving the size
  check runs only after `zlib_decompress` returns a complete `Vec` (`baseline.rs:1053`, back-reference
  copy `:1125`).
- **Narrowed**: the magnitude is **refuted** — DEFLATE's ceiling is 1032:1, so 1 MB → ~1 GB, not
  "hundreds of gigabytes". The decoder does accept the 1-bit distance code that ceiling needs, so
  1032:1 is genuinely reachable. Same reachability narrowing and same P3 severity rationale as I20.
- **Fix (I20 + I21 together)**: compute `expected` once with `checked_mul` in `u64`, thread it into
  `inflate` as a ceiling checked at each push site, and guard the `with_capacity` product at `:739`.
  Existing malformed-input coverage is only "not a PNG" and "truncated"; there is no hostile-IHDR or
  adversarial-stream test and no fuzz target.

### I22 — duplicate `tool_call` ids pass validation and collide on the wire
- **Sources**: F1-18 · **Verdict**: 2/2 survives (narrowed to latent) — high confidence
- `transcript::validate` (`transcript.rs:219`) enforces id uniqueness for `permission`/`approval`
  gates only, yet `claude.rs:144` passes `ToolCall::id` through as the `tool_use` block id. The
  daemon consequence is **worse** than reported: `claude/map/stream.rs:209-228` patches the existing
  row on an id hit rather than minting a second, so the second call gets **no row at all**, and
  `complete_item`'s early return swallows its completion.
- **Narrowed**: latent — the five `fixture:` presets are frozen and no scenario can supply a
  transcript, so nothing in the corpus can trigger it.
- **Fix**: include `TranscriptStep::ToolCall { id, .. }` in `validate`'s duplicate check.

### I23 — `command_actions` mis-parses a single-word command
- **Sources**: F1-19 · **Verdict**: 2/2 survives narrowed — high confidence; reproduced
- `codex.rs:760`: `words.next()` consumes the only word, so `words.next_back()` yields `""` —
  reproduced end to end as `{"type":"listFiles","command":"ls","path":""}`, a fabricated empty value
  rather than degrading to `unknown`. Blast radius is wider than reported (`rg`/`grep` too; `find`/`ls`
  take the last word, not the first argument). Unreachable from today's corpus.
- **Fix**: collect once and take `words.first()` / `words.get(1..)`, treating a program with no
  argument as `unknown`.

### I24 — a failed `shot`'s geometry exchange is missing from the journal
- **Sources**: F1-8 · **Verdict**: 2/2 survives narrowed — high confidence
- `scenario.rs:842`'s `ensure!(outcome.passed(), …)` runs before the exchange is recorded, so the
  app's settled geometry response (`drive.rs:331-343`) never reaches `run.jsonl`, against §8's "the
  journal is the complete record".
- **Narrowed**: the reviewer's collateral claims are **wrong** — `record_event("baseline", …)` and
  `context.artifact = Some(path)` both run *before* the `ensure!`, so the image, pixel counts, verdict
  and diff all still render; `shots_section` walks `self.steps`, not `command` entries; and alignment
  survives because `"error"` is in `EXCHANGES`. Only the geometry response is lost. Also latent: the
  `ensure!` fires only on `Differed`, which needs an existing baseline, and none exist.
- **Fix**: record the exchange before the `ensure!` and let the baseline failure ride as the step error.

### I25 — `--update-baselines` records nothing outside the virtual lane
- **Sources**: F1-2 · **Verdict**: 2/2 survives narrowed — high confidence
- `baseline.rs:343` returns `Skipped` for any non-`Virtual` lane before consulting `update`, and the
  documented mid-run `virtual`→`attach` fallback (`lane.rs:737` → `scenario.rs:457`) can put a run
  there without the developer choosing it.
- **Narrowed**: the behaviour is **deliberate and test-pinned** by
  `nothing_is_compared_or_recorded_outside_the_virtual_lane` (`baseline.rs:1723`, which passes
  `update=true`), the restriction is stated in `check`'s own doc comment, §6 scopes baselines to the
  virtual lane two paragraphs above the sentence the reviewer quoted, and the fallback is not silent
  (each shot journals `status: "skipped"`). The defect reduces to the unqualified promise at
  `docs/TESTING-HARNESS.md:410` plus a `Skipped` outcome that cannot distinguish "skipped comparison"
  from "refused to record".
- **Fix**: qualify §6's sentence and add a distinct `NotRecorded { lane }` outcome.

### I26 — the suite report inlines the magenta diff as if it were a screenshot
- **Sources**: F1-13 · **Verdict**: survives — high confidence; reproduced
- `pngs()` (`report.rs:1227`) excludes only the `failure-` prefix, but diffs are written as
  `<stem>-diff.png` into the same `shots/` directory (`baseline.rs:535`). The generated suite report
  inlines `003-help-diff.png` twice, and because `-` sorts before `.` the bare, unlabelled diff appears
  **first**, ahead of the real screenshot. The one existing test naming a diff (`report.rs:1434`)
  spells it `002-hub.diff.png` — a filename the runner never writes. Latent until a baseline exists.
- **Fix**: add an `is_diff` predicate beside `is_failure_evidence` and skip it at `report.rs:912`,
  `:918`, `:925`.

### I27 — `pngs()` swallows a directory-read error, silently omitting failure evidence
- **Sources**: F1-14 · **Verdict**: survives narrowed — medium-high on the defect, low on the trigger
- `report.rs:1232`'s `while let Ok(Some(entry)) = entries.next_entry().await` treats `Err` as
  end-of-directory — a `let _ =` on a fallible call wearing a `while let`.
- **Narrowed**: the validator **could not** make `next_entry()` return `Err` on a local filesystem. The
  sibling swallow at `report.rs:1228` *is* reachable and was proven: `shots/` at mode 000 makes
  `pngs()` return `[]` silently, dropping `failure-NNN.png` from both the suite failure block and the
  screenshots section. Only the suite report is affected — a failing scenario's own report finds the
  image by path existence.
- **Fix**: match the error and record it on the scenario so `failures_section` can say the screenshots
  could not be listed.

### I28 — headless `await` never repaints, freezing `targets` and `window.frame`
- **Sources**: F2-6 · **Verdict**: 2/2 survives narrowed — medium confidence
- `wait_for` (`drive.rs:651`) re-runs `project()`, which only *copies*
  `fleet_ui_kit::harness::painted(window)`; nothing paints headlessly unless `Harness::paint` is
  called, and it is called once, before the loop (`drive.rs:366`/`:482`).
- **Narrowed**: latent and unreached — `wait_for`'s own doc comment (`drive.rs:640-648`) already tells
  authors not to await on `targets`/`window`, the one headless scenario touching targets uses `assert`
  (which paints first), only three scenarios run headless, and three repro attempts came back negative.
  Only `window.frame` freezes, not the rest of `window`. Reduces to a §11 omission plus a doc comment
  that overclaims ("read as of the moment this call projects them").
- **Fix**: call `self.paint(cx)` at the top of each `wait_for` iteration when headless, or document the
  restriction in §11.

### I29 — target indices are model positions, and a scrolled-out row has no target
- **Sources**: F2-13 · **Verdict**: survives narrowed — medium-high confidence
- `worktrees_list.rs:347`, `prs_screen.rs:293`, `repos_rail.rs:317` and `jobs/presentation.rs:245` pass
  the index `uniform_list`/`gpui::list` hands the builder, which is the model index; only visible rows
  are painted, so a scrolled-out row has no target at all.
- **Narrowed**: §3's "current visual order" is an ambiguous sentence the reviewer over-reads; the
  substantive undocumented rule is the scrolled-out-row one.
- **Which side moves**: doc.

### I30 — `terminal.rows` drops trailing blank rows, so `rows.len() != viewport.rows`
- **Sources**: F1-22 · **Verdict**: survives narrowed — high confidence
- `state/terminal.rs:183` truncates trailing spaces and `:191` pops trailing empty rows; both are
  deliberate and *pinned by a test* (`harness/tests.rs:269-279` asserts `rows.len() == 1` with
  `viewport.rows == 2`).
- **Narrowed**: the reviewer's right-aligned-column impact is **wrong** — interior unset cells still
  become spaces, so column alignment does survive the round trip. Only the row-count half stands.
- **Which side moves**: doc — §3 should state both trims.

### I31 — `renamed_terminals` is a projection input missing from `ProjectionKey`
- **Sources**: F1-23 · **Verdict**: survives narrowed — medium confidence
- `projection.rs:641` branches on `self.renamed_terminals` to choose a tab label; `projection_key`
  (`:157-181`) lists eighteen inputs and not this one, against the module's own rule at `:30-32`.
- **Narrowed**: the refutation largely succeeded — all three mutators are paired with a keyed change
  (`mark_renamed` → `close_overlay()` flips five `Derived` fields in the same closure; `clear()` →
  `link_generation`; `retain()` → `snapshot_revision`), so the claimed hanging `await` is
  **unreachable on every in-tree path**. What survives is a letter-of-the-contract violation plus one
  narrow self-healing race (Esc during an in-flight rename).
- **Fix**: one-line key addition.

### I32 — `type` and `clipboard set` lose trailing whitespace
- **Sources**: F1-24 · **Verdict**: survives narrowed — high confidence
- `scenario.rs:762` (not 768, as the finding says) hands `raw.trim()` to `parse_instruction`, against
  §2:117's "arguments after `type` … preserve spaces".
- **Which side moves**: the **doc**, not the code — a trailing space in a committed scenario file
  survives no toolchain, so preserving it is not a property worth having.

### I33 — `fleet-drive`'s feature doc claims a build-time saving it does not deliver
- **Sources**: F1-25 · **Verdict**: survives narrowed — high confidence
- `lib.rs:5-8` says `fleet-lazygit` takes `legacy` alone so `regex` stays out, but `Cargo.toml:24` has
  `regex.workspace = true` un-gated.
- **Narrowed**: the reviewer's proposed fix **buys nothing** — `cargo tree` shows `gpui` pulls `regex`
  into `fleet-lazygit` on an independent normal edge, so `optional = true` saves zero build time.
- **Fix**: drop `regex` from the doc sentence. Doc only.

---

## Dropped or narrowed

**Dropped outright**

- **C14 / F1-17** — "`starter()` claims build-time validation it does not perform". The finding quotes
  half the doc comment; `fixture/plan.rs:471` names
  `agent::tests::all_three_starter_transcripts_load_and_validate`, which *does* deserialize into
  `Transcript` and run `validate` on all three embedded files, and it passes. No untrusted document can
  reach `install_agents`, and the `--transcript` CLI path validates. The residual — that the test
  enumerates names by hand — is below P3. Both validators agreed.

**Materially refuted sub-claims, struck from the issues above**

- **F1-11** (merged into I3) — "the in-flight claim is released *after* the reply is delivered".
  Refuted in both directions: it is a 3-instruction window on the runtime thread racing a full
  cross-thread wake plus effect flush plus JSON serialize, and its proposed fix would *widen* the
  proven defect in I3. Do not apply it.
- **I5** — the `buttons` third: §3's own target table already documents that `dialog.button[N]` is
  painted nowhere, so an empty `buttons` is accurate, not drift.
- **I6** — the trigger: `Esc` on an approval card is `DenyAndStop`, which answers the gate. Claude also
  self-heals on a 10 s deadline; only Codex wedges.
- **I8** — the "exhausted turn retains the flag" clause: true but inconsequential.
- **I9** — the mis-attribution has no established trigger: no `report` subcommand exists, and
  append-only journalling means a kill loses only the tail.
- **I11** — false for three of the five counters; one cited location has no production caller; both
  cited early-return paths are guarded.
- **I12** — the cold-start manifestation does not reproduce, and the finding's stated reason for that is
  itself wrong.
- **I20** — the release-build "silently accepts garbage" half, and the `update_baseline` reachability.
- **I21** — "hundreds of gigabytes"; the real ceiling is 1032:1.
- **I24** — the report does *not* lose the shot, its verdict, its diff, or its alignment.
- **I25** — the lane restriction is deliberate, test-pinned, documented in §6 and in `check`'s own doc
  comment, and the fallback is not silent.
- **I27** — the `next_entry()` error path could not be triggered on a local filesystem.
- **I28** — three repro attempts negative; `wait_for`'s doc already warns authors off.
- **I30** — the right-aligned-column impact is wrong; interior alignment survives.
- **I31** — the hanging `await` is unreachable; every mutation site is paired with a keyed change.
- **I33** — the proposed `optional = true` fix saves nothing.

**Severity lowered from the reviewer's call**: I20 and I21 (P2→P3, no automated decode of PR-supplied
pixels and no CI at all), I31 (P3→below, self-healing).

## Notes on coverage

Reviewers recorded these as checked and clean, so a later pass need not re-audit: socket framing,
`MAX_FRAME` enforcement and shutdown ordering in `fleet-drive/src/server.rs`; the three `let _ =`
additions (all fire-and-forget channel sends with the required comment); child reaping on error paths
in `Daemon`, `HyprlandBackend` and the seeding daemon; `ToastStack`'s render cap matching
`AppState::MAX_TOASTS`; `parse_click`'s count/point ambiguity.

Gaps worth noting: there is **no `.github/workflows` in the repo at all**, so none of this is gated by
CI; `scenarios/baselines/virtual/` is empty, so every `shot` in the corpus is currently compared
against nothing (§11 records this); and no `catch_unwind` exists in `fleet-harness`, so a decoder panic
skips `stage.teardown()` and `report::write_report` (`scenario.rs:350`, `:378`) and loses the run
directory.
