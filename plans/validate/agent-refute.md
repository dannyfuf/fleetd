# Adversarial validation — batch `agent` (C11–C16)

Scope: `crates/fleet-harness/src/agent/` (the scripted Claude and Codex providers), checked
against their real callers in `crates/fleet-daemon/src/agents/`, the app's Stop path in
`crates/fleet-app/src/screens/agent_thread/`, `crates/fleet-harness/src/agent/tests.rs`,
`docs/TESTING-HARNESS.md` §4/§5/§11 and `docs/KEYMAP.md`.

`cargo test -p fleet-harness --lib agent::` — 23 passed, 0 failed.

---

### C11 (F1-3) — verdict: SURVIVES NARROWED

- **Confidence**: high

- **Reasoning**

  The mechanism is exactly as described, and the two obvious refutations both fail.

  *Refutation 1 — "the client withdraws the gate first" — fails.* Neither adapter withdraws.
  `ClaudeHarness::interrupt` (`crates/fleet-daemon/src/agents/claude/mod.rs:308-318`) writes
  `{"subtype":"interrupt"}` and nothing else; `CodexHarness::interrupt`
  (`crates/fleet-daemon/src/agents/codex/mod.rs:505-523`) calls `interrupt_fanout`, which sends
  `turn/interrupt` and returns. There is no cancel/withdraw method on either gate. In fact the
  withdrawal frames travel the *other* way — `control_cancel_request` is decoded as an **inbound**
  frame from the CLI (`crates/fleet-daemon/src/agents/claude/frames.rs:241`, "the CLI stopped
  waiting. **Answer nothing.**") and `serverRequest/resolved` is likewise server→Fleet
  (`crates/fleet-daemon/src/agents/codex/map/threads.rs:329-334`, "The user interrupted, or
  Codex's own auto-reviewer answered it"). So a **real** CLI resolves its own pending approval when
  it is interrupted, and the scripted player is the only peer that does not. That makes the
  finding stronger, not weaker: `claude.rs:511`'s `Decision::Withdrawn` branch is dead code for
  real Fleet, because Fleet never sends `control_cancel_request`.

  *Refutation 2 — "a test already covers it" — fails.* The reviewer's own note is right:
  `agent/tests.rs:427 a_claude_interrupt_is_receipted_and_aborts_the_turn_it_arrives_in` feeds the
  interrupt **and then an `allow`** (`tests.rs:436-443`). Real Fleet never sends a gate answer
  after a Stop. The test asserts the receipt and the `aborted_streaming` result, both of which only
  happen because the synthetic `allow` released the gate loop. It is a green test over an
  unrealistic frame sequence.

  *What does not hold is the stated trigger and the stated permanence.*

  **The trigger is guarded.** "Pressing Stop while an approval card is up" cannot produce a bare
  interrupt. `AgentThreadView::stop` — the *only* caller of `BridgeCommand::AgentInterrupt`, and
  itself reachable only from `MultilineInputEvent::Escape`
  (`crates/fleet-app/src/screens/agent_thread/mod.rs:546`) — short-circuits on an open permission
  gate. `docs/KEYMAP.md:245,249,250` records the same thing: on `AgentDecision > AgentPermission`,
  `Esc` is "deny and stop", and that context outranks `AgentWorking`. Permission and Approval are
  the only two gate kinds a transcript can open (`TranscriptStep` in `agent.rs:80-140` has no
  question/plan step), and both map to `ComposerMode::Approval`
  (`screens/agent_thread/mod.rs:343-344`). There is no Stop button, no CLI `fleet agent interrupt`,
  and no scenario command for it (`scenario.rs` parses only Key/Click/Type/… — `Command::` has no
  interrupt arm).

  The one path that remains is a **projection race**: between the player emitting the approval
  request and the app rendering the card, `composer_mode()` is not yet `Approval` and
  `is_working()` is still true, so `Esc` in that window dispatches `AgentInterrupt`. The daemon
  admits it because `interrupt()` only checks `active_turn`. That window is one event round-trip
  wide — which in a harness whose premise is determinism is still a defect, just not the one
  described.

  **"Forever" is false for Claude.** `crates/fleet-daemon/src/agents/claude/mod.rs:48` sets
  `RESULT_DEADLINE = 10 s`, and the escalation task at `mod.rs:320-371` closes the CLI's stdin,
  marks every open item `Stopped` and emits `TurnAborted`. Closing stdin makes the player's
  `read_frame` return `Ok(None)`, so `ask` bails with "the client closed the connection with gate
  {gate} open" (`claude.rs:498-500`) and the process exits. The thread does **not** stay `working`;
  it settles as aborted after ten seconds and the harness dies loudly. Codex has no equivalent —
  `grep -rn "escalat\|watchdog" crates/fleet-daemon/src/agents/codex/` finds nothing — so only the
  Codex half wedges, and even there the card stays on screen, so a `click
  agents.approval.allow_once` still releases it.

- **Evidence**

  ```rust
  // crates/fleet-app/src/screens/agent_thread/actions.rs:319
  // An approval's `esc` is `deny and stop`, which is a different thing from an interrupt
  // and is reachable on both harnesses.
  if matches!(self.composer_mode(), ComposerMode::Approval(_)) {
      self.act_on_decision(&DecisionAction::DenyAndStop, cx);
      return;
  }
  ```
  ```rust
  // crates/fleet-daemon/src/agents/claude/mod.rs:48
  const RESULT_DEADLINE: Duration = Duration::from_secs(10);
  ```
  ```rust
  // crates/fleet-daemon/src/agents/codex/map/threads.rs:329
  /// `serverRequest/resolved`: a pending server request no longer needs an answer.
  /// The user interrupted, or Codex's own auto-reviewer answered it.
  ```

- **The reduced claim that holds**

  The gate loops (`codex.rs:560-582`, `claude.rs:496-518`) set `interrupted` and then keep
  blocking, so an interrupt that lands while a gate is open is not acted on until the gate is
  answered — a fidelity gap against both real CLIs, which resolve their own pending request on
  interrupt. It is reachable only through the narrow window in which the gate is open on the
  provider but not yet projected into the app, and it wedges only the Codex player; the Claude
  player is unwedged after 10 s by the daemon's escalation, which kills it with an error rather
  than a hang. Neither `docs/TESTING-HARNESS.md` §5 nor §11 records any of this.

- **The part that does not hold**

  "Pressing Stop while an approval card is up wedges the provider forever" — the app converts that
  exact gesture to `DenyAndStop`, and for Claude the daemon escalates in ten seconds. The
  suggested one-line rewrite of `scenarios/agents/edit-approval-deny.scenario:15` ("substituting
  the Stop affordance for `n`") would **not** reproduce the bug: on an open permission gate `Esc`
  is `DenyAndStop`, which the player already handles (`claude.rs:169-176`, `codex.rs:263-273`).

---

### C12 (F1-4) — verdict: SURVIVES NARROWED

- **Confidence**: high

- **Reasoning**

  `Peer::read_frame` (`agent/peer.rs:51-68`) is a bare `BufRead::read_line` with no
  `set_read_timeout` and no `select`; that part is simply true, and the doc at
  `agent.rs:210-213` does claim `run_transcript` fails on "a gate the client never answers and
  never withdraws". It does not fail; it blocks. Worse than the finding says: the "withdraws"
  escape hatch is unreachable, because Fleet never emits `control_cancel_request` (it only decodes
  one — `claude/frames.rs:241`, `claude/map/gates.rs:221`; `grep -rn control_cancel_request crates/`
  finds no emit site), so `claude.rs:511`'s `Decision::Withdrawn` arm answers a frame that never
  arrives.

  Two parts of the argument do not survive.

  *"The player runs inside `spawn_blocking` so tokio cannot cancel it."* True and inert. `agent` is
  a whole subcommand of its own process (`main.rs:42-60`, `Commands::Agent`), and
  `run_transcript` is the last thing `main` does — nothing in that process ever attempts to cancel
  the blocking task, so the un-cancellability costs nothing. The real bound is external: the
  harness process is a child of `fleetd`, whose shutdown path writes the interrupt **and closes
  stdin** (`crates/fleet-daemon/src/agents/claude/mod.rs:456-465`), and closed stdin is exactly
  what `read_frame` returns `Ok(None)` for. So in a real run the block ends at teardown, and for
  Claude at the 10 s escalation (C11) — it is unbounded only in isolation.

  *The proposed fix needs the scoping the finding gives it.* A deadline on `read_frame` itself
  would be wrong: the between-turns dispatch loops (`claude.rs:43-58`, `codex.rs:47-71`) block
  legitimately and indefinitely waiting for the next prompt, and a scenario may sit idle for
  minutes between `type`/`key enter` lines. Only the gate wait can carry a budget.

- **Evidence**

  ```rust
  // crates/fleet-harness/src/agent/peer.rs:51
  pub(crate) fn read_frame(&mut self) -> anyhow::Result<Option<Value>> {
      loop {
          let mut line = String::new();
          let read = self.reader.read_line(&mut line)   // no deadline
  ```
  ```rust
  // crates/fleet-harness/src/agent.rs:210
  /// Fails when the transcript cannot be loaded, or when the client's half of the conversation
  /// breaks — a closed pipe mid-turn, or a gate the client never answers and never withdraws.
  ```
  `docs/TESTING-HARNESS.md` §11 lists eight known gaps; none of them is this one.

- **The reduced claim that holds**

  The doc comment at `agent.rs:213` is wrong: a gate the client never answers blocks rather than
  fails, and the "never withdraws" qualifier is doubly wrong because Fleet has no withdraw frame
  to send. Either give the gate wait (not `read_frame`) a bounded budget that settles the turn as
  `Interrupted`, or correct the comment. The `spawn_blocking`/cancellation argument should be
  dropped — nothing tries to cancel it, and the process is bounded by its parent's teardown.

---

### C13 (F1-15) — verdict: SURVIVES

- **Confidence**: medium-high

- **Reasoning**

  Both halves check out and I could not find a guard.

  `interrupted` is written in exactly three places per provider and cleared in exactly one:
  `claude.rs:544` (set, from `answer_control`), `claude.rs:213-214` (test-and-clear, inside the
  per-step loop); `codex.rs:153` (set, from `answer_simple`), `codex.rs:329-330` (test-and-clear,
  same place). Nothing consumes it at a turn boundary.

  *Between-turns set is reachable.* Both dispatch loops route an inbound interrupt into the same
  handler that sets the flag — `claude.rs:54` (`Some("control_request") => session.answer_control`)
  and `codex.rs:69` (`handle_request` → `answer_simple`, since `turn/interrupt` is not
  `turn/start`). And the daemon really can write one there: `ClaudeHarness::interrupt` only
  requires `session.active_turn() == Some(turn)` (`claude/mod.rs:311`), which is still true while
  the player's `result` is in flight and undrained; Codex's guard is the same
  (`codex/mod.rs:507-511`). The app-side `is_working()` latch lags the daemon in the *same*
  direction, so it does not close the window — it widens it.

  *Exhausted-turn retention is reachable.* `Playback::next_turn` returns
  `steps: &self.steps[len..]` — an empty slice — once the cursor is past the end
  (`transcript.rs:78-84`), so `for (…) in script.steps.iter().enumerate()` never executes and the
  `if self.interrupted` check at the bottom of the body never runs. `two-turns.json` has exactly
  two turns and `agents/*.scenario` sends prompts into it, so a third prompt hits this path;
  `tests.rs:802 a_prompt_past_the_end_of_the_transcript_settles_instead_of_hanging` exercises the
  exhausted turn but never sets the flag, so it does not cover the interaction.

  *Consequence.* The next turn plays one step and then settles `Interrupted` — Claude emits
  `result` with `terminal_reason: "aborted_streaming"` (`claude.rs:420-430`), Codex
  `turn/completed` with `status: "interrupted"` (`codex.rs:511`) — for a turn the user never
  stopped, and the daemon's `interrupted` set holds the *previous* turn id
  (`claude/session.rs:119,259-267`), so the two disagree about which turn was aborted.

  The only thing I would soften is the framing "non-deterministically, which is the worst possible
  failure mode for a harness": the race is the same one C11 depends on, and no scenario in
  `scenarios/agents/` can press Stop on a working thread today, so it is latent rather than live.
  The one-line fix the finding proposes (`std::mem::take` at the top of `run_turn`) is correct and
  cheap.

- **Evidence**

  ```rust
  // crates/fleet-harness/src/agent/transcript.rs:78
  if self.cursor >= self.steps.len() {
      return TurnScript { steps: &self.steps[self.steps.len()..],
                          settlement: Settlement::Completed, exhausted: true };
  }
  ```
  ```rust
  // crates/fleet-harness/src/agent/claude.rs:213 — the only consumer, inside the step loop
  if self.interrupted { self.interrupted = false; settlement = Settlement::Interrupted; break; }
  ```

---

### C14 (F1-17) — verdict: DROPS

- **Confidence**: high

- **Reasoning**

  The reviewer quoted the first sentence of the doc comment and dropped the second, which names
  the test that enforces the invariant. The full comment reads:

  > The file is compiled into the binary, so a malformed one is a build-time fact rather than a
  > runtime condition; `agent::tests::all_three_starter_transcripts_load_and_validate` is what
  > catches it.

  That test exists, is not `#[ignore]`d, and does the full job: it reads each of the three
  transcript files and runs them through `Transcript` deserialization **and**
  `transcript::validate` via the `starter` helper at `tests.rs:59-67`. It passes in this branch.
  Both files `plan.rs` actually embeds — `two-turns.json` (`plan.rs:453`) and `edit-approval.json`
  (`plan.rs:461`) — are in its list, and `include_str!("../../transcripts/two-turns.json")` and
  `transcripts().join("two-turns.json")` resolve to the same file, so the test and the preset
  cannot diverge.

  So "an unknown step type, an `approval` not adjacent to its `file_change`, or a step after
  `exit` ships into the run" is false: any of those fails `make test` before it can ship. The
  comment's only inaccuracy is calling a test-time check a "build-time fact", which is a wording
  quibble, not a missing guard.

  The residual worth keeping is much smaller than the finding: the test enumerates the three names
  by hand (`tests.rs:182-186`), so a *fourth* transcript added to `transcripts/` and embedded by a
  preset would be uncovered. That is a maintenance hazard, not a defect today, and it is not what
  F1-17 claims.

- **The specific thing the reviewer missed**

  `crates/fleet-harness/src/fixture/plan.rs:471` — the second half of the doc comment names
  `agent::tests::all_three_starter_transcripts_load_and_validate`, and
  `crates/fleet-harness/src/agent/tests.rs:180-191` is that test, which calls `validate` on every
  embedded starter:

  ```rust
  // crates/fleet-harness/src/agent/tests.rs:59
  let transcript: Transcript = serde_json::from_str(&raw)…;
  validate(&transcript).unwrap_or_else(|error| panic!("{} is invalid: {error}", path.display()));
  ```

---

### C15 (F1-18) — verdict: SURVIVES

- **Confidence**: high

- **Reasoning**

  I tried to refute the *impact* on the grounds that `tool_items` is a last-write-wins
  `HashMap<String, ItemId>` and the player emits `tool_use` immediately followed by `tool_result`
  (`claude.rs:143-146`), so a second call would simply overwrite the entry and each result would
  land on its own row. That refutation fails, because the daemon does not mint a second row at all:
  `assistant_tool` looks the provider id up **first** and, on a hit, patches the existing item
  instead of starting a new one (`claude/map/stream.rs:209-228`). So two `tool_call` steps sharing
  an id produce one row whose input, summary and output are clobbered by the second call, and both
  `tool_result` frames resolve to that same `ItemId` (`claude/map/stream.rs:248`), exactly as the
  finding says. The status is worse than described: `complete_item` early-returns once an item is
  already `Completed` (`claude/session.rs:385-395`), so the second call's `ItemUpdated{status:
  InProgress}` is emitted but its completion is swallowed.

  The validator gap is real and precisely located: `gate_id` (`transcript.rs:217-226`) matches only
  `Permission` and `Approval`, so `ToolCall { id, .. }` never enters the duplicate check, and
  `tests.rs:143 two_gates_may_not_share_an_id_and_a_step_may_not_follow_exit` tests only the two
  gate variants. `agent.rs:95` does call the field "the gate-free correlation key".

  Reachability is the one thing to keep honest about: no shipped transcript has two `tool_call`
  steps (`transcripts/*.json` — `two-turns.json` has one, the other two have none), and a scenario
  cannot author a transcript, because `fixture:` takes one of five frozen presets
  (`docs/TESTING-HARNESS.md` §4) and there is no scenario line that supplies a transcript path. So
  this bites the next person who writes a transcript, not the corpus today — which is what P3
  already says. Codex is unaffected: `codex.rs:222-227` destructures `ToolCall` with `..` and
  discards the transcript id entirely.

- **Evidence**

  ```rust
  // crates/fleet-daemon/src/agents/claude/map/stream.rs:209
  if let Some(item) = session.tool_items.get(&provider_id).copied() {
      …                                   // patch the FIRST row
  } else {
      let item = session.start_tool(turn, &provider_id, &name, input, parent, events);
      session.tool_items.insert(provider_id, item);
  }
  ```
  ```rust
  // crates/fleet-harness/src/agent/transcript.rs:217
  const fn gate_id(step: &TranscriptStep) -> Option<&str> {
      match step {
          TranscriptStep::Permission { id, .. } | TranscriptStep::Approval { id, .. } => Some(id.as_str()),
          _ => None,                       // ToolCall::id never checked
      }
  }
  ```

---

### C16 (F1-19) — verdict: SURVIVES NARROWED

- **Confidence**: high

- **Reasoning**

  The parse bug is real and I confirmed it out of band:

  ```text
  "ls"         -> program="ls"   argument=""
  "find"       -> program="find" argument=""
  "rg"         -> program="rg"   argument=""
  "ls -la src" -> program="ls"   argument="src"
  ```

  `words.next()` consumes the only element, so `next_back()` on the drained iterator yields
  `None`, and `{"name":"ls","arguments":null}` → `command_line` → `"ls"` →
  `{"type":"listFiles","command":"ls","path":""}`. The `read` arm already guards this exact case
  with `if !argument.is_empty()` (`codex.rs:766`), which shows the author knew about it and
  applied it to only one of the three arms — the `listFiles` and `search` arms are unguarded
  (`codex.rs:771-772`). No test covers `command_actions` at all (no such name in `tests.rs`).

  What does not hold is the impact as written. Nothing can reach it today:

  - The `agents` preset gives Codex `edit-approval.json` (`fixture/plan.rs:461`), which contains
    no `tool_call` step at all — the only `tool_call` in the whole transcript corpus is Claude's
    `Read`/`Cargo.toml` in `two-turns.json`, and even that renders as `"Read Cargo.toml"`, a
    two-word command that parses correctly.
  - A scenario cannot supply its own transcript: `fixture:` accepts one of five frozen presets
    (`docs/TESTING-HARNESS.md` §4) and the scenario grammar has no transcript line.

  So "a scenario asserting on the row sees a fabricated empty value" is not something any scenario
  can do; it is a latent defect waiting for the first single-word Codex `tool_call` anyone writes.
  The fix the finding proposes is right and is two lines.

- **The reduced claim that holds**

  `command_actions` mis-parses any single-word command into a `listFiles`/`search` action with an
  empty `path`/`query` instead of degrading to `unknown`, because `next()` and `next_back()` share
  one iterator and the emptiness guard was applied only to the `read` arm. Unreachable from the
  shipped corpus and from any scenario the frozen grammar allows; it is a correctness bug in a
  helper, not an observable harness failure today.
