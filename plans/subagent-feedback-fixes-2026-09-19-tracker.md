# Native subagent feedback fixes (batch 1) — Tracker

> Plan: ./subagent-feedback-fixes-2026-09-19-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

**Status: complete, 2026-09-19.** Seven tasks, nine commits on `fix/native-subagents`, nothing
pushed. `make lint` is green and the live field check passed every assertion against a private
daemon. `make test` is **red**, in `fleet-app --test harness_headless` alone, for two reasons
proved by A/B against `703fe95` to predate this batch — see Notes and `SESSION_TODO.md`. The one thing the batch could not do is restart the *shared* `fleetd`,
which hosted the session that executed it — `SESSION_TODO.md` says exactly what a human must do
next, and why `make restart` needs a rebase first.

| Commit | Task |
| --- | --- |
| `fdaabfc` | T01 — `proto: carry the caller's own fleet path on delegation_run` |
| `557be77` | T04 extra — `harness: gate the window origin to the platforms that read it` |
| `8621493` | T03 — `cli: uncap subagent wait, add run --effort and tail --no-follow/--last` |
| `f0d17f8` | T04 — `daemon: report the directory doctor would inject for subagents` |
| `703fe95` | T02 — `daemon: put a fleet on the delegated child's PATH and stop warning on an explicit --worktree` |
| `0a1dbc3` | T06 — `daemon: let subagent --effort stand alone without --model` |
| `7d40fca` | T07 — `daemon: keep a fleet on a resumed delegated child's PATH` |
| `35e1912` | T05 — `docs: consolidate the subagent feedback batch tracker and session TODO` |

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

**This section is history now.** By the time T06, T07 and T05 ran, every other task had
committed and nobody else held the worktree, so those three worked without the ownership
constraint. It is kept because it explains why the first four tasks are cut the way they are.

## Kickoff

- [x] I have read the plan end to end.
- [x] I have read `CLAUDE.md` at the repo root, and the skill named by my task in
      `.claude/skills/`.
- [x] I have run `make lint` and `make test` once on a clean tree to confirm a green baseline.
      **Ticked with a correction:** the baseline was **not** green. `make lint` failed on
      `crates/fleet-harness/src/lane.rs:935` before any task touched anything — a pre-existing
      macOS dead-code error from `05986a0` (2026-09-18), unowned by this plan. T01 found it, T04
      fixed it in `557be77`, and every task after that ran against a genuinely green tree. The
      box is ticked because the check was performed; the answer it gave is in Notes.
- [x] I know which files my task owns and which it does not.
- [x] I am ready to start.

## Tasks

- [x] T01 — Send the caller's own `fleet` executable path with `subagent run` *(proto, client, CLI caller side, goldens; blocks everything else)* — `fdaabfc`
      verified: `cargo test -p fleet-proto` (42/21/9/1 pass), `cargo test -p fleet-client` (33/5/7/2/6/1 pass), `cargo test -p fleet-cli` (112/9 pass), `cargo clippy -p fleet-proto -p fleet-client -p fleet-cli -p fleet-daemon --all-targets --all-features -- -D warnings` clean, `cargo fmt --all -- --check` clean; `make lint` failed only on the pre-existing `fleet-harness` dead-code error unrelated to this task (see Notes, fixed by `557be77`).
- [x] T02 — Put `fleet` on the child's PATH, print it in the footer, and stop warning on an explicit `--worktree` *(daemon + core + NATIVE-AGENTS + ADR 0017)* — `703fe95`
      verified: `cargo test -p fleet-core -p fleet-daemon` pass, `cargo test -p fleet-daemon delegation` pass (103), `cargo fmt --all -- --check` pass, `cargo clippy --workspace --all-targets --all-features -- -D warnings` pass; `make restart`/`make doctor` deliberately not run (the live daemon hosts the delegating session); no GUI surface, so `make harness` is not applicable.
- [x] T03 — CLI: uncap `wait` and report a running timeout honestly, add `--effort`, add `tail --no-follow` and `--last` *(all of fleet-cli + agents-contracts + README)* — `8621493`
      verified: `cargo test -p fleet-cli` 126 pass, `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; manually against the live daemon: `wait --timeout 3` on a running child prints the still-running line and exits 2, `wait --timeout 3600` is accepted (was a clap error) and returns the report at exit 0, `agent tail <thread> --no-follow --last 3` prints three events and exits 0. `--effort` without `--model` was refused here; **superseded by T06**, which makes it work. `make harness` not run — nothing in this task renders. `make test` not run — T02/T04 daemon edits were in flight; the full-workspace clippy compiled them clean.
- [x] T04 — Report the directory `fleet doctor` would inject for subagents *(doctor + doctor_checks + DEVELOPMENT)* — `f0d17f8`; extra item `557be77`
      verified: `cargo test -p fleet-daemon doctor` ok (32 result lines, 0 failures), `cargo test -p fleet-daemon --test doctor_checks` 5/5, `cargo clippy -p fleet-daemon --all-targets --all-features -D warnings` ok, `cargo fmt --all -- --check` ok, `cargo clippy --workspace --all-targets --all-features -D warnings` ok; `make restart`/`make doctor` deferred to T05 (must not restart the hosting daemon), and T05 could not run them either — see Notes.
- [x] T06 — Let `--effort` stand alone *(added mid-flight; core + daemon + fleet-cli + NATIVE-AGENTS + contracts + README)* — `0a1dbc3`
      verified: `cargo test -p fleet-core -p fleet-cli` pass (216 + 119 + 9), `cargo test -p fleet-daemon --lib` 782 pass, `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean. New cases: the four-way resolution matrix in `manager/commands.rs`, the empty-model launch line in `claude/argv.rs`, the empty-model `start_params` golden in `codex/tests/wire.rs`, the `model_selection` matrix and a wire-level `subagent run --effort` with no `--model` in `fleet-cli`.
- [x] T07 — A resumed child keeps its PATH injection *(added mid-flight; daemon manager + NATIVE-AGENTS)* — `7d40fca`
      verified: `cargo test -p fleet-daemon --lib` 783 pass including `a_resumed_delegated_child_keeps_the_daemon_sibling_on_its_path`, `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
- [x] T05 — Verify the batch end to end and record what is still deferred *(SESSION_TODO + this tracker + doc reconciliation)* — `35e1912`
      verified: `make lint` exit 0 on the settled tree; `cargo test` green for `fleet-core`, `fleet-cli`, `fleet-proto`, `fleet-client` and `fleet-daemon --lib` (16 `test result: ok` lines, 0 failures) plus `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; **`make test` red in `fleet-app --test harness_headless` only, proved pre-existing by A/B against `703fe95`** (see Notes); the four doc seams checked and every divergence fixed; the live field check run against a **private** daemon rather than `make restart`, passing every assertion.

### Ownership at a glance

| Task | Owns exclusively | Starts after |
| --- | --- | --- |
| T01 | `crates/fleet-proto/src/request.rs`, `crates/fleet-proto/src/request/tests.rs`, `crates/fleet-proto/tests/agent_compatibility.rs`, `crates/fleet-client/src/api/agents.rs`, `crates/fleet-cli/src/commands/subagents.rs`, `crates/fleet-cli/src/commands/tests.rs` | — |
| T02 | `crates/fleet-core/src/agents/provider.rs`, `crates/fleet-daemon/src/agents/harness/process.rs`, `crates/fleet-daemon/src/agents/claude/mod.rs`, `crates/fleet-daemon/src/agents/codex/mod.rs`, `crates/fleet-daemon/src/services/agents/manager/commands.rs`, `crates/fleet-daemon/src/services/agents/delegation/run.rs`, `crates/fleet-daemon/src/services/agents/delegation/footer.rs`, `crates/fleet-daemon/src/services/agents/delegation/tests/run.rs`, `docs/NATIVE-AGENTS.md`, `docs/decisions/0017-native-subagents.md` | T01 committed |
| T03 | `crates/fleet-cli/src/args.rs`, `crates/fleet-cli/src/commands/subagents.rs`, `crates/fleet-cli/src/commands/agents.rs`, `crates/fleet-cli/src/human.rs`, `crates/fleet-cli/src/commands/tests.rs`, `docs/research/agents-contracts.md`, `README.md` | T01 committed; its final §15 doc check waits for T02 |
| T04 | `crates/fleet-daemon/src/services/doctor.rs`, `crates/fleet-daemon/tests/doctor_checks.rs`, `docs/DEVELOPMENT.md` | T01 committed |
| T05 | `SESSION_TODO.md`, this tracker, and whatever doc divergence the cross-check finds | T02, T03, T04, T06, T07 all committed |
| T06 | `crates/fleet-core/src/agents/state.rs`, `crates/fleet-daemon/src/services/agents/manager/commands.rs`, `crates/fleet-daemon/src/agents/claude/argv.rs`, `crates/fleet-daemon/src/agents/codex/params.rs`, `crates/fleet-daemon/src/agents/codex/tests/wire.rs`, `crates/fleet-cli/src/args.rs`, `crates/fleet-cli/src/commands/subagents.rs`, `crates/fleet-cli/src/commands/tests.rs`, `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`, `README.md` | T03 committed |
| T07 | `crates/fleet-daemon/src/services/agents/manager.rs`, `crates/fleet-daemon/src/services/agents/manager/tests/restart.rs`, `crates/fleet-daemon/src/services/agents/delegation/{mod.rs,run.rs}`, `docs/NATIVE-AGENTS.md` | T02 committed |

By the time T06 and T07 ran, every other task had committed and nobody else held the worktree, so
their ownership rows are a record of what they touched rather than a constraint they worked under.

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


### From the tasks themselves

- **2026-09-19 — T01's field is `fleet_path: Option<String>`** on both
  `RequestBody::DelegationRun` (after `title`, before `eager`) and
  `fleet_client::DelegationRunRequest`. `String`, not `PathBuf`, on both types and on the wire,
  so the one conversion happens at the CLI and a non-UTF-8 path becomes an honest `None` instead
  of a `to_string_lossy` mangling. It is the absolute, canonicalised path of the `fleet` **file**;
  T02 wants its `.parent()`.
- **2026-09-19 — no capability string for `fleet_path`, and no `PROTOCOL_VERSION` bump.** Rule 8
  of `rust-ipc-protocol` wants a capability when optional *behaviour* must be advertised. A caller
  sending a hint needs no answer about whether the daemon honoured it, and an older caller
  omitting the field is byte-indistinguishable from one that could not resolve a path. Additive
  `#[serde(default, skip_serializing_if)]` is the whole mechanism.
- **2026-09-19 — the ownership table had a hole, and T01 had to fill part of it.** Adding a field
  to `RequestBody::DelegationRun` breaks every exhaustive destructuring of the variant. Three such
  sites existed and no task owned any of them: `crates/fleet-daemon/src/services/dispatch.rs`
  (production `match` arm), `crates/fleet-daemon/src/services/router/classify.rs` (`mod tests`),
  and `crates/fleet-client/src/connection.rs` (`mod tests`). T01 made the minimum edit in each,
  because T02/T03/T04 were all gated on the tree compiling and would otherwise have started
  against a broken build. T02 then turned `dispatch.rs`'s `fleet_path: _` into a real binding.
- **2026-09-19 — `make lint` was RED on the "clean" baseline, for a reason unrelated to this
  batch.** `crates/fleet-harness/src/lane.rs:935`, `error: field at is never read`, under
  `-D dead-code`; introduced by `05986a0` (2026-09-18). Verified by stashing all of `crates/` and
  re-running clippy, not inferred. No task in this plan owned `crates/fleet-harness/`; T04 fixed
  it in `557be77` by giving the field the same `#[cfg(not(target_os = "macos"))]` predicate its
  only consumer `capture::window_rect` carries — not an `allow(dead_code)`, because `05986a0`
  shows the field was once *deleted* as unused and that broke the Linux build.
- **2026-09-19 — T02: the caller-hint branch checks `is_file()` on the hint itself**, not just
  that its parent directory exists (the plan's wording). Strictly stronger: it still identifies
  the same-host case, and it is what makes the absolute path written into the child's footer a
  path that resolves. A directory-only check would let a remote host with a coincidentally
  present directory get a useless `PATH` prepend and a broken footer path.
- **2026-09-19 — T02: the footer's program token is shell-quoted** (`shell_words::quote`). The
  footer is a command line the child copies verbatim, so a Fleet installed under a path with a
  space must still produce something runnable. A no-op for ordinary paths, so the verbatim
  contract test holds.
- **2026-09-19 — T02: `path_prepend` is a first-class `StartRequest`/`CreateOptions` field**
  rather than an `extra_env` entry, per the plan: both adapters treat `env` as a whole-value
  override, so a `PATH` there would replace the login shell's rather than extend it.
  `prepend_path` lives in `agents/harness/process.rs`, uses `split_paths`/`join_paths`, is
  idempotent, and is called after `filter_environment` so that signature is unchanged.
  Resolution is logged once per delegation: `info` with the chosen directory and the rule that
  chose it, `warn` when neither rule did.
- **2026-09-19 — T02: `SAME_WORKTREE_WARNING` was reworded as well as re-gated**, so the text
  itself names the implicit default: "no --worktree was passed, so the child edits the caller's
  worktree by default; end your turn before it works, or pass --worktree to isolate it".
  ADR 0017 was amended in its own voice with the original reasoning kept, both for the warning
  exemption and for 540 seconds becoming a default with no ceiling.
- **2026-09-19 — T02: adding a `StartRequest` field required `path_prepend: None` in six struct
  literals outside T02's ownership** (`services/agents/manager.rs`,
  `services/agents/providers/mod.rs`, `agents/claude/argv.rs`, `agents/claude/tests/mod.rs`,
  `agents/codex/tests.rs`, `tests/agents_real_binaries.rs`). The one in `manager.rs` was the
  resume path, and that `None` was a real bug — **fixed in T07**.
- **2026-09-19 — T03: `--no-follow` and `--last N` both imply `--replay`**; a flag that printed
  nothing would repeat the bug they exist to fix. `--last` is a **print** filter only: the
  trimmed prefix is still folded into the projection, because `terminal()` and every later
  `seq <= last_seq` comparison read it. While following, `--last` bounds the replay alone.
- **2026-09-19 — T03: the `wait` timeout cap really did live only in clap.** `fleet-proto`
  carries an unrestricted `u64` and `fleet-client/src/connection.rs` already grants
  `timeout_ms + 15 s` with no ceiling of its own; no change was needed below the CLI. The
  `--json` output of `wait` is byte-identical before and after and is now pinned as a literal.
- **2026-09-19 — T03: `README.md` had no `fleet subagent` rows at all**; the six verbs were added
  rather than leaving the group undocumented.
- **2026-09-19 — T04: doctor names the injected directory, and the middle shape is a pass.**
  `subagent_fleet_check` now takes the daemon's own executable and reports the directory holding
  its sibling `fleet`. Bare `fleet` unresolvable *but* a sibling present is `Ok` with an
  explanation, not `Warn`: the question the check answers is "can a child report", and with a
  directory to inject the answer is yes. `Fail` survives only when neither exists. T04
  deliberately does not use T02's helper — the directory is recomputed from
  `current_exe().parent()/fleet` so the two tasks could run in parallel — and doctor cannot
  report the caller-supplied `fleet_path`, because no delegation is in flight while it runs.
  That limit is in the function's doc comment. `cargo test -p fleet-daemon doctor` filters on
  test *name* and misses the new `subagent_fleet_check_*` cases; use
  `cargo test -p fleet-daemon --test doctor_checks` too.
- **2026-09-19 — T06: the plan's Assumptions were wrong about `create_with`,** which is why T03
  shipped `--effort` without `--model` as a validation error. `create_with` substituted
  `defaults.model` only when the **whole** `ModelSelection` was absent. T06 gives the pairing a
  representation instead of keeping the refusal: the empty string on `ModelSelection::model` is
  now the documented "keep the configured default" sentinel, distinct from an absent
  `ModelSelection`, which still means "no opinion at all". The field could not become an
  `Option` — it is on the wire with byte-exact goldens. Claude's `--effort` turns out to be a
  session-level flag in its own right (`claude --help`), not a qualifier on `--model`, so the
  adapter emits it independently; Codex reads `model_reasoning_effort` from a separate config
  key and only had to stop inserting `"model": ""`. A *present but blank* `--model ""` is still
  a validation error. The plan's Assumptions bullet was amended in place with the wrong
  reasoning kept on the record.
- **2026-09-19 — T07: rule 1 of the `PATH` resolution is genuinely unrecoverable on resume.**
  The caller's `fleet_path` hint describes a process that has already exited and is deliberately
  not persisted, so the resume path re-runs **rule 2 only** — a `fleet` beside this daemon's own
  `fleetd` — for any thread that has a delegation. No new durable state, no migration. A resumed
  child may therefore get a different directory from the one it was first started with, which is
  stated in `docs/NATIVE-AGENTS.md` §15.

### T05 — the closing verification

- **2026-09-19 — `make harness` was not run, and that is correct.** Nothing in this batch renders:
  it is CLI argument parsing, daemon child-environment construction, one output string and
  contract text. No screen, dialog, keymap or token changed. The plan says so under *Repository
  context* and this is the confirmation it asked for rather than a silent omission.
- **2026-09-19 — T03's effort-only refusal was superseded by T06.** Anything that still reads as
  if `fleet subagent run --effort X` needs `--model` is stale; the shipped behaviour is that it
  stands alone and the child keeps the provider's configured default model.
- **2026-09-19 — `make restart`, `make run`, `make daemon` and `fleet daemon restart` were
  forbidden for the whole batch**, because the shared `fleetd` hosts the delegating session that
  executed it. No task ran them, so `make doctor` — T04's last verification step — could not be
  run against a daemon carrying this batch either. T05 booted a **private** daemon from
  `target-t05/debug/fleetd` under its own `FLEET_HOME` for the live field check instead. The
  shared daemon is still running pre-batch code until a human restarts it; `SESSION_TODO.md`
  says exactly what to do then.
- **2026-09-19 — the three doc seams the plan asked T05 to check.** (a) `fleet_path` is produced
  by `fleet-cli`'s `caller_fleet_path`, carried by `fleet-client` and `fleet-proto`, bound in
  `dispatch.rs` and consumed by `resolve_fleet_program` in `delegation/run.rs` — one unbroken
  chain, no dead code. (b) §15 matches what T03 shipped for the `wait` timeout, `--effort` (once
  T06 landed) and the two `tail` flags. (c) `docs/NATIVE-AGENTS.md` §15.3, ADR 0017's amendment,
  `docs/research/agents-contracts.md` and `README.md` all state the same `wait` contract: 540 s
  default, no upper bound, exit 2 with a non-terminal line, report body returned on success.

- **2026-09-19 — the live field check ran against a private daemon and passed every assertion.**
  `target-t05/debug/fleetd` under `FLEET_HOME=/tmp/fleet-t05-home`, with a hand-seeded
  `state.json` naming only `dannyfuf/fleetd#fix-native-subagents` and `hosts` cleared. A Claude
  caller thread was created, put in a running turn, and delegated one child with
  `--worktree dannyfuf/fleetd#fix-native-subagents --effort high` and **no** `--model`. Results:
  the run was accepted at exit 0 with **no same-worktree warning**; `wait --timeout 900` was
  accepted (a clap error before T03) and printed the child's report at exit 0;
  `agent tail <child> --no-follow --last 5` printed five events and exited 0; the child's
  `command -v fleet` resolved to
  `/Users/danny/.swarm/worktrees/dannyfuf/fleetd/fix-native-subagents/target-t05/debug/fleet` —
  the caller's own directory, first in its `PATH` — and it reported with a bare
  `fleet subagent complete`. The daemon logged
  `prepending a fleet directory to the delegated child's PATH source="caller"`. Items 1, 2, 3, 4
  and 5 are all confirmed against real binaries.
- **2026-09-19 — `FLEET_DAEMON` in a delegated child's environment points at the *shared*
  daemon binary, and that nearly wrecked the field check.** The first private daemon the CLI
  auto-spawned was `/Users/danny/.swarm/repos/dannyfuf/fleetd/target/release/fleetd`, inherited
  through `FLEET_DAEMON`, which applied a migration this branch does not have to the private
  home. Anyone booting a private daemon from a worktree must override `FLEET_DAEMON` explicitly;
  `FLEET_HOME` alone is not enough. Not a bug — `FLEET_DAEMON` is doing exactly its job — but a
  sharp edge worth knowing.
- **2026-09-19 — this branch cannot restart the shared daemon until it is rebased.** The running
  release `fleetd` has already applied agent-database migration slot 5 (`closed_threads`) to
  `~/.fleet/agents/state.sqlite`; this branch knows slots 1-4, so a `fleetd` built from it
  refuses to open that database. Recorded as the first step of `SESSION_TODO.md`'s post-batch
  checklist, because `make restart` is otherwise the obvious next command and it would take
  every native-agent verb down.

- **2026-09-19 — `make test` is RED, and the cause is not this batch. Proved by A/B, not argued.**
  `cargo test -p fleet-app --test harness_headless` fails; every other suite that ran was green
  (862 tests, 0 failures), and cargo aborts the remaining targets once one fails. Three separate
  things are tangled in that one red suite, and they were separated deliberately:
  1. **All 20 scenarios fail on the default `$TMPDIR`**, before any scenario logic runs, with
     `filesystem operation failed for /var/folders/…/home/fleetd.sock: path must be shorter than
     SUN_LEN`. The macOS per-user temp directory pushes the harness run directory's socket past
     the 104-byte `sockaddr_un` limit. T04 had already reported this family. Re-running the same
     suite with `TMPDIR=/tmp/ht` makes the fixture daemon start — the run-directory names even
     stop being truncated — so this one is environmental and fully explained.
  2. **With a short `$TMPDIR`, 5 pass and 15 fail.** The 5 that pass (`daemon/link-recovers`,
     `hub/idle-accounting`, `hub/idle-after-context-mutation`, `hub/pointer-tabs`,
     `agents/subagent-reopen-closed-caller`) are exactly the ones that never drive a scripted
     agent turn; all 15 failures do.
  3. Of those 15, **`agents/claude-mode-menu` passes in isolation at HEAD**, so its failure in the
     batch is load or sequencing (20 scenarios each booting their own `fleetd` and GUI, on a
     machine also compiling). **`agents/subagent-runs-end-to-end` fails in isolation at HEAD and
     fails identically at `703fe95`** — the commit before T06 — with
     `agents.delegations[0].status is missing from the snapshot`. Pre-existing.

  The A/B was run properly: `git checkout 703fe95 -- crates/`, full rebuild, run, restore,
  rebuild. `agents/claude-mode-menu` passes on both sides; `agents/subagent-runs-end-to-end`
  fails on both sides. Neither T06 nor T07 changes the outcome. Neutralising T07's
  `path_prepend` alone also changed nothing.

  **What this means for the batch:** `make test` cannot be reported green on this machine, and it
  was not green before this batch either — no task in it ever got a full `make test` to pass.
  The suite also warns it now costs 500s against a 60s budget. It needs an owner; the fixes are
  a short `$TMPDIR` (or a shorter run-directory root), and whatever is wrong with the delegation
  fixtures. Both are in `SESSION_TODO.md`.

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

- **`crates/fleet-daemon/tests/pty_holder.rs:117` is flaky under parallel load.** It failed once
  during T02's first full test run — the greeting reported `(120, 36)` where the test wanted
  `(101, 31)` — and passed 3/3 in isolation immediately after. A PTY window-size race in the
  holder reattach path: the greeting's reported size races the resize applied before the
  disconnect. Nothing in this batch touches PTY, `fleet-term` or holder code. It will bite CI.
  Carried into `SESSION_TODO.md`.
- **`crates/fleet-harness/src/capture.rs` has a macOS `window_rect` that only bails.** The cfg
  split `557be77` put on `Client::at` exists to mirror it. The better fix is one `window_rect`
  over `lane::clients()` for every platform, or a real macOS implementation. Out of this batch's
  ownership; `docs/TESTING-HARNESS.md` is frozen, so check it before changing the message.
  Carried into `SESSION_TODO.md`.
- **`fleet-harness` tests need `FLEET_DAEMON`/`FLEET_APP`/`FLEET_HARNESS_BIN` and a short
  `$TMPDIR`.** Run under a stale or missing `target/debug/fleetd`, nine fixture cases fail on
  "fleetd did not become ready within 20s"; one more fails outright on this machine because the
  default macOS `$TMPDIR` pushes the run directory's socket past the 107-byte limit. `make test`
  sets the three variables but cannot shorten `$TMPDIR`. Worth a clearer failure or a documented
  `HARNESS_RUNS`.
- **`557be77` was not compiled on Linux.** Only `aarch64-apple-darwin` is installed here, so the
  `lane.rs` cfg is argued structurally — the predicate on the field is character-identical to the
  one on its only consumer — rather than proven by a build. Worth one CI check on Linux.
- **This branch must be rebased before `make restart`.** The running release `fleetd` applied
  agent-database migration slot 5 (`closed_threads`); this branch knows 1-4 and refuses to open
  the database. First item of `SESSION_TODO.md`'s post-batch checklist. Not a bug in this batch,
  but the batch cannot be validated on the shared daemon until it is handled.
- **`FLEET_HOME` alone does not isolate a daemon.** A delegated child inherits `FLEET_DAEMON`
  pointing at the shared binary, so a private daemon must override it too. Worth a sentence in
  `docs/DEVELOPMENT.md` next time someone edits it; not worth a commit of its own here.
- **Two mid-flight tasks were added and closed inside this batch**, T06 (`--effort` stands alone)
  and T07 (a resumed child keeps its `PATH`). They are in *Tasks* above, not here, because they
  shipped.
