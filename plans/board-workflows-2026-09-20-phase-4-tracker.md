# Board workflows, phase 4: what a run changed — Tracker
> Plan: ./board-workflows-2026-09-20-phase-4-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [x] P4-T01 — `changed_since` over `snapshot_tree` and `diff_trees`
  - verified: `cargo test -p fleet-daemon --lib services::checkpoints` — 34 passed, including the
    four new `changed_since_*` tests against a real `git` in a temporary worktree; `cargo check -p
    fleet-daemon --all-targets` and `cargo clippy -p fleet-daemon --all-targets --all-features --
    -D warnings` clean.
- [x] P4-T02 — Capture it at delivery and write both counts
  - verified: `cargo test -p fleet-daemon --lib services::boards` and `cargo test -p fleet-daemon
    --lib services::agents::delegation` pass; `on_run_delivered` takes the diff before the gate,
    writes `CardRun.files_changed` in the gated write and hands the paths to
    `DelegationService::set_result_files`, which now rewrites `result_files` through
    `delegations::update` instead of answering `Ok(())`.
- [x] P4-T03 — The report section and the shared-worktree sentence
  - verified: four unit tests in `services/boards/automation.rs` — no heading on an empty list,
    `{M|A|D} {path}` in the order `changed_since` returned, the shared-worktree sentence only
    above one live run, and the section surviving a report that is over the excerpt cap.
- [x] P4-T04 — Tests and the two documents
  - Tests half done: the four `changed_since_*` tests in
    `services/checkpoints/tests.rs` (modified/added/deleted, a directory that is not a worktree, a
    thread with no checkpoint, and first-checkpoint-not-latest), plus the `docs/NATIVE-AGENTS.md`
    §5 sentence. verified: `cargo test -p fleet-daemon --lib services::checkpoints` — 34 passed.
  - Docs half done: the `docs/BOARD.md` §11.2 files-section paragraph landed with P4-T03 — when
    the diff is taken, what the heading means, why an empty list gets no heading, where the
    section sits relative to the excerpt cap, the shared-worktree sentence, and how it reaches the
    next run. `docs/NATIVE-AGENTS.md` §5's cross-reference to it is now honest.

## Notes / decisions log
- 2026-09-21 (contracts:daemon) — the P4 shapes exist as a compiling skeleton: `ChangeKind`,
  `ChangedFile` and `Checkpoints::changed_since` in `services/checkpoints.rs`
  (`// CONTRACT STUB (d:changed-since)`, answering `Ok(vec![])`), and
  `DelegationService::set_result_files` (`// CONTRACT STUB (d:files-at-delivery)`, answering
  `Ok(())`). `Automation` already stores `Arc<Checkpoints>`, so T02 has its handle at delivery.
  `changed_since` takes `&ThreadId` per contracts §3.4, unlike the `ThreadId` the sibling
  `Checkpoints::list` takes; it returns the module's `Result<Vec<ChangedFile>>` alias, which is
  the same type the contract spells out in full.
- 2026-09-21 (d:changed-since) — `changed_since` replaced its stub. Three decisions worth
  recording. (1) The two empty answers both come out of `list`, which already answers an empty
  vector for a directory that is not a working tree, so there is no second `is_worktree` call on
  the delivery path. (2) A ref the sweep collected between the listing and the `resolve_tree` is
  also `Ok(vec![])`: the thread's history is gone, which is the same answer as never having had
  one. (3) A *file*-scoped first checkpoint narrows the diff to its own recorded paths
  (`read_commit` + `Metadata::decode`). Its tree holds only those paths, so an unnarrowed diff
  would report every other file in the worktree as added — the plan's `read_commit` step is what
  this is for. A turn-scoped first checkpoint, which is the normal case because `capture_turn`
  runs before a turn starts, diffs the whole tree. The list is sorted by path so two runs of the
  same work read the same.
- 2026-09-21 (d:changed-since) — the stub had left `list`'s own doc comment attached to
  `changed_since`, so `list` was undocumented and `changed_since` claimed to list checkpoints.
  `changed_since` now sits below `list` with its own doc and `list` has its paragraph back.
- 2026-09-21 (d:files-at-delivery) — what phase 3 had already built was left alone: the whole of
  `on_run_delivered`, `report_excerpt`, the comment rotation and `Automation.checkpoints` existed
  and were correct, so this task added two helpers and changed three lines inside the hook rather
  than rewriting it. Four decisions worth recording. (1) The diff is taken **before** the gate,
  beside the usage read: it shells out to Git, and under the gate it would hold every other card
  on the board for as long as Git took. (2) `set_result_files` writes **only** a non-empty list.
  The diff answers an empty list both when a run changed nothing and when it could not be taken
  at all, and a blind write would erase the list a recovered completion had already filled in
  (`transition.rs` fills `files_changed` from `DelegationFacts`). It also skips a row with no
  `result`: a child that never reported has nothing for the paths to hang from, and inventing an
  empty `DelegationResult` would make it look as though it had. (3) Every way of not knowing —
  no worktree, a worktree this daemon no longer holds, an unreadable state file, a failed Git
  command — logs and answers an empty list. A delivery that cannot describe a run is still a
  delivery. (4) No section is written when there is no report comment to append it to, which is
  contracts §3.4 read literally: a cancelled run that reported nothing keeps its count on the card
  and nothing else.
- 2026-09-21 (d:files-at-delivery) — `set_result_files` stayed in `delegation/mod.rs`, where the
  contracts stage put its stub, rather than moving to `queries.rs` as the phase plan's *Touches*
  line reads. Moving it would have edited a file this task does not own to gain nothing; the
  contract fixes the signature on `DelegationService`, not the file it is spelled in.

## Follow-ups
