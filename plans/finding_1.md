# Independent review — `test-harness` vs `881162c5c283cefb4319bbfdb79629ef31960745`

Scope: `crates/fleet-drive`, `crates/fleet-harness`, `crates/fleet-app/src/drive.rs`,
`state/harness*`, `bridge/*`, `shell/root/*`, `crates/fleet-ui-kit/src/harness.rs`,
`crates/fleet-daemon/src/agents/harness/mockpeer.rs`, `Makefile`, `scenarios/`,
against the frozen `docs/TESTING-HARNESS.md`.

No P0. 8 × P2, 11 × P3.

---

### F1-1: `dialog.fields`, `dialog.buttons` and `dialog.message` are structurally always empty, so a dialog predicate can only ever be a false green
- **Severity**: P2
- **Location**: `crates/fleet-app/src/state/harness/projection.rs:367`
- **Problem**: `docs/TESTING-HARNESS.md` §3 freezes `dialog` as
  `{name,fields:[{name,value,focused}],buttons:[string],message:string|null}`, but the builder
  returns `fields: Vec::new(), buttons: Vec::new(), message: None` unconditionally — the dialog
  host entity is never read. §11 ("Known gaps") does not record this, so the doc still promises
  an oracle the app does not populate.
- **Impact**: Every `assert dialog.fields[0].value == …` is permanently false (a scenario that
  looks right can never pass), and — the dangerous half — every `assert dialog.fields[0] absent`
  / `dialog.message absent` passes unconditionally, including on a dialog that *does* show a
  populated field or an error message. `create_worktree`, `context`, `card_create`, `edit_hooks`
  and `clone_repo` all paint `dialog.field[N]` targets a scenario can click, so the asymmetry
  ("you can click the field but never read it, and reading it always says empty") is exactly the
  shape that produces a green test for a broken dialog.
- **Fix**: Either mirror the focused field/value/message from `dialogs::host` into the
  projection (the `ActiveDialog` entity is reachable from the same update path that already
  copies window metrics in `drive::Harness::project`), or add this to §11 as a verified limit and
  say in §3 that the three sub-fields are unpopulated today. The doc and the code must move in
  one commit — `docs/` is authoritative, not descriptive.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/state/harness/projection.rs:373
  Some(DialogSnapshot {
      name: dialog.context_name().to_owned(),
      fields: Vec::new(),
      buttons: Vec::new(),
      message: None,
  })
  ```
  `scenarios/hub/settings.scenario:6` already carries a comment about it, so the corpus knows
  and the frozen contract does not.

---

### F1-2: `--update-baselines` silently records nothing when the run is not on the `virtual` lane, including after a mid-run fallback
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/baseline.rs:343`
- **Problem**: `Baselines::check` returns `Skipped` for any lane other than `Virtual` *before* it
  looks at `update`, so `--update-baselines` is a no-op in `attach` and `headless`. §6 states
  flatly "`--update-baselines` explicitly replaces baselines", with no lane caveat.
- **Impact**: `make harness HARNESS_ARGS=--update-baselines` on a machine where the window
  refuses to move onto the isolated output — which `scenario.rs:340-344` documents as a
  *mid-run* `virtual`→`attach` fallback the developer did not choose — exits green having
  written nothing. The report then says "baseline: not compared in the attach lane", which is
  not "nothing was recorded". Given §11 says no baseline has ever been recorded, the first
  person to try to record one is the person this bites.
- **Fix**: When `update` is set and the lane is not `virtual`, return an error (or a distinct
  `NotRecorded { lane }` outcome whose summary says "not recorded in the <lane> lane"), and add
  the lane restriction to §6.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/baseline.rs:343
  if lane != Lane::Virtual {
      return Ok(BaselineOutcome::Skipped { lane: lane.as_str().to_owned() });
  }
  let baseline = self.baseline_for(lane, shot)?;
  if update { … }
  ```
  Reached from `scenario::dispatch`'s `Command::Shot` arm (`scenario.rs:836-841`) with
  `context.update_baselines` from the CLI flag.

---

### F1-3: A `turn/interrupt` that arrives while a gate is open is recorded and then ignored — the scripted agent blocks forever
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/agent/codex.rs:562` (same shape at
  `crates/fleet-harness/src/agent/claude.rs:498`)
- **Problem**: `ask` is the only stdin read during a gate. `turn/interrupt` is dispatched into
  `answer_simple`, which sets `self.interrupted = true` and returns; the loop then goes straight
  back to `read_frame`, waiting for a gate answer the client is never going to send. The
  `if self.interrupted` check lives in `run_turn`'s step loop (`codex.rs:329`), i.e. only after
  `ask` has returned.
- **Impact**: A scenario that presses Stop while an approval or permission card is up wedges the
  scripted provider: no `turn/completed` is ever emitted, the thread stays `working`, and the
  scenario fails on its `await` timeout with a message about the predicate rather than about the
  hang. `agent.rs:16-18` promises "a mid-stream `interrupt` is noticed at the next gate", which
  is exactly what does not happen. `scenarios/agents/edit-approval-deny.scenario:15` parks on
  that gate today; substituting the Stop affordance for `n` is a one-line change.
- **Fix**: Consult the flag inside both gate loops — in `codex.rs` after `answer_simple` returns,
  `if self.interrupted { return Ok(Decision::Cancel); }`; in `claude.rs` after `answer_control`,
  take the flag and return a withdrawn decision, and set `Settlement::Interrupted` at the call
  site so the turn still emits its terminal frame.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/agent/codex.rs:152
  "turn/interrupt" => { self.interrupted = true; json!({}) }
  // crates/fleet-harness/src/agent/codex.rs:562
  loop {
      let Some(frame) = self.peer.read_frame()? else { anyhow::bail!("…gate {gate:?} open"); };
      …
      } else { self.answer_simple(&id, method, &Value::Null)?; }   // sets `interrupted`, loops
  ```
  The regression test that should have caught it (`agent/tests.rs:427`) feeds the interrupt
  *and then an `allow`* (`tests.rs:442`), supplying an answer a real Fleet never sends.

---

### F1-4: `Peer::read_frame` has no deadline, so the scripted agent's documented failure mode is an unbounded block
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/agent/peer.rs:51`
- **Problem**: `read_frame` is a bare `read_line` with no timeout. `agent.rs:213` documents
  `run_transcript` as failing "when … a gate the client never answers and never withdraws" — it
  does not fail; it hangs. Because the player runs inside `spawn_blocking` (`agent.rs:217`), the
  tokio runtime cannot cancel it either.
- **Impact**: This is the mechanism that turns F1-3 (and any future stuck-gate bug on the Fleet
  side) from a loud failure into a wedge. In a harness whose whole premise is determinism, the
  one process with no read deadline is the one that speaks a provider protocol.
- **Fix**: Give the gate wait a bounded budget (a constant sized like the scenario `await`
  default), settle the turn as `Interrupted` when it expires, and correct the doc comment if the
  budget lands anywhere other than "fails".
- **Evidence**: `crates/fleet-harness/src/agent/peer.rs:51-70` — `self.reader.read_line(&mut line)`
  with no `set_read_timeout`/`select`. Callers: `claude.rs:500`, `codex.rs:563`, plus the
  between-turns dispatch loops at `claude.rs:54` and `codex.rs:69`.

---

### F1-5: `expand` computes the expected raw size with unchecked `usize` arithmetic — a crafted or corrupt PNG panics the runner
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/baseline.rs:733`
- **Problem**: `width` and `height` come straight out of IHDR (`parse_header` only rejects zero,
  `baseline.rs:691`). `(bytes_per_row + 1) * header.height as usize` and the `Vec::with_capacity`
  product at `baseline.rs:739` are unchecked; `width = height = 0xFFFF_FFFF` at colour type 6 /
  depth 16 overflows both.
- **Impact**: `make harness` builds through `make build` → the `dev` profile, which leaves
  `overflow-checks` on, so this is `attempt to multiply with overflow` — a panic out of
  `read_image`, whose contract (`baseline.rs:594`) is to return a named `anyhow` error. In a
  release build the wrap can make `raw.len() == expected` accept garbage, and line 739 then hits
  a capacity-overflow abort. Reached from `compare` on the committed baseline
  (`baseline.rs:385`) as well as on the run's own shot — and baselines arrive by pull request.
- **Fix**: Do the arithmetic in `u64` with `checked_mul`/`checked_add` and bail with a named
  error before allocating; drop the `with_capacity` hint or guard its product the same way.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/baseline.rs:732
  let bytes_per_row = (header.width as usize * bits_per_pixel).div_ceil(8);
  let expected = (bytes_per_row + 1) * header.height as usize;
  …
  let mut pixels = Vec::with_capacity(header.width as usize * header.height as usize * 4);
  ```
  The chunk CRC check at `baseline.rs:653` is the only gate before `parse_header`, and it is
  trivially satisfiable in a hand-built file.

---

### F1-6: `inflate` has no output bound, so a zlib-bomb PNG exhausts memory before `expand` can reject it
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/baseline.rs:1053`, back-reference copy at
  `crates/fleet-harness/src/baseline.rs:1125`
- **Problem**: `output` grows without a cap; the size sanity check lives in `expand`
  (`baseline.rs:734`), which runs only after the whole stream has been decompressed. A
  length-258 match costs about ten bits, so a 1 MB IDAT expands to hundreds of gigabytes.
- **Impact**: The runner is OOM-killed rather than failing the line, and the run directory — the
  evidence — is never written. Reachable through a committed baseline PNG (`baseline.rs:385`) or
  through `update_baseline`'s validating decode (`baseline.rs:508`).
- **Fix**: Thread the expected raw size (`(bytes_per_row + 1) * height`, computed safely per
  F1-5) into `inflate` and `anyhow::ensure!` after each block and inside the back-reference copy.
  `zlib_decompress` already holds `header` at its only call site (`baseline.rs:670`).
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/baseline.rs:1125
  for step in 0..length {
      let byte = output[start + step];
      output.push(byte);
  ```

---

### F1-7: `align`'s "verified against the line number" guard is inert for `command` entries, so one dropped journal line mis-attributes every later report row
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/report.rs:239`, reading
  `crates/fleet-harness/src/report.rs:170` and `crates/fleet-harness/src/rundir.rs:113`
- **Problem**: `align` pairs steps with journal entries positionally and claims to abandon the
  pairing "the moment the two disagree". The check is `entry.line().is_none_or(|line| line ==
  step.line)`, and `JournalEntry::line()` reads `data["line"]` — but `RunDirectory::record`
  writes `command` entries with `at`/`kind`/`request`/`response` and no `data` object at all.
  `command` is the kind for every app command, i.e. almost every line of a real scenario, so
  `line()` is always `None` and `trustworthy` never goes false.
- **Impact**: `Journal::read` deliberately tolerates a malformed line by dropping it and counting
  it (`report.rs:202`). One dropped `command` line — a partially flushed write when the runner is
  killed, a `Request`/`Response` shape a newer binary can no longer deserialize — shifts the
  pairing by one from that point on. `## Lines` then shows another line's "what happened",
  `## Assertions` shows another line's predicate under this line's number, and `## Dumps` links
  the wrong summary. The only trace is a footnote (`report.rs:652`) saying N lines were
  malformed; it does not say the tables above it are wrong. `report.rs:8-9` says a report that
  claimed something `run.jsonl` does not show "would be worse than no report at all".
- **Fix**: Give the command exchange a line number — `rundir.rs:113` gains
  `"data": {"line": line}`, threaded in from `scenario.rs:488` — and tighten the filter to
  `entry.line() == Some(step.line)` so a missing line number is a mismatch, not a pass.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/report.rs:236
  let paired = trustworthy.then(|| exchanges.next()).flatten()
      .filter(|entry| entry.line().is_none_or(|line| line == step.line));
  ```
  ```rust
  // crates/fleet-harness/src/rundir.rs:114
  self.append(serde_json::json!({ "at": …, "kind": "command", "request": request, "response": response }))
  ```
  The test that covers this (`report.rs:1555`) uses a `runner` entry — the one kind that *does*
  carry a line.

---

### F1-8: A `shot` that fails its baseline is never journalled, contradicting §8's "the journal is the complete record"
- **Severity**: P2
- **Location**: `crates/fleet-harness/src/scenario.rs:844`
- **Problem**: `dispatch`'s `Command::Shot` arm sends the command, captures, compares, and then
  `anyhow::ensure!(outcome.passed(), …)`. On a blown budget `dispatch` returns `Err`, so
  `execute`'s `(Err(error), _)` arm (`scenario.rs:519`) writes only an `"error"` event and the
  successful request/response for that `shot` never reaches `run.jsonl`.
- **Impact**: The one exchange a reviewer most wants after a pixel regression — the geometry and
  frame the app reported for the shot it took — is the one the "complete record" does not hold,
  and `report::shots_section` loses its `command` entry for that line. §8 says the report "is
  assembled only from what is already on disk, so it cannot describe something `run.jsonl` does
  not show"; here the journal does not show it.
- **Fix**: Record the exchange with `run_dir.record(&Request::new(response.id, command)?, &response)`
  before the `ensure!`, and let the baseline failure ride as the step error.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/scenario.rs:842
  context.artifact = Some(path);
  anyhow::ensure!(outcome.passed(), "{}", outcome.summary());
  ```

---

### F1-9: The run-directory socket-length guard measures the wrong path, so the app's own socket can still be too long to bind
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/rundir.rs:29`
- **Problem**: `LONGEST_SOCKET_NAME` is `"home/fleetd.sock"` (16 bytes past the root), but the
  run also binds `FLEET_HARNESS_SOCK` at `<root>/fleet-harness.sock` (18 bytes past the root),
  which is longer and has no `FleetHome`-style fallback.
- **Impact**: For a `--run-dir` whose length lands in the two-byte window the guard misses, the
  check passes, `fleetd` starts, and then `server::listen` fails inside Fleet with
  `ENAMETOOLONG`. The runner reports "Fleet did not open … within 60s; see app.log" — precisely
  the buried failure the guard exists to prevent, and now after a 60-second wait.
- **Fix**: Make the constant the actual longest, e.g. compare against
  `max("home/fleetd.sock", "fleet-harness.sock")`, or derive it from `HarnessEnv`'s own two
  names so a rename cannot desynchronise them.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/rundir.rs:29
  const LONGEST_SOCKET_NAME: &str = "home/fleetd.sock";
  // crates/fleet-harness/src/env.rs:117
  socket: root.join("fleet-harness.sock"),
  ```

---

### F1-10: No `idle` counter's busy→idle transition notifies `AppState`, so a decrement on an early-return path leaves `await` asleep until its timeout
- **Severity**: P3
- **Location**: `crates/fleet-app/src/state/harness.rs:253` (`ArmedDebounce::disarm`) and
  `crates/fleet-app/src/state/harness.rs:330` (`finish_request`)
- **Problem**: `drive::Harness::wait_for` is woken solely by `cx.observe(state)`, i.e. by
  `cx.notify()`. Of the five `idle` inputs, only `pending_frame` notifies when it clears
  (`shell/root/focus.rs`). `armed_debounces` and `in_flight_requests` are decremented by `Drop`
  with no notification anywhere, and the app has no fallback poll: `spawn_ticker`
  (`shell/root/events.rs:177`) only notifies when `AppState::tick` returns true, which on a
  connected app with no toasts is never.
- **Impact**: Any path that decrements and then returns without notifying makes the projection
  stale until something unrelated notifies. Concretely: `clone_repo.rs:284` disarms and then
  returns early when the search's reply channel closes (`search_owners`'s
  `Err(_) => return None`, `clone_repo.rs:365`) or when `live` is false; `hub/cache.rs:573`
  disarms and then returns on a dead entity or a stale generation. An `await idle` armed before
  that point sleeps to its timeout and then reports the *last evaluated* snapshot —
  `armed_debounces=1` — for a state that has been idle the whole time. The failure reads as a
  Fleet bug, not a harness one.
- **Fix**: Make the decrement itself the notification. Give `HarnessState` a seam that the
  Shell observes (or have `ArmedDebounce`/`InFlight` carry a weak `Entity<AppState>` and notify
  on drop) so every busy→idle edge raises the signal `wait_for` listens for. Failing that, add a
  cheap safety poll to `wait_for` — it currently has none, by design.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/state/harness.rs:253
  pub fn disarm(&mut self) {
      if let Some(counter) = self.0.take() { counter.fetch_sub(1, Ordering::AcqRel); }
  }
  ```
  ```rust
  // crates/fleet-app/src/dialogs/clone_repo.rs:284
  armed.disarm();
  if cx.update(|cx| { … stale seq … }) { return; }          // no notify
  let Some(SearchResults { … }) = search_owners(…).await else { return; };  // no notify
  ```

---

### F1-11: The in-flight claim is released *after* the reply is delivered, so a fast foreground can project a stale `in_flight_requests`
- **Severity**: P3
- **Location**: `crates/fleet-app/src/bridge/requests.rs:122`
- **Problem**: In `dispatch`'s spawned task the order is `reply.send(result).await` and *then*
  the end of the async block, which is where `_in_flight` drops. The same ordering holds on the
  offline early return (`requests.rs:119`, `try_send` then `return`). The decrement therefore
  happens strictly after the waiter has been woken.
- **Impact**: The foreground task can wake, apply the response, `cx.notify()`, and have
  `wait_for` re-project before the runtime thread has decremented — reading
  `in_flight_requests = 1` for a request that is finished. Combined with F1-10 (nothing notifies
  on the decrement), that evaluation is the last one an `await idle` ever makes, and the command
  times out. The window is sub-microsecond against a cross-thread wakeup, so this is rare rather
  than routine — but it is the one `idle` race that needs no early-return path to reach.
- **Fix**: Drop the claim before handing the answer over — `drop(in_flight);` immediately before
  `reply.send(result).await` (and before the `try_send` on the offline path). The claim's job is
  "a request is outstanding", and it stops being outstanding when the answer exists.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/bridge/requests.rs:122
  tokio::spawn(async move {
      let _in_flight = in_flight;
      let result = client.request(body).await;
      …
      let _ignored = reply.send(result).await;
  });                                     // `_in_flight` drops here, after the wake
  ```

---

### F1-12: `idle` says nothing about the daemon link, so `await idle` can be satisfied on a cold start before Fleet has attached
- **Severity**: P3
- **Location**: `crates/fleet-app/src/state/harness.rs:184`
- **Problem**: All five counters are zero on a freshly launched Fleet whose bridge is still
  opening its connection: `in_flight_requests` counts only *app* requests (the bridge's own
  `ensure_daemon` is invisible to it), `running_jobs` is 0 because `self.snapshot` is `None`, and
  the rest are structurally zero. `DaemonLink::Starting` is not an input.
- **Impact**: The first `await idle` of every scenario can return while the app is still on the
  cold-start splash. It happens not to today only because the Hub's deferred `synchronize`
  admits a request before the socket is connectable — an ordering nothing pins. When it slips,
  the next line is the one that fails: `scenarios/daemon/first-run.scenario:16`
  (`assert daemon.link == connected`) and every `assert lists.* …` immediately after an
  `await idle` are all reading a state that may not have arrived.
- **Fix**: Either fold "the bridge has not finished opening" into the `idle` derivation as a
  sixth input (it is a property of pending work, which is what `idle` means), or say in §2 that
  `idle` is silent about the link and make the corpus pair the first `await idle` with
  `await daemon.link == connected`, as `link-recovers.scenario` already does by accident.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/state/harness.rs:192
  idle: in_flight_requests == 0 && running_jobs == 0 && !pending_frame
      && live_toast_timers == 0 && armed_debounces == 0,
  ```
  `Shell::new` (`shell/root.rs:78`) starts the bridge but issues no request itself; the first
  claim comes from `HubScreen::bind`'s `cx.defer(… synchronize …)` (`screens/hub.rs:353`).

---

### F1-13: The suite report inlines `NNN-name-diff.png` a second time, unlabelled, as if it were a screenshot of Fleet
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/report.rs:923`, feeding off `pngs()` at
  `crates/fleet-harness/src/report.rs:1227`
- **Problem**: `pngs()` returns every PNG under `shots/`, and the only exclusion is the
  `failure-` prefix. `baseline.rs:535` writes diff images into that same directory as
  `<stem>-diff.png`, and `baselines.get(file_name(diff))` misses because the map is keyed by the
  shot's name.
- **Impact**: A scenario that blew its baseline budget renders the shot, its verdict, the magenta
  diff (via `render_baseline`), and then the diff *again* as a bare entry in the same list. A
  reader sees a magenta-speckled image presented as a screenshot of the application.
- **Fix**: Add an `is_diff` predicate beside `is_failure_evidence` (`report.rs:1204`) and skip it
  at `report.rs:912`, `918` and `925`. The single-run `shots_section` (`report.rs:553`) is
  already safe because it walks `self.steps` rather than the directory.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/report.rs:923
  for shot in &scenario.shots {
      if is_failure_evidence(shot) { continue; }
  ```

---

### F1-14: `pngs()` cannot tell an IO error from the end of the directory, so a suite report can silently omit failure evidence
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/report.rs:1232`
- **Problem**: `while let Ok(Some(entry)) = entries.next_entry().await` treats an `Err` exactly
  like end-of-directory. This is a `let _ =` on a fallible call wearing a `while let`.
- **Impact**: A suite report can omit a failed scenario's `failure-NNN.png` from both the failure
  block (`report.rs:858`) and the screenshots section, with nothing said about it — the report
  looks complete and is not.
- **Fix**: `match` the error, record it on the scenario so `failures_section` can say the
  screenshots could not be listed, or at minimum `eprintln!` it the way `lane.rs` does for its
  own best-effort steps.
- **Evidence**: `crates/fleet-harness/src/report.rs:1232`.

---

### F1-15: `self.interrupted` survives a turn boundary and an exhausted turn, silently truncating a later turn
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/agent/claude.rs:213`,
  `crates/fleet-harness/src/agent/codex.rs:329`
- **Problem**: The flag is consumed only by a check *inside* the per-step loop. An interrupt read
  by the between-turns dispatch loop (`claude.rs:54`, `codex.rs:69`) stays set and aborts the
  *next* turn after its first step; on an exhausted turn (`transcript.rs:79` returns an empty
  step slice) the loop body never runs, so the flag is retained indefinitely.
- **Impact**: `ClaudeHarness::interrupt` writes the frame whenever `session.active_turn() ==
  Some(turn)`, which a Stop pressed between the player emitting `result` and the daemon
  processing it satisfies. The scenario then sends its next prompt and gets a truncated,
  `aborted_streaming` turn — non-deterministically, which is the worst possible failure mode for
  a harness.
- **Fix**: Consume the flag at the top of `run_turn`:
  `let mut settlement = if std::mem::take(&mut self.interrupted) { Settlement::Interrupted } else { script.settlement };`
  and keep the per-step check for the in-turn case.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/agent/claude.rs:213
  if self.interrupted { self.interrupted = false; settlement = Settlement::Interrupted; break; }
  ```

---

### F1-16: `error-mid-stream.json` is documented as embedded by the `agents` preset and is embedded by nothing
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/fixture/plan.rs:424`
- **Problem**: §5 says the three starter transcripts "are what the `agents` fixture embeds".
  `agents()` wires two — `two-turns.json` for Claude (`plan.rs:453`) and `edit-approval.json` for
  Codex (`plan.rs:461`). `error-mid-stream.json` is referenced only from `agent/tests.rs`.
- **Impact**: No scenario can reach a provider error or the `failed` thread state — the
  `agents` corpus records this as `scenarios/agents/blocked/error-mid-stream.blocked` — and the
  frozen `fixture:` vocabulary has no sixth preset to hang a third transcript on, so a scenario
  author cannot fix it from the corpus side.
- **Fix**: Append a failing third turn to `two-turns.json` (the blocked file already spells the
  three steps out, and `next_turn`'s exhausted-turn behaviour keeps turns one and two
  byte-identical), or amend §5 to say the preset embeds two of the three.
- **Evidence**: `crates/fleet-harness/src/fixture/plan.rs:453-466` vs
  `docs/TESTING-HARNESS.md:359-361`.

---

### F1-17: `starter()` claims build-time validation it does not perform, so a structurally invalid preset transcript reaches the daemon
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/fixture/plan.rs:472`
- **Problem**: The comment says "a malformed one is a build-time fact rather than a runtime
  condition". `include_str!` is compile-time but `serde_json::from_str::<Value>` is not, and it
  checks syntax only — the document is never deserialized into `Transcript` nor run through
  `transcript::validate`. `tools::install_agents` (`tools.rs:111`) then writes the `Value`
  verbatim onto the run's PATH.
- **Impact**: An unknown step type, an `approval` not adjacent to its `file_change`, or a step
  after `exit` ships into the run and fails at `agent::load`, which the daemon reports as a
  harness that died at session start — the confusing failure §5's load-time validation exists to
  prevent.
- **Fix**: Deserialize into `Transcript` and run `transcript::validate` in `install_agents`,
  naming the preset in the error.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/fixture/plan.rs:472
  serde_json::from_str(document)
      .unwrap_or_else(|error| panic!("the embedded transcript {name} is not valid JSON: {error}"))
  ```

---

### F1-18: Duplicate `tool_call` ids pass validation and collide on the wire
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/agent/transcript.rs:219`
- **Problem**: `gate_id` enforces uniqueness for `permission`/`approval` ids only, yet
  `claude.rs:144` passes a `ToolCall::id` straight through as the `tool_use` block id.
- **Impact**: Two `tool_call` steps sharing an id emit two `tool_use` blocks with the same
  provider item id; the daemon keys items by that id, so the second tool result lands on the
  first row and a transcript-driven assertion reads the wrong item. `agent.rs:95` calls the field
  "the gate-free correlation key", which it cannot be if it is not unique.
- **Fix**: Include `TranscriptStep::ToolCall { id, .. }` in `validate`'s duplicate check.
- **Evidence**: `crates/fleet-harness/src/agent/transcript.rs:219-226`;
  `crates/fleet-harness/src/agent/claude.rs:144`.

---

### F1-19: `command_actions` mis-parses a single-word command
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/agent/codex.rs:760`
- **Problem**: `let program = words.next()` consumes the only word, so `words.next_back()` on the
  exhausted iterator yields `""`.
- **Impact**: `{"type":"tool_call","name":"ls","arguments":null}` emits
  `{"type":"listFiles","command":"ls","path":""}` — a shell row rendering with an empty path
  rather than degrading to `unknown`, so a scenario asserting on the row sees a fabricated empty
  value.
- **Fix**: Collect once (`let words: Vec<&str> = command.split_whitespace().collect();`) and take
  `words.first()` / `words.get(1..)`, treating a program with no argument as `unknown`.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/agent/codex.rs:762
  let program = words.next().unwrap_or_default();
  let argument = words.next_back().unwrap_or_default();
  ```

---

### F1-20: `job success` hardcodes ordinal 0, so a second one in the same scenario fails
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/fixture/jobs.rs:92`
- **Problem**: `success` takes an `ordinal` that every call site pins to `0`, producing the
  worktree slug `injected-0` every time.
- **Impact**: The frozen grammar allows `job success` twice (`scenario.rs:964` parses each line
  independently), and the second one fails with "inject a successful job as injected-0: … already
  exists" rather than producing a second toast — a scenario that reads correct cannot be written.
  No shipped scenario does this today.
- **Fix**: Thread a per-run counter into `inject`, or derive the ordinal from the number of
  existing `injected-*` worktrees.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/fixture/jobs.rs:92
  Injected::Success => one(success(client, fixture, 0).await?),
  // crates/fleet-harness/src/fixture/jobs.rs:109
  let slug = format!("{SLUG_PREFIX}-{ordinal}");
  ```

---

### F1-21: The fake `gh`/`acli` paste an unquoted path into a single-quoted shell literal
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/fixture/tools.rs:77` and
  `crates/fleet-harness/src/fixture/tools.rs:101`
- **Problem**: `GH.replace("@DATA@", &data.to_string_lossy())` expands into `data='@DATA@'`. A run
  root containing a single quote produces a `/bin/sh` script with an unterminated string.
  `agent/launcher.rs:120` refuses exactly this case for the agent shim; the tool shims do not.
- **Impact**: Every `gh` invocation exits non-zero with a shell parse error, so repository
  discovery and every PR surface fail with a message that points at `gh` rather than at the
  fixture. Reachable through `--run-dir`, which is not sanitised the way the default name is.
- **Fix**: Reuse `launcher::shell_word`'s check (make it `pub(crate)`) and refuse, or escape as
  `'\''`.
- **Evidence**: `crates/fleet-harness/src/fixture/tools.rs:77`, expanding `GH:197`.

---

### F1-22: `terminal.rows` trims trailing spaces and drops trailing blank rows, contradicting §3's round-trip promise
- **Severity**: P3
- **Location**: `crates/fleet-app/src/state/terminal.rs:169`
- **Problem**: §3 says `rows` "renders an unset cell as a space, so column alignment survives the
  round trip". `harness_row` then truncates to `text.trim_end_matches(' ').len()`, and
  `harness_rows` pops every trailing empty row — so `terminal.rows.len()` does not equal
  `terminal.viewport.rows`, and right-hand column alignment does not survive.
- **Impact**: A predicate that pins a right-aligned column, or one that indexes a row by its grid
  position near the bottom of the screen (`terminal.rows[23] == ""`), reads a missing path and
  evaluates false — a scenario that cannot pass, written against the documented behaviour.
- **Fix**: Either keep the padding (and let `terminal.text`'s join carry it) or amend §3 to say
  both trims explicitly. The `viewport.rows` field should agree with `rows.len()` either way.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/state/terminal.rs:183
  text.truncate(text.trim_end_matches(' ').len());
  // crates/fleet-app/src/state/terminal.rs:191
  while rows.last().is_some_and(String::is_empty) { rows.pop(); }
  ```

---

### F1-23: `renamed_terminals` is read by the projection but is not in `ProjectionKey`
- **Severity**: P3
- **Location**: `crates/fleet-app/src/state/harness/projection.rs:641`
- **Problem**: `terminal_row` branches on `self.renamed_terminals.contains(&terminal.id)` to
  choose the tab label, but `projection_key` does not include that set. The module's own doc
  (`projection.rs:30-32`) says "a key that misses an input is a stale snapshot, which makes
  `await` hang until its timeout".
- **Impact**: `AppState::mark_renamed` (`state/terminal.rs:230`) is called from the rename
  dialog's response handler. If the local mark lands before the daemon's snapshot does, the
  projection serves the old label until an unrelated input moves the key — an `await
  lists.tabs.rows[0].label == …` that waits out its timeout for a rename that already happened.
- **Fix**: Add `renamed_terminals: self.renamed_terminals.clone()` (or its length plus a
  revision) to `ProjectionKey`.
- **Evidence**:
  ```rust
  // crates/fleet-app/src/state/harness/projection.rs:641
  let label = if self.renamed_terminals.contains(&terminal.id) { terminal.name.clone() } else { … };
  ```
  `projection_key` (`projection.rs:157-181`) lists eighteen inputs; this is not one of them.

---

### F1-24: `type <text>` loses trailing whitespace, against §2's "arguments after `type` … preserve spaces"
- **Severity**: P3
- **Location**: `crates/fleet-harness/src/scenario.rs:901`
- **Problem**: `parse` hands `raw.trim()` to `parse_instruction` (`scenario.rs:838`), so a
  scenario's `type foo   ` has its trailing spaces removed before the argument is taken. Interior
  spaces survive; leading ones are eaten by `split_command`'s `trim_start`.
- **Impact**: A scenario that types trailing whitespace into a filter or a composer — the one
  case where the distinction is testable — cannot express it. The same is true of
  `clipboard set`, which §2 names in the same sentence.
- **Fix**: Pass the untrimmed line (minus a trailing `\r`) to the `type` and `clipboard set`
  arms, or state the trim in §2.
- **Evidence**:
  ```rust
  // crates/fleet-harness/src/scenario.rs:768
  let trimmed = raw.trim();
  …
  instruction: parse_instruction(trimmed)
  ```

---

### F1-25: `regex` is an unconditional dependency of `fleet-drive`, so the `legacy`-only consumer still builds it
- **Severity**: P3
- **Location**: `crates/fleet-drive/Cargo.toml:24`
- **Problem**: `crates/fleet-drive/src/lib.rs:5-8` states that `fleet-lazygit` takes `legacy`
  alone "so `regex`, the predicate evaluator and the server loop stay out of a binary that drives
  nothing over a socket". The two modules are correctly `#[cfg]`-gated, but `regex.workspace =
  true` is not `optional`, so `cargo build -p fleet-lazygit` compiles it regardless.
- **Impact**: Build time only — the crate is dead-stripped from the binary. The doc claim is
  simply not true of one of its three subjects.
- **Fix**: `regex = { workspace = true, optional = true }` and `socket = ["legacy", "dep:regex"]`,
  or drop `regex` from the sentence.
- **Evidence**: `crates/fleet-drive/Cargo.toml:24`, `crates/fleet-drive/src/lib.rs:5`.

---

## Checked and clean

Recorded so a later pass does not re-audit them.

- **Socket framing and shutdown ordering** (`fleet-drive/src/server.rs`). `MAX_FRAME` is enforced
  before the frame is parsed, the overrun is drained so the connection stays framed, the read and
  write timeouts are both armed, `accept` retries only on genuinely transient errors, and
  `run()`'s `drop(channel); drop(server);` is the order `Server`'s doc requires — which the
  tuple destructure in the cancelled-future case also produces (reverse declaration order).
- **Correlation and desynchronisation** (`fleet-harness/src/client.rs`). `read_line`'s
  cancel-unsafety is handled by owning `pending` across calls, a timeout poisons the connection
  rather than letting a late answer correlate to a newer id, `budget()` adds each command's own
  advertised delay, and a closed socket is only success for `quit`.
- **Predicate grammar** (`fleet-drive/src/predicate.rs`). Every index into `parts` after an arm
  match is provably in range, `bracket_span` and `parse_bracket` terminate on malformed input,
  `scalar_eq`/`numeric_pair` are total, and `predicate_and_timeout`'s four arms each fix the
  atom's length exactly. `await a == b && c == d 3000` splits correctly; `click 100 200` is *not*
  mis-read as a repeat count, because `parse_location_tokens(["200"])` fails.
- **Tolerance semantics** (`fleet-harness/src/baseline.rs`). `> threshold` makes a per-channel
  delta of exactly 8 pass, `ratio <= 0.002` makes exactly 0.2% pass, the denominator is
  `width × height`, alpha is compared, and a size mismatch is an error rather than a pixel count
  in both `compare` and `differing_share_within`.
- **The empty-output guard** (`lane.rs` + `baseline.rs`) is fail-closed on every degenerate
  region: an off-image or empty region yields `Ok(0.0)`, which `lane.rs:506` reads as "the window
  is not there". The mask index is provably in range. One nit: the denominator is the *clipped*
  area while the failure message quotes the unclipped `region.width × region.height`.
- **The deflate/PNG reader** has no reachable panic besides F1-5: `Bits::take`, `Huffman::build`,
  `dynamic_codes`, the back-reference bounds, `unfilter`, `sample`/`scale` and the palette lookup
  are all bounded. Path traversal via the shot name is closed by `sanitize`.
- **`fleet_ui_kit::harness`** adds no layout node, branches on one thread-local `bool` before
  touching a name, clears rather than reallocates per frame, and `AppFrame` is a genuine
  once-per-frame boundary. `TargetName::Indexed`'s `format!` and `views::harness::name`'s closure
  both run only while recording.
- **Target indices match list indices**: `worktrees.row[N]`, `prs.row[N]`, `repos.row[N]` and
  `jobs.row[N]` are stamped with the virtualized list's absolute index, and `toasts.toast[0]` is
  the oldest because `AppState` caps at `ToastStack::MAX` and the stack takes `.rev().take(3).rev()`.
- **`advance_clock`** matches §1 exactly in what it moves and what it does not, and `rewind`
  cannot underflow.
- **Teardown is unconditional and ordered**: injections, then `fault::resume` (so a `daemon stop`
  without a `cont` cannot eat the grace period), then `quit`, then the app, then `fleetd`, then
  the lane — with a detached watchdog outside the process for the isolated output.
- **Hermetic environment**: private `FLEET_HOME`, child-only `HOME` with XDG below it, fake
  binaries first on PATH, `kill_on_drop` on both children, and a reaping `Drop` on `Daemon`. The
  seeding path shuts its private `fleetd` down on both the success and failure branch.
- **The headless selector cannot drift**: `Makefile`'s `HARNESS_PIXEL_BOUND` and
  `crates/fleet-app/tests/harness_headless.rs:35` apply the same two-directive rule to the same
  extension set, and the test fails rather than passing when the rule selects nothing (3 of 41
  scenarios are selectable today, all excluded for `shot` and none for `clipboard`).
- **No `unwrap`/`todo!`/`unimplemented!`/`dbg!` in any new production path**, and every
  `let _ =` is a fire-and-forget channel send carrying the comment the rule asks for.
- **Nothing heavy in a render path**: `harness_projection` is reachable only from `dump`,
  `await` and `assert`, all of which run in update paths; the memo holds its revision when the
  projected content is unchanged, so a repainted frame does not wake a waiting `await`.
