# Session TODO — native subagent feedback, deferred items

Source: an orchestrator agent's field report after driving `fleet subagent` end to end
(2026-09-19). Items 1-5 are planned in `plans/subagent-feedback-fixes-2026-09-19-plan.md` and
have **shipped** on `fix/native-subagents`, along with two mid-flight additions (T06, T07). The
four items deferred out of that batch are planned in
`plans/subagent-feedback-deferred-2026-09-19-plan.md` and have now **shipped** too; the note
below records what landed and what the work turned up.

## Shipped 2026-09-19 (deferred batch)

All four deferred items landed on `fix/native-subagents` in seven commits — `de5921a` (shared
types and the `--env` wire field), `6e3b1d8` (consume on a caller's own `wait`), `176c708` (the
child's usage on every delegation read), `bd4eb88` (migration slot 6, the persisted child
environment and its restore on resume), `810554d` (the whole `fleet subagent` CLI reshape),
`95c7b16` (the docs) and `d88b473`/`45316fb`/`121d423` (T07's flake fix and doc reconciliation).
Tracker: `plans/subagent-feedback-deferred-2026-09-19-tracker.md`.

- **Cost visibility** — `fleet subagent status` and `fleet subagent list` now show the child's
  tokens, cost and context, and `--json` carries `delegation.usage`. `fleet agent list` is
  still unchanged and still needs a `threads`-table migration; that stays deferred, as do
  delegation-*tree* rollups.
- **Repeatable `--env KEY=VALUE`** — plumbed CLI → client → proto → daemon, merged **under**
  Fleet's identity variables by the daemon itself, refused for a value with no `=`, an empty or
  repeated key, any `FLEET_*` name and `PATH`, and kept across a resume.
- **Result delivery for orchestrators that never end their turn** — a `wait` naming its own
  caller now consumes the delivery, so the result is not injected a second time when the turn
  settles; `fleet subagent status` prints the report body through the same template `wait` uses.
- **`run --json` echoing the whole brief** — `run`, `wait` and `list` now cut a brief over 200
  characters and set `briefElided: true`; `status` and `cancel` keep it whole.

Four things the code contradicted the decisions on, worth keeping for whoever restarts the
shared daemon:

1. **`DelegationWait` had no caller.** Consume-on-wait needed an additive wire field
   (`caller: Option<ThreadId>`), not just a store value — the daemon could not otherwise tell a
   caller's own `wait` from a third party's.
2. **`manager.projection()` is not a cheap read.** It hydrates a cold thread and replays its
   whole event log, so `list` could not use it; the usage numbers are computed from SQL over
   `turns.usage_json` plus the newest `token_usage` event, with an equivalence test pinning them
   to `ThreadProjection` field for field.
3. **The resume path dropped `extra_env` entirely**, so persistence was required after all:
   migration **slot 6** (`delegation_env`, `ALTER TABLE delegations ADD COLUMN env_json TEXT`).
4. **The report was never elided on the wire** — `status` simply did not print it. No `--full`
   flag and no new wire field were needed; `status` got its own renderer instead.

## Found during the batch, not fixed by it

- [x] **`retry_tick_sends_and_counts_the_nudge` was flaky under the whole-crate daemon run —
  fixed in `d88b473`.** `crates/fleet-daemon/src/services/agents/delegation/tests/worker.rs`
  busy-waited 200 `yield_now()` iterations for the worker's startup pass to close its `Recover`
  outbox row and then asserted the outbox was empty; under `cargo test -p fleet-daemon` the
  worker did not always get there inside 200 yields, so it failed there and passed in isolation.
  A/B-proved at `810554d` and at `bd4eb88` before the fix. It now waits on the condition through
  a `Harness::wait_for_empty_outbox` helper shaped exactly like the existing
  `wait_for_delegation`: subscribe to the event bus first, then read the store, so a row closed
  between the read and the wait still wakes the loop. No tolerance was widened and no sleep was
  added; the whole-crate run is 817/817, three times in a row.

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

## Verify after both batches land

The shared `fleetd` on this machine **contains neither batch**: every task in both was forbidden
from restarting the daemon, because that daemon was hosting the sessions that executed them. It
is a release build from `~/.swarm/repos/dannyfuf/fleetd`, on a commit newer than this branch in
some respects and older in all of these. Nothing below can be observed until it is replaced.

**`make restart` from this worktree applies agent-database migration slot 6 (`delegation_env`)
to `~/.fleet/agents/state.sqlite`, and that is one way.** The slot is additive and harmless in
itself, but the ledger row it writes is not removable: a `fleetd` built without the slot — the
current release binary, or this branch reverted — then **refuses to open that database** and
every native-agent verb fails, exactly as slot 5 did before `06a56d2`. A `consumed` delivery
value in a row has the same property: `RawDelegation::decode` rejects an unknown delivery word.
Copy `~/.fleet/agents/state.sqlite` before the first restart; restoring that copy is the only
way back. In order:

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
- [ ] **Copy `~/.fleet/agents/state.sqlite`**, then **`make restart`.** This is the step. Until
  it runs, `fleet subagent run` goes on producing children with no `fleet` on their `PATH`, the
  old same-worktree warning and the old footer, and no `--env`, no usage on a delegation read and
  no consume-on-wait — every one of those behaviours lives in the daemon, however new the `fleet`
  binary in your shell is. The restart is what applies slot 6; see the warning above.
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

- [ ] **Re-run the four-children orchestrator with `--env CARGO_TARGET_DIR=…` per child** and
  confirm the build-lock contention the flag exists for is actually gone, that
  `fleet subagent list` shows a token total and a cost per child, that `fleet subagent status`
  prints the report body without anyone reading a file out of `/tmp`, and that waiting on every
  child leaves no duplicate result messages when the orchestrator's turn settles.
- [ ] **Kill a delegated child's provider mid-run and let the daemon resume it**, then confirm
  the resumed process still has the `--env` variables it was started with (slot 6's whole
  point) alongside a freshly rotated `FLEET_DELEGATION_TOKEN`. Covered by a unit test
  (`manager::tests::lifecycle::a_resumed_delegated_child_keeps_its_environment_and_gets_a_fresh_token`)
  and by nothing end to end.

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

**Already verified against a private daemon (2026-09-19, deferred batch T07).** Same recipe, a
second time: `target-p1/debug/fleetd` under `FLEET_HOME=/tmp/fleet-d07-home`, a copied
`config.json` with `hosts` cleared, a hand-seeded `state.json` naming only the `personal`
context, `dannyfuf/fleetd` and this worktree, and **both** `FLEET_HOME` and `FLEET_DAEMON`
overridden in every shell — a delegated child inherits the shared daemon's `FLEET_DAEMON`, so
`FLEET_HOME` alone still is not enough. The private daemon applied slots 1-6 to its own empty
database; `~/.fleet/agents/state.sqlite` was never opened. With a Claude caller in a running
turn, two children were delegated. All of it held: `--env D07_CHECK=d07 --env
CARGO_TARGET_DIR=/tmp/never-used` reached the child verbatim while its `FLEET_DELEGATION` was
the daemon's own id; `--env FLEET_DELEGATION=x`, `--env PATH=/x` and a duplicated key were all
refused at the CLI with the documented messages; `run --json` on a 650-character brief carried a
200-character preview and `briefElided: true` while `status --json` carried all 650; `list`
printed eight fields with `150013` and `$0.53`; `status` printed the fixed line, the brief, the
usage block and the report body, and that body was byte-identical to what `wait` printed;
`wait --caller <caller>` left the record `delivery: consumed`, and after the caller's turn ended
its transcript held **no** delegation-origin message for that child — while the second child,
never waited on, was injected exactly once, which is the unchanged default. What is still left
above is the four-concurrent-children load case, the provider-kill recovery case, and everything
that needs the *shared* daemon.
