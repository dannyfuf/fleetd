# Quality pass — handoff

Temporary. Deleted with the rest of `.claude/workflows/` scaffolding when the pass lands.

**Read this first if you are resuming this work.** Branch `chore/cleaning`, PR
https://github.com/dannyfuf/fleetd/pull/25.

## What this pass is

A quality pass over the workspace against the Zed-derived project skills in `.claude/skills/`.
Correctness, reliability, performance and the right abstractions — no new features, no visual
redesign. Every behavioural fix lands with a regression test that fails before it.

## How the work is organised

1. **Audit (done).** `quality-pass.js` fanned 13 area auditors over the workspace, each loading
   the project skill that governs its area, then adversarially verified every finding.
   **85 findings survived** (73 CONFIRMED, 12 PLAUSIBLE): 22 high, 43 medium, 20 low.
   The full list is in `FINDINGS.md`. The machine-readable copy with evidence, verifier notes,
   corrected fixes, blast radius and test plans is at
   `.claude/workflows/state/findings.json`, split per batch into `.claude/workflows/state/batches/<batch>.json`.
2. **Fix**, in three waves of file-disjoint batches so agents never edit the same file:
   - `w1-*` — daemon core (fleet-daemon, fleet-core, fleet-client, fleet-term)
   - `w2-*` — fleet-git, fleet-lazygit, fleet-app
   - `w3-*` — fleet-ui-kit, palette
   Batch → owned paths is the `OWNERSHIP` map in `quality-fix.js`. Briefs are pre-written at
   `.claude/workflows/state/briefs/<batch>.txt`.

`<scratchpad>` is
`/tmp/claude-1000/-home-df--fleet-worktrees-dannyfuf-fleetd-chore-cleaning/3a50ef35-e725-4f61-bd9d-09b923302fce/scratchpad`.
**If that directory is gone**, regenerate the findings from the audit result kept at
`<same session dir>/tasks/wjc1xgqej.output` (JSON: `.result.kept`), or just re-run the audit —
`quality-pass.js` is idempotent.

## Where the work stood at handoff (2026-09-10 03:40 -03)

- `cargo check --workspace --all-targets` — **passes**.
- `cargo fmt --check` — passed at baseline; not re-run since the edits.
- Full `make test` — **794 passed, 1 failed**, then the run aborted (cargo stops at the first
  failing suite), so every crate after `fleet-client` is unverified. **Re-run it.**
  The failure is `reconnects_and_restores_event_subscription`
  (`crates/fleet-client/tests/client.rs`, `-p fleet-client --test client`): expected fallout of
  the `ipc-subscribe-extend-vs-replace` fix, which changed `Subscribe` from *extend* to
  *replace*. The `w1-server` Codex job owns both sides of that seam
  (`crates/fleet-client/**` and `crates/fleet-daemon/src/server/**`) and should have resolved it;
  confirm it did, and that the test now pins the *replace* semantics deliberately rather than
  having been loosened to pass.
- Working tree: ~49 files changed, +2397/-202, all uncommitted at the time of the WIP checkpoint
  commit that accompanies this file.

### Wave 1

| Batch | State |
| --- | --- |
| `w1-worktrees` | **complete** — 4 findings fixed, reported and self-verified |
| `w1-agents` | partial work in tree; handed to Codex `task-mtv5q7nl-9wizr5` |
| `w1-server` | partial work in tree; handed to Codex `task-mtv5q7xz-ov15go` |
| `w1-machines` | partial work in tree; handed to Codex `task-mtv5q883-uw3sb5` |
| `w1-router-term` | partial work in tree; handed to Codex `task-mtv5q8in-2k1dus` |
| `w1-jobs` | partial work in tree; handed to Codex `task-mtv5q8vy-wx7261` |

The five Codex jobs were started at 03:39 -03 with `--model gpt-5.6-sol --effort high --write
--background`. They are separate processes and keep running independently of the Claude session.
Their brief tells them the tree already holds partial unverified work, to audit it finding by
finding, finish it, and to watch each regression test fail before its fix.

Waves 2 and 3 had **not** been started.

## Resume checklist

```sh
export CLAUDE_PLUGIN_ROOT=~/.claude/plugins/cache/openai-codex/codex/1.0.6
node "$CLAUDE_PLUGIN_ROOT/scripts/codex-companion.mjs" status --all
node "$CLAUDE_PLUGIN_ROOT/scripts/codex-companion.mjs" result <job-id>   # per job above
```

1. Collect the five Codex reports. Check each claim against `git diff` — the brief warns them
   their report will be checked, so check it.
2. `make lint` and `make test`. Fix the fallout.
3. Commit wave 1 per area, in the repo's `<area>: <imperative lowercase summary>` form.
4. Run wave 2: one Codex job per batch in `WAVES.w2`, brief at `.claude/workflows/state/briefs/<batch>.txt`.
   Same for wave 3. Dispatch a whole wave at once; the batches within a wave are file-disjoint.
5. Review the whole diff against `.claude/skills/zed-quality-review`.
6. Delete `.claude/workflows/` scaffolding (`quality-pass.js`, `quality-fix.js`, `FINDINGS.md`,
   this file) in the final commit, and tick the PR checklist.

## Standing instructions from the user

- Defer execution work to Codex (`--model gpt-5.6-sol --effort high`); keep the decisions, the
  scoping and the review. Verify what Codex returns rather than relaying it.
- Maximise parallelism; use workflows for orchestration.
- Every bug fix gets a regression test. Think of the tests as the thing that prevents regressions —
  map the cases and cover them.
- No new features. Code quality and bug fixing over usability. UI quick wins are optional.
