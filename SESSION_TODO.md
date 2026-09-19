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

- [ ] **`make test` is red on this machine unless `$TMPDIR` is both short *and* canonical.**
  The delegation-fixture half of this item is **fixed**: `e4d6f66` clears the six variables an
  outer `fleetd` exports (`FLEET_DELEGATION`, `FLEET_DELEGATION_TOKEN`, `FLEET_SESSION`,
  `FLEET_TERMINAL`, `FLEET_TERMINAL_ID`, `FLEET_STATUS_PATH`) in `HarnessEnv::apply`. The
  scripted `claude`/`codex` shims branch on `FLEET_DELEGATION` to tell caller from child, so a
  harness run launched from inside a delegation made every scripted *caller* play
  `subagent-child-blocked.json`; its permission gate went unanswered and the turn died. A/B'd at
  `d51bbae` and at HEAD, inherited and clean: identical both ways, so it was never a regression
  and never the batch's doing. See `/tmp/fleet-briefs/report-AB.md`. What is left:
  1. **`$TMPDIR` must be short.** The macOS per-user temp directory pushes the harness run
     directory's socket past the 104-byte `sockaddr_un` limit: `filesystem operation failed for
     /var/folders/…/home/fleetd.sock: path must be shorter than SUN_LEN`. It reads as "fleetd did
     not become ready within 20s", which sends you looking in the wrong place. Worth fixing in
     the harness — a short fixed run-directory root, or an explicit failure naming `$TMPDIR`.
     `docs/TESTING-HARNESS.md` is **frozen**; read it before changing a message.
  2. **`$TMPDIR` must also be canonical — use `/private/tmp/ht`, not `/tmp/ht`.** On macOS
     `/tmp` is a symlink to `private/tmp`, and `platform_root_alias`
     (`crates/fleet-daemon/src/adapters/files.rs:863`) rewrites `/var` to `/private/var` but has
     **no `/tmp` entry**. Under `TMPDIR=/tmp/ht` the lexical path stays `/tmp/…` while
     `resolve_root`'s `canonicalize` returns `/private/tmp/…`, the removable-root containment
     check disagrees with itself, and three `adapters::files::tests::conditional_remove_*` tests
     fail with `Not a directory (os error 20)`. Verified 2026-09-19: they fail under `/tmp/ht`
     and under a fresh `/tmp/ht2`, and pass under both the default `$TMPDIR` and
     `/private/tmp/ht` — the same directory as `/tmp/ht`, spelled canonically. `files.rs` is
     byte-identical at `d51bbae` and at HEAD, so this is pre-existing and unrelated to this
     branch. The real fix is a `/tmp` arm in `platform_root_alias` beside the `/var` one.
  3. The headless subset still overruns its budget. Measured 2026-09-19 on the merge:
     `headless subset: 21 scenario(s) in 95.1s, 39 skipped`, and the suite warns "the headless
     subset now costs make test 95s, over its 60s budget; move scenarios back to
     `make harness-headless`". The earlier ~500s figure predates `e4d6f66` and was inflated by
     scenarios timing out on unanswered permission gates; 95s is the real cost. It is a warning,
     not a failure, so `make test` is green regardless.

## Verify after the first batch lands

The shared `fleetd` on this machine **does not contain this batch**: every task in it was
forbidden from restarting the daemon, because that daemon was hosting the session that executed
the batch. It is a release build from `~/.swarm/repos/dannyfuf/fleetd`, on a commit newer than
this branch in some respects and older in all of these. Nothing below can be observed until it
is replaced. In order:

- [x] **`main` is merged in — `make restart` from this worktree is now safe.** `06a56d2`
  (`build: merge origin/main into fix/native-subagents`) merges `origin/main` at `6257fad`,
  which carries agent-database migration slot 5 (`closed_threads`, `a774140`). That slot was the
  blocker: the running release `fleetd` had already applied it to `~/.fleet/agents/state.sqlite`,
  and a build from this branch knew slots 1-4 only, so it refused to open that database — "the
  agent database records migration slot 5 (closed_threads), which this build does not know: it
  was written by a newer fleetd. Refusing to open it" — and every native-agent verb failed. The
  merged tree registers slot 5 (`crates/fleet-daemon/src/services/agents/store/migrations.rs`),
  so a `fleetd` built here opens it. On the merge `make lint` is green and `make test` is green
  with `TMPDIR=/private/tmp/ht` (see the `$TMPDIR` item above). Rebuild and restart; no rebase is
  needed, and this repo integrates with merge commits anyway.
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
