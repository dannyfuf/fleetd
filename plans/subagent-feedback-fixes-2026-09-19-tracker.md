# Native subagent feedback fixes (batch 1) — Tracker

> Plan: ./subagent-feedback-fixes-2026-09-19-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement

- Check the kickoff box below before starting.
- Move tasks through: `[ ]` todo → `[~]` in progress → `[x]` done. One task in progress at a time
  **per agent**; this batch runs several agents concurrently, so put your agent's name beside the
  task you take.
- After each task: tick its box, paste the verification command output (or a one-line
  "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an
  existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

### Extra agreement for this batch — shared worktree

- **These tasks run concurrently in ONE worktree.** Edit only the files listed under your task's
  "Owns" in the plan. If you need a file you do not own, stop and write it under *Notes* instead
  of editing it.
- **T01 must commit before T02, T03 and T04 start.** T02/T03/T04 then run in parallel. T05 runs
  last, after all three.
- **T03 inherits `crates/fleet-cli/src/commands/subagents.rs` and
  `crates/fleet-cli/src/commands/tests.rs` from T01** once T01 has committed. Do not open them
  before that.
- **T02 owns `docs/NATIVE-AGENTS.md` for the whole batch**, including the §15 paragraphs for
  items 2, 3 and 5 whose code lands in T03. T03 re-reads §15 as its last step, after T02 has
  committed, and fixes any divergence.
- Before ticking your box, grep for your own change. In a shared worktree a lost edit looks like
  nothing at all, not like a conflict.
- Commit format: `<area>: <imperative lowercase summary>`, area one of `proto`, `client`, `cli`,
  `core`, `daemon`, `docs`.

## Kickoff

- [ ] I have read the plan end to end.
- [ ] I have read `CLAUDE.md` at the repo root, and the skill named by my task in
      `.claude/skills/`.
- [ ] I have run `make lint` and `make test` once on a clean tree to confirm a green baseline.
- [ ] I know which files my task owns and which it does not.
- [ ] I am ready to start.

## Tasks

- [ ] T01 — Send the caller's own `fleet` executable path with `subagent run` *(proto, client, CLI caller side, goldens; blocks everything else)*
- [ ] T02 — Put `fleet` on the child's PATH, print it in the footer, and stop warning on an explicit `--worktree` *(daemon + core + NATIVE-AGENTS + ADR 0017)*
- [ ] T03 — CLI: uncap `wait` and report a running timeout honestly, add `--effort`, add `tail --no-follow` and `--last` *(all of fleet-cli + agents-contracts + README)*
- [ ] T04 — Report the directory `fleet doctor` would inject for subagents *(doctor + doctor_checks + DEVELOPMENT)*
- [ ] T05 — Verify the batch end to end and record what is still deferred *(SESSION_TODO)*
- [ ] T06 — Let `--effort` stand alone *(added mid-flight; core + daemon + fleet-cli + NATIVE-AGENTS + contracts + README)*
- [ ] T07 — A resumed child keeps its PATH injection *(added mid-flight; daemon manager + NATIVE-AGENTS)*

### Ownership at a glance

| Task | Owns exclusively | Starts after |
| --- | --- | --- |
| T01 | `crates/fleet-proto/src/request.rs`, `crates/fleet-proto/src/request/tests.rs`, `crates/fleet-proto/tests/agent_compatibility.rs`, `crates/fleet-client/src/api/agents.rs`, `crates/fleet-cli/src/commands/subagents.rs`, `crates/fleet-cli/src/commands/tests.rs` | — |
| T02 | `crates/fleet-core/src/agents/provider.rs`, `crates/fleet-daemon/src/agents/harness/process.rs`, `crates/fleet-daemon/src/agents/claude/mod.rs`, `crates/fleet-daemon/src/agents/codex/mod.rs`, `crates/fleet-daemon/src/services/agents/manager/commands.rs`, `crates/fleet-daemon/src/services/agents/delegation/run.rs`, `crates/fleet-daemon/src/services/agents/delegation/footer.rs`, `crates/fleet-daemon/src/services/agents/delegation/tests/run.rs`, `docs/NATIVE-AGENTS.md`, `docs/decisions/0017-native-subagents.md` | T01 committed |
| T03 | `crates/fleet-cli/src/args.rs`, `crates/fleet-cli/src/commands/subagents.rs`, `crates/fleet-cli/src/commands/agents.rs`, `crates/fleet-cli/src/human.rs`, `crates/fleet-cli/src/commands/tests.rs`, `docs/research/agents-contracts.md`, `README.md` | T01 committed; its final §15 doc check waits for T02 |
| T04 | `crates/fleet-daemon/src/services/doctor.rs`, `crates/fleet-daemon/tests/doctor_checks.rs`, `docs/DEVELOPMENT.md` | T01 committed |
| T05 | `SESSION_TODO.md` | T02, T03, T04 all committed |

## Notes / decisions log

*(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)*

- **2026-09-19 — plan written.** Five of eight reported orchestrator pain points are in scope.
  Sized Standard, five tasks. The cut is by **file ownership**, not one-task-per-reported-item,
  because items 2/3/5 all rewrite `crates/fleet-cli/src/args.rs` and items 1/4 both rewrite the
  daemon's `delegation/run.rs` and `footer.rs`. Full reasoning in the plan's *Concurrency and
  file ownership* section.
- **2026-09-19 — item 4 needs no proto change.** `RequestBody::DelegationRun` already carries
  `worktree: Option<WorktreeId>`, and `run.rs` resolves the default with `unwrap_or_else`, so the
  daemon can already tell an explicit `--worktree` from the implicit caller default. No golden
  regeneration for this item. Only T01's new field touches
  `crates/fleet-proto/tests/agent_compatibility.rs`.
- **2026-09-19 — item 3 needs no proto change either.** `ModelSelection.effort` is already on the
  wire and already appears in the populated `delegation_run` golden.
- **2026-09-19 — `docs/SWARM-INVENTORY.md` is not in scope.** It was named as an authoritative
  doc for this work, but it contains no reference to subagents, delegations or `fleet subagent`;
  it is the Swarm compatibility baseline and there is no Swarm counterpart to deviate from.
- **2026-09-19 — no `install` target in the `Makefile`.** `make build` already places `fleet` and
  `fleetd` side by side in `target/$(PROFILE)/`, which is what makes T01's `current_exe` approach
  work locally. No Makefile change planned. The remote path
  (`crates/fleet-daemon/src/services/bootstrap.rs`) installs only `fleetd`, which is the case
  T02's daemon-side fallback covers.
- **2026-09-19 — `make harness` is not required for this batch.** Nothing here renders. Confirm
  in T05 rather than skipping silently.
- **2026-09-19 — deliberate deviation from the same-commit doc rule.** T02 writes the
  `docs/NATIVE-AGENTS.md` §15 paragraphs for items 2, 3 and 5 even though that code lands in T03,
  because §15 is touched by four of five items and serialising it would serialise the batch. T03
  re-checks §15 last. Named and bounded; do not generalise it.

## Follow-ups

*(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)*

Carried over from `SESSION_TODO.md` — deferred, **not** part of this batch:

- **Cost visibility in `fleet agent list` / `fleet subagent status`.** Usage exists per
  `ThreadProjection` but not on `AgentThreadSummary` or the `threads` table; tree rollups need
  design (resumed processes must not double count).
- **Repeatable `--env KEY=VALUE` on `fleet subagent run`.** The daemon already has a per-child
  `extra_env`; the work is CLI/client/proto plumbing plus goldens and docs.
- **Result delivery for orchestrators that never end their turn.** Check the `elided` threshold
  on `DelegationResult.text` before touching the ADR 0017 end-of-turn policy; revisit only if
  uncapping `wait` (item 2 here) proves insufficient.
- **`fleet subagent run --json` echoes the whole brief back.** A 250-line brief costs the
  orchestrator 250 lines of context per launch; elide or omit `brief` in the run envelope, or add
  a `--quiet`.

Discovered during this batch:

*(none yet)*
