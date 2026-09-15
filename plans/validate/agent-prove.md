# Adversarial validation — batch `agent` (scripted providers, `crates/fleet-harness/src/agent/`)

Role: **PROVE**. Base `881162c5c283cefb4319bbfdb79629ef31960745`; every `agent/*.rs` file below is
new on `test-harness`.

All six candidates were driven through the in-memory peer with **temporary** tests appended to
`crates/fleet-harness/src/agent/tests.rs`, run with `cargo test -p fleet-harness agent::tests::prove
-- --nocapture`, then reverted. The working tree is back to its pre-validation state (the one
remaining `M crates/fleet-harness/src/baseline.rs` is another agent's, not mine — I never opened
that file). The proof source is kept at
`/tmp/claude-1000/-home-df--fleet-worktrees-dannyfuf-fleetd-test-harness/9854f307-554c-41b4-a3bb-8b7c90e6437d/scratchpad/tests.rs.proof-final`.

Full run output (7/7 passed):

```
C15 tool_use ids on the wire: ["toolu_same", "toolu_same"]
C11/codex error at EOF: the client closed the connection with gate "gate-edit-1" open
C11/codex turn/interrupt receipted: true
C11/claude error at EOF: the client closed the connection with gate "gate-edit-1" open
C11/codex turn/completed frames: 0
C11/claude result frames: 0
C11/claude interrupt receipted: true
C16 commandActions[0] = {"type":"listFiles","command":"ls","path":""}
C13/claude result[0] subtype=Some("success") terminal_reason=Some("completed")
C13/claude result[1] subtype=Some("success") terminal_reason=Some("aborted_streaming")
C13/codex turn/completed[0] status=Some("completed")
C13/codex turn/completed[1] status=Some("interrupted")
C12 outcome after 3 s with the client still connected: Err(Timeout)
test result: ok. 7 passed; 0 failed
```

---

### C11 (F1-3) — verdict: SURVIVES NARROWED

- **Confidence**: high (on the defect and on the wedge); high (on the narrowing)

- **Reasoning** — the defect is real and reproduced end to end; the *claimed* trigger is not.

  *The defect.* Both gate loops are the only stdin read while a gate is open, and both dispatch a
  client request straight back into the generic answerer without ever re-reading the flag it sets:

  - Codex: `ask` loops on `read_frame` (`codex.rs:563`); a request that is not `turn/start` goes to
    `answer_simple` (`codex.rs:572`), whose `"turn/interrupt"` arm sets `self.interrupted = true`
    and returns a receipt (`codex.rs:152-155`). The loop then re-enters `read_frame`. The only
    consumer of the flag is `codex.rs:329`, inside `run_turn`'s per-step loop — reached only once
    `ask` has returned, which it never does.
  - Claude: identical shape at `claude.rs:499` → `answer_control` → `"interrupt"` arm at
    `claude.rs:543-546`; consumer at `claude.rs:213`.

  Reproduced twice. With an EOF-terminated cursor the player errors instead of settling
  (`the client closed the connection with gate "gate-edit-1" open`) having emitted **zero** `result`
  / `turn/completed` frames — i.e. it was still in the gate loop when input ran out. With a *live*
  `UnixStream` pair, which is what the daemon actually gives it, the player is **still blocked after
  3 s** with the interrupt receipted and no settlement (that is the C12 proof below). So: interrupt
  with a gate open ⇒ no terminal frame, ever. The finding's mechanism is exactly right.

  *The narrowing — the GUI never sends that frame from an open approval card.* The claimed
  one-line scenario change ("substitute the Stop affordance for `n`" at
  `scenarios/agents/edit-approval-deny.scenario:23`) does **not** produce it:

  1. The only client that can emit a bare interrupt is `RequestBody::AgentInterrupt`
     (`fleet-proto/src/request.rs:220`), and the only construction of `InterruptReason` anywhere in
     the tree is `InterruptReason::User` at
     `fleet-daemon/src/services/agents/providers/mod.rs:303` — `ThreadClosing`, `DaemonShutdown`
     and `GateCancelled` (`agents/harness/mod.rs:195-200`) are declared and **never constructed**
     (`grep -rn GateCancelled crates/ --include=*.rs` returns the declaration only). So no
     daemon-internal path, including thread close and shutdown, emits one.
  2. The app's only dispatch site is `AgentThreadView::stop`
     (`fleet-app/src/screens/agent_thread/actions.rs:302-341`), and it refuses exactly this case:
     ```rust
     // An approval's `esc` is `deny and stop`, which is a different thing from an interrupt
     if matches!(self.composer_mode(), ComposerMode::Approval(_)) {
         self.act_on_decision(&DecisionAction::DenyAndStop, cx);
         return;
     }
     ```
     `composer_mode()` derives `Approval` straight from `projection.gates.last()`
     (`agent_thread/mod.rs:338-346`), so while a permission/approval gate is in the mirror, `esc`
     **answers the gate**.
  3. The keymap agrees from the other side: `"escape", "Agent > AgentDecision > AgentPermission"
     => native_agent::DenyAndStop` (`fleet-app/src/keymap.rs:512`); `native_agent::Stop` is bound
     only under `AgentWorking` (`keymap.rs:433`) and `AgentNativeScroll` (`keymap.rs:462`), and
     `agent_context_chain` puts an open gate above both (`fleet-app/src/state/agents.rs:571-575`).
     In scroll mode the first `esc` exits scroll mode (`actions.rs:315-318`) and hands the keyboard
     back to `AgentPermission`.
  4. There is no clickable bare Stop. The agent thread exposes `agents.transcript`,
     `agents.composer` and `agents.decision`
     (`fleet-app/src/screens/agent_thread/view.rs:93,97,121`), and the decision drawer exposes the
     five `agents.approval.*` targets (`fleet-ui-kit/src/components/agent/decision.rs:66-75`). The
     Stop-looking one, `agents.approval.deny_and_stop`, routes to `DecisionAction::DenyAndStop` —
     a gate answer, not an interrupt.
  5. `act_on_decision` has no fall-back to an interrupt when a decision refuses a key
     (`actions.rs:562-583`).

  *Where it IS reachable.* Two paths, both real:

  - **`fleet agent interrupt <thread>`** — a first-class CLI subcommand
    (`fleet-cli/src/commands.rs:183`, `fleet-cli/src/commands/agents.rs:100-106`,
    `fleet-cli/src/args.rs:571`) that calls `client.agent_interrupt` directly and so bypasses the
    app's `DenyAndStop` routing entirely. Run against a harness run's private `fleetd` while an
    approval card is up, it wedges the scripted provider deterministically. Not reachable from
    `scenarios/*.scenario`: the grammar has no CLI verb (`scenario.rs:886-935`).
  - **A race in the GUI.** `stop()`'s guard reads the *app's mirror*. The player emits the approval
    request and immediately blocks in `ask`; the gate only reaches `composer_mode()` after the
    daemon maps it, emits `AgentEvent`, the IPC hop lands and GPUI renders a frame. An `esc`
    inside that window sees `is_working()` and dispatches `AgentInterrupt` — the player is already
    in `ask`, and the run wedges. Narrow (single-digit ms), non-deterministic, and precisely the
    moment a user reaches for Stop (the agent has gone quiet).

- **Evidence**: the two `prove_c11_*` tests above; `codex.rs:152,329,563,572`;
  `claude.rs:213,499,543`; `actions.rs:302-341`; `keymap.rs:433,462,512`; `agents.rs:571-575`;
  `providers/mod.rs:303`; `fleet-cli/src/commands/agents.rs:100`.

- **The reduced claim that holds**: *A bare `turn/interrupt` (Codex) or `interrupt` control request
  (Claude) delivered while a gate is open leaves the scripted player blocked in its gate loop for
  ever — no `turn/completed`, no `result`, thread stuck `working`. It is not reachable by pressing
  `esc` on an approval card (that is `DenyAndStop`, which answers the gate); it is reachable
  deterministically via `fleet agent interrupt`, and non-deterministically via an `esc` that lands
  in the window between the player emitting the approval request and the app's mirror learning
  about the gate.* The fix the finding proposes is still exactly right, and `agent.rs:16-18`'s
  promise ("a mid-stream `interrupt` is noticed at the next gate") remains false as written.

- **Why `tests.rs:427` does not catch it** — confirmed verbatim.
  `a_claude_interrupt_is_receipted_and_aborts_the_turn_it_arrives_in` builds `interrupt` *and*
  `allow` (`tests.rs:433-437`) and feeds both (`tests.rs:441`). The `allow` is a
  `control_response` for `gate-edit-1` — an answer a real Fleet, having decided to stop, never
  sends. It is the `allow` that releases `ask`; the turn then aborts at the *next step boundary*
  (`claude.rs:213`), and the test reads `terminal_reason == "aborted_streaming"` as proof the
  interrupt worked. Remove the `allow` — which is what my `prove_c11_claude_*` test does — and
  there is no `result` frame at all. The test asserts the flag's *post-gate* behaviour and mistakes
  it for the gate's.

---

### C12 (F1-4) — verdict: SURVIVES

- **Confidence**: high
- **Reasoning**: `Peer::read_frame` (`peer.rs:51-69`) is `self.reader.read_line(&mut line)` with no
  `set_read_timeout`, no `select`, no budget; the whole player is fenced behind
  `tokio::task::spawn_blocking` (`agent.rs:216-224`), which tokio cannot cancel. Meanwhile the
  rustdoc immediately above it (`agent.rs:210-214`) says:

  > Fails when the transcript cannot be loaded, or when the client's half of the conversation
  > breaks — a closed pipe mid-turn, **or a gate the client never answers and never withdraws**.

  It does not fail on that second case. It blocks.

  Proved directly rather than by inspection. `prove_c12_…` replaces the test suite's
  EOF-terminated `Cursor` with a real `UnixStream::pair()` — the client half stays open, exactly
  as the daemon's pipe does — writes the prompt and then an `interrupt` with the edit gate open,
  and waits on a channel:

  ```
  C12 outcome after 3 s with the client still connected: Err(Timeout)
  ```

  The player is still inside `read_line` three seconds later. Note what this shows about the whole
  existing agent test suite: every test's `drive()` ends in EOF, so the only reason a stuck gate
  ever *errors* in tests is that the cursor ran out. Nothing in the suite can observe the hang,
  which is why C11 shipped.
- **Evidence**: `peer.rs:51-69`; `agent.rs:210-224`; callers `claude.rs:499`, `codex.rs:563`, plus
  the between-turns loops `claude.rs:44`/`codex.rs:48`; the `prove_c12_…` run output above.

---

### C13 (F1-15) — verdict: SURVIVES

- **Confidence**: high on the mechanism and its consequence; medium on how often the trigger fires.
- **Reasoning**: `self.interrupted` is consumed only *inside* the per-step loop (`claude.rs:213`,
  `codex.rs:329`). The between-turns dispatch loops (`claude.rs:44-58`, `codex.rs:47-71`) also route
  requests into `answer_control`/`answer_simple`, so an interrupt read there sets the flag with no
  turn to spend it on, and it survives into the next `run_turn` — which then plays exactly **one**
  step and breaks with `Settlement::Interrupted`.

  Reproduced on both providers with a two-turn transcript and the frame order
  `prompt₁, interrupt, prompt₂`:

  ```
  C13/claude result[0] subtype=success terminal_reason=completed
  C13/claude result[1] subtype=success terminal_reason=aborted_streaming
  C13/codex  turn/completed[0] status=completed
  C13/codex  turn/completed[1] status=interrupted
  ```

  Turn two — which the transcript scripts as a clean single-text turn — comes back truncated and
  aborted because of an interrupt that belonged to turn one.

  *Trigger.* `ClaudeHarness::interrupt` writes the frame whenever `session.active_turn() ==
  Some(turn)` (`fleet-daemon/src/agents/claude/mod.rs:308-317`); `CodexHarness::interrupt` the same
  against `session.active_turn` (`codex/mod.rs:505-523`). The daemon's own manager comment states
  the race as a routine event:

  > `esc` and the `result` frame that ends the turn race by milliseconds, and the client cannot see
  > the settle coming. (`services/agents/manager/commands.rs:353-357`)

  That comment covers the case where the daemon has *already* settled (a harmless no-op). The
  damaging half is the other side of the same millisecond: the player has emitted `result` /
  `turn/completed` and returned to its dispatch loop, but the daemon has not yet applied the frame,
  so `runtime_inflight` still yields a turn and the interrupt goes out. The flag lands between
  turns and truncates whatever the scenario prompts next. Non-deterministic truncation of a later
  turn is precisely the failure a determinism harness must not have.

  The second half of the finding — "on an exhausted turn (`transcript.rs:79-86` returns an empty
  step slice) the loop body never runs, so the flag is retained indefinitely" — is **true but
  inconsequential**: `Playback`'s cursor never rewinds (`transcript.rs:97`), so every turn after
  exhaustion is empty regardless of the flag, and the retained flag can change nothing. That clause
  should be dropped from the finding; the fix (`std::mem::take` at the top of `run_turn`) is
  unaffected.
- **Evidence**: `claude.rs:44-58,213`; `codex.rs:47-71,329`; `transcript.rs:79-103`;
  `fleet-daemon/src/agents/claude/mod.rs:308-317`; `codex/mod.rs:505-523`;
  `services/agents/manager/commands.rs:352-370`; the two `prove_c13_*` runs above.

---

### C14 (F1-17) — verdict: SURVIVES NARROWED

- **Confidence**: high
- **Reasoning**: The literal claim about the function is true. `fixture::plan::starter`
  (`fixture/plan.rs:468-475`) does `serde_json::from_str::<serde_json::Value>` — syntax only, never
  into `Transcript`, never through `transcript::validate` — and `tools::install_agents`
  (`fixture/tools.rs:104-117`) writes `agent.transcript` verbatim onto the run's PATH
  (`write_json(&transcript, &agent.transcript)`).

  But the impact does not follow, for two independent reasons:

  1. **The comment names its own coverage.** The finding quotes the first half and stops. The full
     text is: *"a malformed one is a build-time fact rather than a runtime condition;
     `agent::tests::all_three_starter_transcripts_load_and_validate` is what catches it."* That test
     exists (`agent/tests.rs:180-191`) and its `starter()` helper (`agent/tests.rs:59-66`) does
     deserialize into `Transcript` **and** call `validate`, over all three shipped documents. So the
     stated failure — a structurally invalid preset shipping into a run — is caught by `make test`,
     not by nothing.
  2. **No untrusted document can reach `install_agents`.** Its input is `fixture.agents`, and the
     only producer is `plan::agents()` (`plan.rs:428-444`) wiring the two `include_str!`ed starters
     (`plan.rs:453-466`). The `fixture:` vocabulary is frozen (`docs/TESTING-HARNESS.md` §5) and has
     no way to name a transcript. The one path that *does* take an arbitrary file — the
     `fleet-harness agent --transcript <file>` subcommand (`docs/TESTING-HARNESS.md:324`) — goes
     through `agent::load`, which deserializes **and** runs `transcript::validate`
     (`agent.rs:198-207`).

  So the trigger requires someone to hand-edit an embedded starter into something syntactically
  valid but structurally invalid *and* to ignore a failing `make test`. At that point the player's
  own `load` rejects it at session start with a named error, which is a worse message than a
  build-time one but not a silent one.
- **If NARROWED**: *`fixture::plan::starter`'s name and doc overstate what it does — it is a JSON
  syntax check, not validation — and the same document is validated only by a test in another
  module. Deserializing into `Transcript` and running `transcript::validate` inside
  `install_agents`, naming the preset, is worth doing as defence in depth and to make the comment
  true. There is no reachable failure today: both embedded starters are covered by
  `all_three_starter_transcripts_load_and_validate`, no fixture preset can name a third document,
  and the `--transcript` CLI path validates.* Severity below P3.
- **Evidence**: `fixture/plan.rs:428-475`; `fixture/tools.rs:104-117`; `agent/tests.rs:59-66,180-191`;
  `agent.rs:198-207`; `docs/TESTING-HARNESS.md:322-362`.

---

### C15 (F1-18) — verdict: SURVIVES NARROWED

- **Confidence**: high on the validation gap and the collision; high that it is latent today.
- **Reasoning**: `gate_id` (`transcript.rs:219-226`) matches only `Permission` and `Approval`, so
  `validate`'s duplicate check (`transcript.rs:202-207`) never sees a `ToolCall::id` — yet
  `claude.rs:144` passes it through verbatim as the `tool_use` block id:
  `self.tool_use(name, arguments.clone(), Some(id.clone()))`. `agent.rs:95` calls that field "the
  gate-free correlation key".

  Proved: a transcript with two `ToolCall` steps sharing `"toolu_same"` passes `validate` without
  complaint, and the player puts the same id on two `tool_use` blocks —

  ```
  C15 tool_use ids on the wire: ["toolu_same", "toolu_same"]
  ```

  The daemon-side consequence is **worse than the finding states**. `ClaudeSession::tool_items` is a
  `HashMap<provider_id, ItemId>` (`fleet-daemon/src/agents/claude/session.rs:125`), and
  `map::stream`'s `tool_use` handler takes the already-present branch on a repeat id
  (`map/stream.rs:209-228`): the second call produces **no row at all** — it silently patches the
  first row's input and summary — and both tool results, keyed by `tool_use_id`
  (`map/mod.rs:102,134`), land on that one item. So a transcript author gets one row where they
  wrote two, with the second call's input and the second result on it.

  Latent, though: neither shipped starter has duplicate ids (`two-turns.json` has a single
  `tool_call`, `edit-approval.json` none), the fixture presets embed only those two, and no
  scenario can supply a transcript. The exposure is a hand-written transcript via
  `fleet-harness agent --transcript`, or a future starter.
- **If NARROWED**: *`transcript::validate` does not enforce uniqueness of `ToolCall::id` even
  though `claude.rs:144` uses it as the provider item id and the daemon keys `tool_items` by it;
  duplicates silently collapse two transcript steps into one transcript row. No shipped transcript
  or reachable fixture triggers it today — it is a load-time guard that is missing, not a live
  failure.* The one-line fix (add `ToolCall { id, .. }` to `gate_id`, or a second uniqueness pass)
  is worth taking.
- **Evidence**: `transcript.rs:202-226`; `claude.rs:138-146`; `agent.rs:93-96`;
  `fleet-daemon/src/agents/claude/session.rs:125,375`;
  `fleet-daemon/src/agents/claude/map/stream.rs:209-228`;
  `fleet-daemon/src/agents/claude/map/mod.rs:102,134`; the `prove_c15_…` run above.

---

### C16 (F1-19) — verdict: SURVIVES NARROWED

- **Confidence**: high on the parse bug; high that no shipped transcript reaches it.
- **Reasoning**: `command_actions` (`codex.rs:760-763`):
  ```rust
  let mut words = command.split_whitespace();
  let program = words.next().unwrap_or_default();
  let argument = words.next_back().unwrap_or_default();
  ```
  `next()` consumes the only element of a one-word command, so `next_back()` on the now-exhausted
  `SplitWhitespace` yields `None` → `""`. The `"ls" | "find"` arm has no `!argument.is_empty()`
  guard (unlike the `read` arm at `codex.rs:764`), so it fabricates an empty path instead of
  degrading to `unknown` — which the doc comment right above it says is "exactly the case Fleet
  must degrade to the raw command line for".

  Proved end to end through the Codex player, from a `tool_call` with `arguments: null` (which
  `command_line` renders as the bare name, `codex.rs:735-751`):

  ```
  C16 commandActions[0] = {"type":"listFiles","command":"ls","path":""}
  ```

  Note the same hole is one word wider than the finding says: `"rg"`/`"grep"` with a single word
  produce `{"type":"search","query":""}` by the identical route, and any two-word command has
  `program == argument` for a one-argument call only by luck — `find . -name x` yields
  `path: "x"`, not `"."`.

  Not reachable from the corpus: neither shipped starter contains a `tool_call` whose rendered
  command is a single word (`two-turns.json`'s is `Read` with a `file_path`), the frozen `fixture:`
  vocabulary cannot name another transcript, and the only route to an arbitrary transcript is
  `fleet-harness agent --transcript`.
- **If NARROWED**: *Real parse bug, correctly diagnosed, with a slightly wider blast radius than
  reported (`rg`/`grep` too, and the `find`/`ls` arms take the LAST word rather than the first
  argument). Unreachable from today's scenario corpus because no shipped transcript renders a
  single-word command; it is a trap for the next transcript author, not a live defect.* The
  proposed fix (collect once, `first()` / `get(1..)`, `unknown` when there is no argument) is right.
- **Evidence**: `codex.rs:735-751` (`command_line`), `codex.rs:755-776` (`command_actions`);
  `crates/fleet-harness/transcripts/two-turns.json`; the `prove_c16_…` run above.

---

## Incidental

While chasing C11's reachability I checked `docs/TESTING-HARNESS.md:246`'s five
`agents.approval.*` click targets: they are registered, in `fleet-ui-kit`'s decision drawer
(`crates/fleet-ui-kit/src/components/agent/decision.rs:66-75`), not in `fleet-app`. Worth noting
because `agents.approval.deny_and_stop` is the one clickable "Stop" a scenario can reach from an
open approval card — and it is a **gate answer**, which corroborates C11's narrowing rather than
contradicting it.
