# Agent-agnostic native subagents — Roadmap
> Phase plans:
>  - Phase 1: ./native-subagents-2026-09-17-phase-1-plan.md · ./native-subagents-2026-09-17-phase-1-tracker.md
>  - Phase 2: ./native-subagents-2026-09-17-phase-2-plan.md · ./native-subagents-2026-09-17-phase-2-tracker.md
>  - Phase 3: ./native-subagents-2026-09-17-phase-3-plan.md · ./native-subagents-2026-09-17-phase-3-tracker.md
>  - Phase 4: ./native-subagents-2026-09-17-phase-4-plan.md · ./native-subagents-2026-09-17-phase-4-tracker.md
>  - Phase 5: ./native-subagents-2026-09-17-phase-5-plan.md · ./native-subagents-2026-09-17-phase-5-tracker.md
>  - Phase 6: ./native-subagents-2026-09-17-phase-6-plan.md · ./native-subagents-2026-09-17-phase-6-tracker.md
>  - Phase 7: ./native-subagents-2026-09-17-phase-7-plan.md · ./native-subagents-2026-09-17-phase-7-tracker.md
>
> Source design: the Claude doc "Fleet: agent-agnostic native subagents — design"
> (https://claude.ai/code/artifact/aa6d9a65-a14e-416a-884f-022a9f579805). Every phase plan restates
> the parts of that design it needs, so a reader without access to the doc can still execute it.

## Summary

A subagent is a real native thread (Claude Code or Codex) that `fleetd` creates on behalf of another
native thread, links to it through a durable `Delegation` record, drives to a verified end, and whose
result `fleetd` feeds back into the caller as a new turn. The caller spawns it from its own shell tool
with `fleet subagent run`, ends its turn, and later receives one message carrying the report. No caller
polls; the daemon carries the wait. Any native thread can spawn a child of either provider, nesting is
bounded, the child is inspectable as an ordinary agent tab, and the link survives a daemon restart.

The design doc's implementation table names phases 0 to 6. This roadmap maps them to phases 1 to 7
below, in the same order, with three deliberate deviations recorded under "Why this is phased".

## Why this is phased

The work crosses every layer of the workspace: `fleet-core` types, `fleet-proto` wire variants and
goldens, a new SQLite migration and a new daemon service with its own worker, a new CLI noun, two new
transcript row kinds in `fleet-ui-kit`, tab-strip and palette changes in `fleet-app`, and the GUI
harness. Each layer has its own reviewer checklist (`.claude/skills/`) and its own verification
(`make test` versus `make harness`). The design doc asks for one pull request per phase and each phase
is shippable on its own: the wire is additive and gated on one capability string, so a daemon that has
finished phase 3 works with an app that has not started phase 4.

Three deviations from the design doc's table, each a judgement call about shippability:

1. **Outbox drain on daemon start moves from recovery (doc phase 5) into the service phase (phase 3
   here).** The design's exactly-once guarantee rests on "on restart, any terminal delegation still
   `Pending` is delivered again". A service phase that ships without that drain is not shippable by the
   design's own rules. The drain is the same code path as drain-on-wake, so the cost is one call at
   start.
2. **The same-worktree warning on `run` moves from polish (doc phase 6) into the service phase.** It is
   one condition inside the `run` validation that phase 3 writes anyway.
3. **Docs ride with their code, not in a docs phase.** `CLAUDE.md` requires a doc update in the same
   commit as the code it describes. Each phase therefore carries its own `docs/` edits. Phase 7 keeps
   only what has no earlier home: the ADR, the `fleet doctor` line, the `TODO.md` entries and a final
   cross-document audit.

One repo-specific constraint shapes phases 4 and 5 and is not in the design doc. The GUI harness
grammar (`docs/TESTING-HARNESS.md` §2) has no way to run a shell command, and the scripted provider
(§5) has no step that executes one. A subagent scenario needs the scripted caller to actually run
`fleet subagent run` and the scripted child to actually run `fleet subagent complete`. Phase 4 adds an
additive `shell` transcript step and role-based transcript selection to `fleet-harness` before any
scenario is written. §5 already permits additive optional transcript data, so the frozen grammar of §2
is untouched.

## Phase list

### Phase 1 — Prerequisites in the manager and reducer
- **Goal:** Split `Submitted.queued` into `JoinedActive` versus `QueuedNew`, stop the reducer and the
  store projector from closing live background items at turn settle, and record why a thread stopped.
- **Shippable state at end of phase:** No user-visible change. The manager can tell a steer from a
  queued turn, a Claude background task survives its turn's settle in the projection, and a thread
  record says whether the user or the provider stopped it.
- **Plan:** ./native-subagents-2026-09-17-phase-1-plan.md
- **Tracker:** ./native-subagents-2026-09-17-phase-1-tracker.md

### Phase 2 — Delegation model, wire variants and storage
- **Goal:** Add every type, wire variant, golden and table the later phases write to, with nothing
  yet served.
- **Shippable state at end of phase:** The daemon opens databases at migration 3 and serves nothing
  new. Every new wire shape has a byte-exact golden. The app compiles against `ItemKind::Delegation`
  and `MessageOrigin` and draws both as plain rows.
- **Plan:** ./native-subagents-2026-09-17-phase-2-plan.md
- **Tracker:** ./native-subagents-2026-09-17-phase-2-tracker.md

### Phase 3 — Delegation service and the `fleet subagent` CLI
- **Goal:** Spawn, complete, nudge, end, deliver and wait, end to end from a shell, with the
  `agent.delegation` capability advertised.
- **Shippable state at end of phase:** A Claude or Codex thread can run `fleet subagent run`, end its
  turn, and receive the child's report as a new turn. The report renders as a plain user message and the
  delegation as a plain row; the child is reachable only through `fleet agent list` and `fleet subagent
  status`.
- **Plan:** ./native-subagents-2026-09-17-phase-3-plan.md
- **Tracker:** ./native-subagents-2026-09-17-phase-3-tracker.md

### Phase 4 — App: delegation row, result card, child tabs and attach
- **Goal:** Give the caller's transcript a live delegation row and a result card, keep children out of
  the strip until attached, and bubble child attention up to the caller.
- **Shippable state at end of phase:** A human watching the caller sees the child's state, presses
  `⏎` to attach it, works in it as an ordinary tab, presses `^s u` to return, and `^s x` to detach.
  The harness corpus proves it.
- **Plan:** ./native-subagents-2026-09-17-phase-4-plan.md
- **Tracker:** ./native-subagents-2026-09-17-phase-4-tracker.md

### Phase 5 — App: the agents picker
- **Goal:** Reach any native thread the daemon knows, including closed callers and children in other
  worktrees, from `^s d` and an `AGENTS` palette section.
- **Shippable state at end of phase:** Every thread is reachable without a tab. A closed caller can be
  reopened. Attaching a child in another worktree switches session first.
- **Plan:** ./native-subagents-2026-09-17-phase-5-plan.md
- **Tracker:** ./native-subagents-2026-09-17-phase-5-tracker.md

### Phase 6 — Recovery after a daemon restart and cancel propagation
- **Goal:** Resume an orphaned child once with a nudge, mark undeliverable delegations, and cancel a
  tree from the top.
- **Shippable state at end of phase:** Killing the daemon mid-delegation costs one nudge turn, not the
  delegation. A caller deleted before delivery leaves a readable record. Cancelling a delegation with
  grandchildren stops the whole subtree.
- **Plan:** ./native-subagents-2026-09-17-phase-6-plan.md
- **Tracker:** ./native-subagents-2026-09-17-phase-6-tracker.md

### Phase 7 — ADR, doctor line and cross-document audit
- **Goal:** Record the decision, tell a user when the CLI is not on the harness child's `PATH`, and
  make every authoritative document agree with the shipped code.
- **Shippable state at end of phase:** `docs/decisions/0017-native-subagents.md` exists,
  `fleet doctor` reports the CLI's reachability, and `NATIVE-AGENTS.md` §13 and §15, `UX-SPEC.md`,
  `KEYMAP.md`, `agents-contracts.md` and `TODO.md` describe exactly what runs.
- **Plan:** ./native-subagents-2026-09-17-phase-7-plan.md
- **Tracker:** ./native-subagents-2026-09-17-phase-7-tracker.md

## Seams between phases

- **Phase 1 → 2.** `Submitted::{JoinedActive, QueuedNew}` and `StopCause` exist in memory after
  phase 1. Phase 2's migration 3 adds the `threads.stop_cause` column so a list read keeps its
  "never touches items, turns or gates" invariant. Until phase 2 lands, the stop cause is rebuilt from
  the log on hydrate only.
- **Phase 2 → 3.** Phase 2 adds the `agent.delegation` capability *string* but does not advertise it.
  Phase 3 adds it to `AGENT_CAPABILITIES` in the same commit that routes the six requests. A client
  that sees the capability can rely on all six verbs.
- **Phase 2 → 4.** Phase 2 makes the app compile against `ItemKind::Delegation` and
  `MessageOrigin::Delegation` by drawing them as a plain text row and an ordinary user bubble. Phase 4
  replaces those two fallbacks with `DelegationRow` and `DelegationResultCard`. An app older than phase 4
  talking to a phase-3 daemon still reads correctly.
- **Phase 3 → 4.** `Event::DelegationChanged` is published from phase 3 and carries the `headline`.
  Phase 4 subscribes to it. Phase 3's tokio tests are the contract; phase 4 adds no daemon behaviour.
- **Phase 3 → 6.** Phase 3 handles `TurnAborted { ProviderExited }` on a child by ending the
  delegation as `Failed` with no resume. Phase 6 inserts the one-resume rule before that ending and
  bumps `recoveries`. Phase 3 also drains the outbox on start; phase 6 adds the orphan pass and the
  undeliverable pass after it.
- **Phase 4 → 5.** Phase 4 adds `attached: HashSet<ThreadId>` and the `reopen` path on the app's
  agent state. Phase 5's picker calls those and adds nothing to the strip model.
- **Harness.** Phase 4's first task adds the scripted-provider `shell` step, role-based transcript
  selection, and the additive snapshot fields (`agents.threads[].parent`, `agents.delegations[]`).
  Phase 5 adds `lists.palette` rows for the `AGENTS` section. Both amend `docs/TESTING-HARNESS.md` §3
  and §5 under its own additive rule and touch nothing in §2.
- **Docs.** `docs/NATIVE-AGENTS.md` §13 gains one status row per phase as it lands, and §15
  "Delegations" is created in phase 3 and grown by phases 4 to 6. Phase 7 audits the result.

## Cross-phase risks

- **The transition half runs inside the store transaction.** Phase 3 places delegation transitions
  inside `project_event` (`crates/fleet-daemon/src/services/agents/store/project.rs`), which runs on
  the owned writer thread while the child's operation lock is held. Any manager call from there
  deadlocks. The rule "the transition half takes no lock and calls no manager verb" is enforced by
  giving it only a `&Transaction` and a wake channel, never a manager handle. Phase 6 must keep it.
- **The frozen harness contract.** `docs/TESTING-HARNESS.md` is frozen. Phases 4 and 5 add only what
  its §3 and §5 additive rules allow. Any edit to the §2 line grammar or an existing target name is a
  stop-and-ask, not a judgement call.
- **Clippy `-D warnings` across the workspace.** A new `ItemKind` or `TranscriptRowKind` variant breaks
  every exhaustive match at once. Phase 2 and phase 4 each budget for the sweep.
- **Goldens are byte-exact.** `#[serde(default, skip_serializing_if = ...)]` on every new field is what
  keeps existing goldens green. A field without `skip_serializing_if` changes every existing golden and
  is a bug, not a fixture refresh.
- **Real-harness drift.** The live tests in phase 3 run `claude -p` and `codex app-server` behind the
  `real-agents` feature. They are not in `make test`. Run them before the phase-3 PR is opened, and
  again in phase 7.

## Suggested order

Strictly 1 → 2 → 3, then 4 → 5, then 6, then 7. Phases 4 and 6 depend only on phase 3 and could run in
parallel by two people, since 4 touches `fleet-app` and `fleet-ui-kit` and 6 touches only
`fleet-daemon`. Phase 5 depends on phase 4's `attached` set. Phase 7 is last.
