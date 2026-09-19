# Session TODO — native subagent feedback, deferred items

Source: an orchestrator agent's field report after driving `fleet subagent` end to end
(2026-09-19). Items 1-5 are planned in `plans/subagent-feedback-fixes-2026-09-19-plan.md` and
have **shipped** on `fix/native-subagents`, along with two mid-flight additions (T06, T07). The
items below were deliberately deferred; pick them up once the first batch has been validated by
another orchestrator run.

## Deferred

- [ ] **Cost visibility in `fleet agent list` / `fleet subagent status`.** Tokens and
  cumulative cost already exist per `ThreadProjection` (`cumulative_usage`,
  `crates/fleet-core/src/agents/projection/usage.rs`) and the GUI shows them, but
  `AgentThreadSummary` and the denormalized `threads` table carry none of it.
  Cheap first step: show the child's usage in `fleet subagent status` by opening the
  child projection (no migration). Rollups across a delegation tree need design:
  resumed processes must not double count, and "includes descendants" must be defined.
- [ ] **Repeatable `--env KEY=VALUE` on `fleet subagent run`.** Motivated by cargo
  build-lock contention when several children share one worktree (`CARGO_TARGET_DIR`).
  The daemon already has a per-child `extra_env` map (`CreateOptions.extra_env`);
  the work is plumbing through `fleet-cli`, `fleet-client`, `fleet-proto`
  `DelegationRun`, goldens and docs. Do not put cargo-specific advice in the generic
  footer; that belongs in the orchestrator's brief.
- [ ] **Result delivery for orchestrators that never end their turn.** The end-of-turn
  default is an ADR 0017 decision and `--eager` exists. Before changing policy, check
  the `elided` threshold on `DelegationResult.text`: the orchestrator read report files
  from `/tmp` even though `wait` already returns the body, which suggests elision kicked
  in or the behavior was not discoverable. `wait` is now uncapped (batch item 2); consider
  a `--follow` on `wait` only if that proves insufficient.
- [ ] **`fleet subagent run --json` echoes the whole brief back.** The envelope includes
  `delegation.brief` verbatim, so a 250-line brief costs the orchestrator 250 lines of
  context per launch. Elide or omit `brief` in the run envelope (keep it in `status`),
  or add a `--quiet` that returns only id, child and status. Found 2026-09-19 while
  dispatching the planner for this batch.

## Found during the batch, not fixed by it

- [ ] **`crates/fleet-daemon/tests/pty_holder.rs:117` is flaky under parallel load.** It failed
  once during T02's run — `a_holder_outlives_its_daemon_and_replays_what_the_shell_already_printed`
  asserted the greeting reports the *current* window size and got `(120, 36)` where it wanted
  `(101, 31)` — and passed 3/3 in isolation immediately afterwards. The greeting's reported size
  races the resize applied before the disconnect. Nothing in this batch touches PTY, `fleet-term`
  or holder code, so it is pre-existing; it will bite CI before it bites a developer.
- [ ] **`crates/fleet-harness/src/capture.rs` has a macOS `window_rect` that only bails.**
  `557be77` gave `lane::Client::at` the `#[cfg(not(target_os = "macos"))]` predicate its only
  consumer carries, which fixes the dead-code error without repeating the deletion that broke the
  Linux build in `05986a0`. The real fix is one `window_rect` over `lane::clients()` for every
  platform, or a real macOS implementation. `docs/TESTING-HARNESS.md` is **frozen** — read it
  before changing a command, a target name, the grammar or that message.

- [ ] **`make test` is red on this machine, for two reasons, neither of them the subagent batch.**
  Proved by A/B against `703fe95`, not inferred.
  1. **`$TMPDIR` breaks every harness scenario before it starts.** The macOS per-user temp
     directory pushes the run directory's socket past the 104-byte `sockaddr_un` limit:
     `filesystem operation failed for /var/folders/…/home/fleetd.sock: path must be shorter than
     SUN_LEN`. `TMPDIR=/tmp/ht make test` gets past it. Worth fixing in the harness — a short
     fixed run-directory root, or an explicit failure that names `$TMPDIR` — because right now it
     reads as "fleetd did not become ready within 20s", which sends you looking in the wrong
     place. `docs/TESTING-HARNESS.md` is **frozen**; read it before changing a message.
  2. **The delegation fixtures fail even with a short `$TMPDIR`.**
     `agents/subagent-runs-end-to-end` times out on
     `agents.delegations[0].status is missing from the snapshot`, in isolation, at HEAD **and**
     identically at `703fe95`. Same for `subagent-attach-from-picker`. Fifteen scenarios fail
     this way — precisely the ones that drive a scripted agent turn; the five that do not
     (`daemon/link-recovers`, the three `hub/*`, `subagent-reopen-closed-caller`) pass.
     `agents/claude-mode-menu` passes alone and fails in the batch, so some of the 15 are load or
     sequencing rather than logic.
  3. The suite also warns that the headless subset now costs `make test` 500s against a 60s
     budget and should move back to `make harness-headless`.

## Verify after the first batch lands

The shared `fleetd` on this machine **does not contain this batch**: every task in it was
forbidden from restarting the daemon, because that daemon was hosting the session that executed
the batch. It is a release build from `~/.swarm/repos/dannyfuf/fleetd`, on a commit newer than
this branch in some respects and older in all of these. Nothing below can be observed until it
is replaced. In order:

- [ ] **Rebase this branch onto `main` first — `make restart` will otherwise break the daemon.**
  The running `fleetd` is
  `/Users/danny/.swarm/repos/dannyfuf/fleetd/target/release/fleetd`, built from a commit *newer*
  than this branch, and it has already applied agent-database migration slot 5
  (`closed_threads`) to `~/.fleet/agents/state.sqlite`. This branch knows migrations 1-4 only, so
  a `fleetd` built from it refuses to open that database — verified: "the agent database records
  migration slot 5 (closed_threads), which this build does not know: it was written by a newer
  fleetd. Refusing to open it". Every native-agent verb then fails. Rebase, rebuild, and only
  then restart.
- [ ] **`make restart`.** This is the step. Until it runs, `fleet subagent run` goes on producing
  children with no `fleet` on their `PATH`, the old same-worktree warning and the old footer,
  however new the `fleet` binary in your shell is — all three behaviours live in the daemon.
- [ ] **`make doctor`, then read the `subagent fleet CLI` line.** It is the one verification step
  T04 could not run. Expect `ok` naming the resolved child path and, when a `fleet` sits beside
  `fleetd`, `· children also get <dir> prepended to PATH`. A `fail` here means no child can
  report and nothing else in this list is worth trying.
- [ ] **Re-run an orchestrator with four concurrent children in one worktree** and confirm, per
  child: no absolute `fleet` path is needed anywhere in the brief (bare `fleet subagent complete`
  works); one `wait` per child is enough and a `--timeout` above 540 is accepted; an explicit
  `--worktree` produces no same-worktree warning while omitting it still does;
  `fleet agent tail <child> --no-follow --last 20` prints a snapshot and exits 0; and
  `fleet subagent run --effort high` with no `--model` starts a child at that effort.
- [ ] **Kill one child's provider mid-run and let the daemon recover it**, then confirm the
  recovered child can still run bare `fleet subagent complete` (T07). This is the one behaviour
  with no end-to-end test: the unit test covers the resolution rule, not a real recovery.

**Already verified against a private daemon (2026-09-19, T05).** Rather than restart the shared
`fleetd`, T05 booted one from `target-t05/debug/fleetd` under `FLEET_HOME=/tmp/fleet-t05-home`
with a hand-seeded `state.json` naming this one worktree, created a Claude caller thread, put it
in a running turn and delegated one child with
`--worktree dannyfuf/fleetd#fix-native-subagents --effort high` and no `--model`. All of it held:
the run was accepted with **no same-worktree warning**, `wait --timeout 900` was accepted and
printed the child's report at exit 0, `agent tail <child> --no-follow --last 5` printed five
events and exited 0, the child's own `command -v fleet` came back as
`.../target-t05/debug/fleet` — the caller's directory, first in its `PATH` — and it reported with
a bare `fleet subagent complete`. The daemon logged
`prepending a fleet directory to the delegated child's PATH source="caller"`. What is left above
is the four-concurrent-children load case, the recovery case, and everything that needs the
*shared* daemon.
