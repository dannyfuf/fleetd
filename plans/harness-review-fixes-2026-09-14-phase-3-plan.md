# Test-harness review fixes — Phase 3: a scripted agent that settles — Plan
> Tracker: ./harness-review-fixes-2026-09-14-phase-3-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

The harness ships a scripted mock provider — a "player" that replays a recorded transcript over the
native-agent protocol so scenarios can exercise Fleet's agent surfaces without a real Claude or
Codex process. A two-reviewer adversarial review found six defects in it, and three of them are the
worst shape a test fixture can have: they hang or truncate non-deterministically, so they present as
flaky *application* scenarios rather than as harness bugs.

An `interrupt` arriving while an approval gate is open is dispatched into `answer_simple`, which
sets a flag nobody consults, and the gate loop goes straight back to awaiting an answer that will
never come — on Codex, forever. The read it blocks in has no deadline and runs inside
`spawn_blocking`, so tokio cannot cancel it either, which is what turns that bug and any future
stuck-gate bug into a wedge instead of a loud failure. And an interrupt read by the between-turns
dispatch loop leaves its flag set, silently truncating the *next* turn after its first step.

This phase makes the player honour an interrupt, bound its reads, and stop fabricating or colliding
wire data. It is self-contained: almost everything it touches lives under
`crates/fleet-harness/src/agent/`.

## Sizing call

**Phased**, and this is phase 3 of 4 — see
[the roadmap](./harness-review-fixes-2026-09-14-roadmap.md). Six issues: three P2 and three P3. It is
one focused stretch of a few days, and it is a separate phase rather than part of Phase 2 because
nothing outside `agent/` depends on it and nothing it depends on changes elsewhere — which makes it
the phase a second engineer can run in parallel from the moment `P1-T01` is committed. It is not
split further because `I6`, `I7` and `I8` are three views of one lifecycle (what happens to a turn
when an interrupt arrives, and what bounds the wait) and fixing one without the others leaves the
wedge reachable by a different route.

## Repository context

- **Project type:** Rust, one Cargo workspace, `resolver = "3"`, edition 2024, toolchain pinned by
  `rust-toolchain.toml`. Twelve crates under `crates/`.
- **Lint command:** `make lint` = `cargo fmt --all -- --check` plus
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **Test command:** `make test` = `cargo build -p fleet-daemon -p fleet-app -p fleet-harness`, then
  `cargo test --workspace` with `FLEET_DAEMON`, `FLEET_APP` and `FLEET_HARNESS_BIN` pointed at
  `target/debug/`.
- **The player:** `crates/fleet-harness/src/agent.rs` and `crates/fleet-harness/src/agent/` —
  `codex.rs`, `claude.rs`, `peer.rs`, `transcript.rs`, `launcher.rs`. It runs inside
  `spawn_blocking` (`agent.rs:217`).
- **The fixture side:** `crates/fleet-harness/src/fixture/plan.rs` installs the presets; `fn agents()`
  is at `:455`/`:463`. The five `fixture:` presets are frozen by §2's grammar
  (`empty|one-repo|busy|board|agents`), so no scenario can supply its own transcript.
- **The daemon side:** `crates/fleet-daemon/src/.../claude/map/stream.rs` maps provider frames onto
  thread rows; `manager/commands.rs:353-357` documents the Stop-versus-settle race that triggers
  `I8`.
- **The contract:** `docs/TESTING-HARNESS.md` §5 covers scripted-agent transcripts. §11 already
  records a *related but different* limit — "an answered Codex gate does not reliably settle its
  turn" — which is a native-agent adapter defect the harness found, not a harness defect, and is
  **not** in scope here.
- **Scenarios that exercise the player:** `scenarios/agents/` — `edit-approval-allow.scenario`,
  `edit-approval-deny.scenario`, `thread-streaming.scenario`, `popup-open.scenario`,
  `unread-mark.scenario` (recorded as intermittent in §11),
  `prefix-inside-a-thread.scenario`, plus `blocked/` and `expected-to-fail/`.
- **The `virtual` lane needs a live, unlocked Hyprland session and a hand-exported environment.**
  `export XDG_RUNTIME_DIR=/run/user/1000`, probe each directory under `/run/user/1000/hypr/` for the
  signature whose socket actually answers `hyprctl monitors -j`, export it as
  `HYPRLAND_INSTANCE_SIGNATURE`, and `export WAYLAND_DISPLAY=wayland-1`.
- **Skills to load before editing** (`CLAUDE.md`'s table): `rust-async-background-work` for the
  `spawn_blocking` and deadline work, `rust-gpui-testing` for every test added here,
  `rust-workspace-architecture` for the error-handling policy, `zed-quality-review` before calling
  the phase done.
- **Prerequisite:** `P1-T01` must be committed before this phase starts, or `make test` cannot pass.

## Assumptions

- The narrowings in `plans/issues.md` are authoritative. In particular `I6`'s trigger as the
  reviewer stated it is **wrong** — `Esc` on an approval card is `DenyAndStop`
  (`agent_thread/actions.rs:315-324`, `keymap.rs:512`), which answers the gate — and "forever" is
  false for Claude, whose 10 s `RESULT_DEADLINE` escalation closes stdin and bails. Only Codex
  wedges indefinitely.
- The five `fixture:` presets stay frozen. `P3-T06` may add a third scripted provider to the
  `agents` preset, but it does not add a sixth preset or a scenario-supplied transcript path.
- `I22` and `I23` are latent — no scenario can reach them — so each is closed with a unit test
  rather than a scenario. A latent fix with no test is indistinguishable from no fix.
- §11's "an answered Codex gate does not reliably settle its turn" entry describes a *native-agent
  adapter* defect that this phase does not fix. If the work here happens to resolve it, that is a
  bonus to be verified and recorded, not a goal.

## Out of scope

- The native-agent adapter defect recorded in §11 (Codex gates leaving
  `"dropped invalid native-agent provider event … event targets the wrong turn"` in `fleetd.log`),
  and the intermittency of `agents/unread-mark.scenario` that shares its cause.
- Adding a sixth `fixture:` preset, or a scenario directive that supplies a transcript.
- Recording screenshot baselines; adding CI; `catch_unwind`.
- Phase 1's idle and shutdown work, Phase 2's projection cluster, Phase 4's run-directory, report
  and baseline long tail.

## Affected areas

Single repository (`fleetd` workspace).

- `crates/fleet-harness/src/agent/codex.rs` — the gate loop (`:562`), the step loop's interrupt
  check (`:329`), the between-turns dispatch loop (`:69`), `command_actions` (`:760`).
- `crates/fleet-harness/src/agent/claude.rs` — the gate loop (`:498`), the between-turns dispatch
  loop (`:54`), the `tool_use` block id (`:144`).
- `crates/fleet-harness/src/agent/peer.rs` — `read_frame` (`:51`), a bare `read_line`.
- `crates/fleet-harness/src/agent.rs` — the doc comment at `:16-18` and `:210-214`, and the
  `spawn_blocking` site at `:217`.
- `crates/fleet-harness/src/agent/transcript.rs` — `validate` (`:219`).
- `crates/fleet-harness/src/fixture/plan.rs` — `fn agents()` (`:455`, `:463`).
- `docs/TESTING-HARNESS.md` — §5.
- `scenarios/agents/blocked/error-mid-stream.blocked`, `scenarios/agents/README.md`.

## Tasks

### P3-T01 — Notice an interrupt inside both gate loops (I6)

- **Intent:** make a `turn/interrupt` that arrives while an approval gate is open cancel the turn
  instead of leaving the player awaiting an answer that will never come.
- **Touches:** `crates/fleet-harness/src/agent/codex.rs`, `crates/fleet-harness/src/agent/claude.rs`.
- **Steps:**
  - Confirm the mechanism: `turn/interrupt` during an open gate is dispatched into `answer_simple`,
    which sets `self.interrupted = true` and returns; the gate loop (`codex.rs:562`, same shape at
    `claude.rs:498`) goes straight back to `read_frame`. The `if self.interrupted` check lives in
    `run_turn`'s step loop (`codex.rs:329`), reached only after `ask` returns. `agent.rs:16-18`
    promises "a mid-stream `interrupt` is noticed at the next gate" — the doc is right and the code
    is wrong.
  - Read the narrowing before writing a repro. The reachable triggers are
    `fleet agent interrupt <thread>` (CLI, deterministic) and an `Esc` landing in the millisecond
    window before the app's mirror learns of the gate. `Esc` on an approval card is `DenyAndStop`
    and answers the gate, so the reviewer's stated trigger will not reproduce.
  - In `codex.rs`, after `answer_simple` returns inside the gate loop:
    `if self.interrupted { return Ok(Decision::Cancel); }`.
  - In `claude.rs`, take the flag after `answer_control` and return a withdrawn decision, setting
    `Settlement::Interrupted` at the call site so the turn still emits its terminal frame.
  - Note what the two failed refutations established, so the fix is not re-litigated: neither
    adapter withdraws the gate (`claude/mod.rs:308`, `codex/mod.rs:505`), and the withdrawal frames
    travel the other way. The player is the only side that can break the wait.
  - Add a test per provider: open a gate, deliver `turn/interrupt`, assert a terminal frame
    (`result`/`turn/completed`) is emitted. The validator reproduced **zero** such frames, ever, on
    both providers — that is the assertion.
- **Verification:** `cargo test -p fleet-harness <test names>`; `make test`; then `make harness`
  over `scenarios/agents/`, with attention to `unread-mark.scenario`, which §11 records as
  intermittent. Paste the agents-directory verdicts into the tracker.
- **Done when:** an interrupt during an open gate settles the turn on both providers, pinned by a
  test per provider.

### P3-T02 — Give `Peer::read_frame` a deadline (I7)

- **Intent:** turn a stuck gate from an unbounded hang into a named failure.
- **Touches:** `crates/fleet-harness/src/agent/peer.rs`, `crates/fleet-harness/src/agent.rs`.
- **Steps:**
  - Confirm the mechanism: `agent/peer.rs:51` is a bare `read_line` with no timeout, and the player
    runs inside `spawn_blocking` (`agent.rs:217`), so tokio cannot cancel it either. The validator
    reproduced it directly — with the client half of a real socket pair still open, the player sits
    in `read_line` past a 3 s watchdog. Every existing test's `drive()` ends in EOF, which is why the
    suite cannot see it.
  - Note the doc drift this also fixes: `agent.rs:210-214` documents `run_transcript` as *failing*
    when a gate is never answered. It hangs instead.
  - Give the gate wait a bounded budget, sized like the scenario `await` default, and settle the turn
    as `Interrupted` when it expires. Record the value chosen and what it was sized against.
  - Correct the `agent.rs:210-214` doc comment if the budget lands somewhere other than where it
    implies.
  - Consider whether the deadline belongs on every `read_frame` or only on the gate wait; a deadline
    on the between-turns dispatch loop would make an idle player fail, which is wrong. Say which you
    chose and why.
  - Add a test: hold the client half of a socket pair open, never answer the gate, assert the player
    fails with a named error inside the budget rather than hanging. This is the mechanism that turns
    `I6` — and any future stuck-gate bug on the Fleet side — from a loud failure into a wedge, so the
    test matters more than the fix.
- **Verification:** `cargo test -p fleet-harness <test name>`, which must complete rather than hang
  (run it with a wall-clock check the first time); `make test`; then `make harness` over
  `scenarios/agents/`.
- **Done when:** an unanswered gate fails with a named error inside a bounded budget, pinned by a
  test that would hang before the fix.

### P3-T03 — Consume the `interrupted` flag at the top of `run_turn` (I8)

- **Intent:** stop an interrupt read between turns from silently truncating the next turn.
- **Touches:** `crates/fleet-harness/src/agent/codex.rs`, `crates/fleet-harness/src/agent/claude.rs`.
- **Steps:**
  - Confirm the mechanism: the flag is consumed only inside the per-step loop, so an interrupt read
    by the between-turns dispatch loop (`claude.rs:54`, `codex.rs:69`) stays set and aborts the
    *next* turn after its first step. The daemon documents the triggering race itself at
    `fleet-daemon/.../manager/commands.rs:353-357` (Stop vs settle).
  - Apply the fix `plans/issues.md` prescribes: consume the flag at the top of `run_turn` —
    `let mut settlement = if std::mem::take(&mut self.interrupted) { Settlement::Interrupted } else { script.settlement };`
    — keeping the per-step check for the in-turn case.
  - Do not chase the "an exhausted turn retains the flag indefinitely" clause; the narrowing records
    it as true but inconsequential, because the playback cursor never rewinds.
  - Add a test replaying `prompt₁, interrupt, prompt₂` and asserting turn two runs to completion.
    That sequence reproducibly truncates turn two to `aborted_streaming`/`interrupted` today.
  - This is the worst failure mode in the phase — non-deterministic truncation of a *later* turn,
    which presents as a flaky agent scenario rather than as a harness bug. Say in the commit message
    that it is what `agents/unread-mark.scenario`'s intermittency should be re-checked against.
- **Verification:** `cargo test -p fleet-harness <test name>`; `make test`; then run
  `make harness-one SCENARIO=scenarios/agents/unread-mark.scenario LANE=virtual` several times and
  record whether the intermittency §11 documents is still present. Do **not** edit §11's entry
  unless the reruns show it is gone.
- **Done when:** `prompt₁, interrupt, prompt₂` runs turn two to completion, pinned by a test.

### P3-T04 — Reject duplicate `tool_call` ids in `transcript::validate` (I22)

- **Intent:** stop a transcript with colliding tool-call ids from silently losing a row on the wire.
- **Touches:** `crates/fleet-harness/src/agent/transcript.rs`.
- **Steps:**
  - Confirm the gap: `transcript::validate` (`transcript.rs:219`) enforces id uniqueness for
    `permission`/`approval` gates only, yet `claude.rs:144` passes `ToolCall::id` through as the
    `tool_use` block id.
  - Note that the daemon consequence is **worse** than the finding reported:
    `claude/map/stream.rs:209-228` patches the existing row on an id hit rather than minting a
    second, so the second call gets *no row at all*, and `complete_item`'s early return swallows its
    completion. The duplicate is not merely confusing; it is data loss.
  - Include `TranscriptStep::ToolCall { id, .. }` in `validate`'s duplicate check, alongside the gate
    ids it already checks. Decide whether the two id spaces are shared or separate and say which in
    the code — a `tool_call` and a `permission` with the same id may or may not be legal, and the
    check should state the answer rather than imply it.
  - Add a test with two `ToolCall` steps sharing an id, asserting `validate` rejects it with a
    message naming the id.
  - This is latent: the five `fixture:` presets are frozen and no scenario can supply a transcript,
    so the test is the only thing that exercises the fix.
- **Verification:** `cargo test -p fleet-harness <test name>`; `make test`; and confirm the two
  embedded transcripts in `fn agents()` still validate, since tightening a validator is exactly the
  change that rejects existing fixtures.
- **Done when:** a transcript with duplicate `tool_call` ids fails validation with a named error, and
  the shipped transcripts still pass.

### P3-T05 — Fix `command_actions`'s single-word parse (I23)

- **Intent:** stop the player fabricating an empty path for a command with no argument.
- **Touches:** `crates/fleet-harness/src/agent/codex.rs`.
- **Steps:**
  - Confirm the defect: at `codex.rs:760`, `words.next()` consumes the only word, so
    `words.next_back()` yields `""`. Reproduced end to end as
    `{"type":"listFiles","command":"ls","path":""}` — a fabricated empty value rather than a
    degradation to `unknown`.
  - Note the blast radius is wider than the finding reported: `rg` and `grep` are affected too, and
    `find`/`ls` take the *last* word rather than the first argument, which is a second bug in the
    same function worth fixing while you are here.
  - Collect once and take `words.first()` / `words.get(1..)`, treating a program with no argument as
    `unknown`.
  - Add a test per affected program covering the no-argument case and the multi-argument case, so
    the last-word-versus-first-argument behaviour is pinned rather than inferred.
  - Unreachable from today's corpus, so the tests are the verification.
- **Verification:** `cargo test -p fleet-harness <test names>`; `make test`.
- **Done when:** a single-word command parses as `unknown`, multi-argument commands take the right
  word, and both are pinned by tests.

### P3-T06 — Reconcile §5's transcript count with the `agents` preset (I18)

- **Intent:** make the doc and the preset agree about how many starter transcripts exist, so a
  scenario can reach a provider error.
- **Touches:** `crates/fleet-harness/src/fixture/plan.rs`, `docs/TESTING-HARNESS.md` (§5),
  `scenarios/agents/blocked/error-mid-stream.blocked`.
- **Steps:**
  - Confirm the drift: §5 says the `agents` fixture embeds three starter transcripts; `fn agents()`
    has only two `include_str!` sites (`fixture/plan.rs:455`, `:463`) — `two-turns.json` and
    `edit-approval.json`. `error-mid-stream.json` is used by a unit test and no preset, so **no
    scenario can reach a provider error or the `failed` thread state**.
  - `scenarios/agents/blocked/error-mid-stream.blocked` already diagnoses this; read it before
    deciding.
  - Choose and record: give the preset a third scripted provider carrying `error-mid-stream.json`,
    or amend §5 to say two. The repo's docs-are-authoritative rule makes this non-optional either
    way — P3 by impact, but it cannot be left as drift.
  - Prefer wiring the third provider: it is what unblocks
    `scenarios/agents/blocked/error-mid-stream.blocked`, and a preset that cannot produce a provider
    error leaves the `failed` thread state untested by anything.
  - If the provider is wired, promote the blocked scenario into `scenarios/agents/` and confirm it
    passes; if §5 is amended instead, update the `.blocked` file's diagnosis to say the limitation is
    now documented rather than accidental.
- **Verification:** `cargo test -p fleet-harness` (the unit test that loads all three transcripts
  must still pass); `make test`; then `make harness` over `scenarios/agents/`, including the
  promoted scenario if you wired the provider.
- **Done when:** §5's count matches `fn agents()`, and either a scenario can reach the `failed`
  thread state or the `.blocked` file records the documented reason it cannot.

## Verification

Run from the workspace root, in this order:

```sh
make lint     # cargo fmt --all -- --check + clippy --workspace --all-targets --all-features -D warnings
make test     # builds fleet-daemon, fleet-app, fleet-harness, then cargo test --workspace
make restart  # if daemon code changed; this phase should not touch it, but check the diff
make harness  # the whole scenarios/ corpus in the virtual lane
```

Plus, specific to this phase:

```sh
make harness-one SCENARIO=scenarios/agents/unread-mark.scenario LANE=virtual   # run several times
```

`unread-mark.scenario` is recorded in §11 as intermittent, and `P3-T03` is the fix most likely to
change that. Several runs, with the count and outcomes recorded, is the evidence — a single green
run of an intermittent scenario proves nothing.

`make harness` needs a live, **unlocked** Hyprland session with `WAYLAND_DISPLAY`,
`XDG_RUNTIME_DIR` and a *probed* `HYPRLAND_INSTANCE_SIGNATURE` exported by hand — see Repository
context.

## Definition of done

- [ ] Every task `P3-T01`…`P3-T06` is checked off in the tracker, each with pasted verification
      output or a one-line "verified: <how>".
- [ ] `make lint` is clean.
- [ ] `make test` passes.
- [ ] `make harness` runs the full corpus green, or the tracker records exactly why it could not run
      and what was verified instead.
- [ ] Every test added here completes rather than hangs — `P3-T02`'s test in particular would hang
      before its fix, so its wall-clock time is part of the evidence.
- [ ] `docs/TESTING-HARNESS.md` §5 matches `fn agents()`, and `agent.rs`'s doc comments at `:16-18`
      and `:210-214` match the code.
- [ ] `unread-mark.scenario` has been run several times and the result recorded, with §11's
      intermittency entry left alone unless the reruns show it is gone.
- [ ] Every latent fix (`I22`, `I23`) carries a test written with it.
- [ ] `zed-quality-review` has been run over the phase's diff.
- [ ] The tracker reflects reality, including the `P3-T02` budget value and the `P3-T06`
      wire-vs-amend decision.
- [ ] Follow-ups discovered mid-flight are captured in the tracker's Follow-ups section.

## Risks and rollback

- **A deadline in the wrong place makes an idle player fail.** `P3-T02` must bound the *gate wait*,
  not every `read_frame`; a budget on the between-turns dispatch loop would turn a player waiting
  legitimately for the next prompt into a failure. Decide deliberately and write the reason in the
  code.
- **Tightening `validate` can reject the shipped fixtures.** `P3-T04` adds a duplicate check over a
  field nothing checked before. Run the transcript-loading unit test explicitly, not just
  `make test`, and confirm `two-turns.json` and `edit-approval.json` still validate.
- **`P3-T06` can make an intermittent scenario worse.** Wiring a third provider that produces a
  provider error adds a path through the native-agent adapter that §11 already records as
  unreliable. If the promoted scenario is flaky, leave it `.blocked` with an updated diagnosis
  rather than committing a flaky scenario into the corpus — a flaky green is what this whole
  initiative exists to remove.
- **These fixes are mostly invisible to the corpus.** Four of the six are latent or need a
  deliberately constructed transcript, so a green `make harness` after this phase says almost
  nothing about whether the work is correct. The unit tests are the verification; treat a task
  ticked without one as not done.
- **No CI catches any of this.** There is no `.github/workflows` in the repo, so the Verification
  block above is the only gate.
