# Board workflows, phase 4: what a run changed — Plan
> Tracker: ./board-workflows-2026-09-20-phase-4-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you
  commit.

## Summary

A run's report gains the one fact a reviewer needs: what the tree looks like now compared with when
the run started. `Checkpoints::changed_since(worktree, thread)` diffs the thread's **first**
checkpoint tree against a fresh snapshot of the worktree, using the two primitives the revert path
already pairs — `Plumbing::snapshot_tree` and `Plumbing::diff_trees`. The diff is taken **at
delivery**, inside `on_run_delivered`, for the ending run's own thread. Its count fills
`CardRun.files_changed` and its paths fill the delegation's `DelegationResult.files_changed`, which
a reported completion leaves empty today. Its list is rendered as `## Files changed since this run
started` and appended to the report comment the card keeps, so it reaches the next run of the same
card as part of `## Previous run reports` — the review run reads what the implementation run
touched. When `max_live_runs > 1` a second sentence says the worktree is shared. A worktree that is
not a git repository, and a thread with no checkpoint, answer an empty list and the run still
succeeds.

## Sizing call

**Phased, phase 4 of 9.** See ./board-workflows-2026-09-20-roadmap.md. The smallest phase and the
only one that can be cut without cutting the feature: one new async method over existing git
plumbing, one capture point, one rendered section, one test file and two doc sentences. It is
scheduled after phase 3 because it needs `on_run_delivered` and the run's `thread_id` to know which
thread's first checkpoint to diff.

## Repository context

- `services/checkpoints.rs:70` `Checkpoints`, `new(shell)` `:88`, `capture_turn` `:107` (a capture
  failure never refuses a turn, `:98-102`), `list(worktree, thread)` `:163` — oldest first by
  `ordinal`, empty for a non-worktree `:164-172`; `next_ordinal` `:256`. The thread's first
  checkpoint is `list(..)[0]`, `ordinal == 1`.
- Git primitives, all private `Plumbing` over `adapters::shell::Shell` in `checkpoints/git.rs`:
  `is_worktree` `:90`, `snapshot_tree` `:105`, `resolve_tree` `:207`, `read_commit` `:230`,
  `diff_trees` `:273`, `parse_name_status` `:430`, `ScratchIndex` `:457-480`, `Change`/`Difference`
  `:43`/`:54`, `GIT_TIMEOUT` `:36`. `revert.rs:48` is the existing `snapshot_tree` + `diff_trees`
  pairing to copy. **No `changed_since` exists.**
- `DelegationResult.files_changed: Vec<String>` `fleet-core/src/agents/delegation.rs:163`; a
  reported completion leaves it empty (`delegation/complete.rs:232`); the fallback path fills it
  from `DelegationFacts` (`transition.rs:235`). Persisted as `result_files`
  (`store/delegations.rs:483`, decode `:916-925`, `update` `:387`).
- `footer.rs:59-84` `delivered_message` prints `files changed: {n}` for thread callers — unchanged
  here.
- Phase 3's seams: `boards/automation.rs::on_run_delivered` (the capture point),
  `Automation.checkpoints` (stored in phase 3, used here), the board's worktree resolved by
  `worktree_context` (`boards/lifecycle.rs:696-715`; confirm the path field with `grep -n "pub path"
  crates/fleet-core/src/model.rs`), `BoardSettings::max_live_runs()`.
- Tests: `services/checkpoints/tests.rs` — `IsolatedShell` `:31-65`, `Fixture` `:68` with `new()`
  `:76`, `write` `:103`, `read` `:111`, `git` `:119`, `tree_now` `:150`, `checkpoint_refs` `:199`,
  `#[tokio::test]`s from `:208`.
- Skills: `rust-async-background-work` (the git calls are `spawn`-free async over the shell
  adapter), `rust-workspace-architecture` (the new public type, the error arm), `rust-gpui-testing`
  (the fixture), then `zed-quality-review`.
- Docs in step: `docs/BOARD.md` §11 (the files section and the shared-worktree sentence),
  `docs/NATIVE-AGENTS.md` §5 (one sentence that a card run's changed-file list comes from the
  thread's first checkpoint).

## Assumptions

- The diff is tree-to-tree, so a human editing the worktree during the run appears in it. That is
  why the heading says "since this run started" and not "this card changed".
- The public `ChangeKind` mirrors the private `git::Change` (`checkpoints/git.rs:43`) and is
  converted at the boundary; the private type stays private (contracts §3.4).
- Phase 3's `on_run_delivered` already runs after the delegation is terminal and before the
  `Deliver` row is closed, so a diff taken there is the tree the run left.
- The report excerpt cap (`REPORT_EXCERPT_CAP_BYTES`) applies to the reported text; the files
  section is appended **after** capping, so it is never the part that is elided.
- A checkpoint capture that never happened (a provider that ran no turn) is normal, not an error.

## Out of scope

- Any second diff at brief-assembly time: `brief()` (contracts §2) takes no files argument and the
  section travels in the previous report (contracts §3.4).
- Per-file attribution between concurrent runs. With `max_live_runs > 1` the sentence says the list
  may include another run's work; nothing tries to split it.
- Committing, stashing or touching `HEAD` — `docs/NATIVE-AGENTS.md` §5 keeps them untouched.
- `TurnDiff`, which carries no paths on either provider.

## Affected areas

- `crates/fleet-daemon/src/services/checkpoints.rs`, `checkpoints/git.rs` (read only),
  `checkpoints/error.rs`, `checkpoints/tests.rs`.
- `crates/fleet-daemon/src/services/boards/automation.rs`; `services/agents/delegation/{mod.rs,
  queries.rs}` for the one new seam.
- `docs/BOARD.md`, `docs/NATIVE-AGENTS.md`.

## Tasks

### P4-T01 — `changed_since` over `snapshot_tree` and `diff_trees`
- **Intent:** One method that answers what changed, and answers an empty list rather than failing.
- **Touches:** `services/checkpoints.rs`, `checkpoints/error.rs`, `checkpoints/git.rs`.
- **Steps:**
  - Add `pub struct ChangedFile { pub path: String, pub kind: ChangeKind }` and `pub enum ChangeKind
    { Modified, Added, Deleted }` to `checkpoints.rs`, with a `From<git::Change>` conversion beside
    them (contracts §3.4).
  - `pub async fn changed_since(&self, worktree: &Path, thread: &ThreadId) ->
    Result<Vec<ChangedFile>, CheckpointError>`: return `Ok(vec![])` when `Plumbing::is_worktree` is
    false or when `list(worktree, thread)` is empty; otherwise take the first checkpoint (`ordinal
    == 1`), resolve its tree (`read_commit` + `resolve_tree`), take a fresh `snapshot_tree` of the
    worktree, and `diff_trees(first_tree, snapshot)`.
  - Map each `Difference` through `parse_name_status`'s result into a `ChangedFile`, relative paths,
    sorted by path so two runs of the same test read the same.
  - Honour `GIT_TIMEOUT` and the existing `CheckpointError` arms; add no new error variant unless
    the diff itself needs one.
  - Doc comment: the diff is tree-to-tree, so a user's own edits during the run are included, and it
    is taken when the run ends, not while the brief is assembled.
- **Verification:** `cargo test -p fleet-daemon checkpoints`, `make lint`.
- **Done when:** The method exists with the contracts §3.4 signature and neither empty case returns
  an `Err`.

### P4-T02 — Capture it at delivery and write both counts
- **Intent:** The run row shows a real number, and a reported child stops reading `files changed:
  0`.
- **Touches:** `services/boards/automation.rs`, `services/agents/delegation/{mod.rs, queries.rs}`.
- **Steps:**
  - In `on_run_delivered`, before the card write: resolve the board's worktree path through
    `worktree_context`, call `automation.checkpoints.changed_since(path, &run.thread_id)`, and
    log-and-continue on `Err` — a diff failure never blocks a delivery.
  - Write `CardRun.files_changed = list.len() as u32` in the same gated write that records
    `ended_at` and `outcome`.
  - Add `pub(crate) async fn set_result_files(&self, id: &DelegationId, paths: Vec<String>) ->
    DaemonResult<()>` on `DelegationService`, writing `result_files` through
    `store::delegations::update` (`:387`), and call it for card runs with the changed paths
    (contracts §3.3).
  - Leave the thread-caller path alone: `delivered_message` (`footer.rs:59-84`) keeps reading
    whatever the record holds.
- **Verification:** `cargo test -p fleet-daemon services::boards`, `cargo test -p fleet-daemon
  delegation`, `make lint`.
- **Done when:** A card run that reports through `fleet subagent complete` ends with a non-zero
  `files_changed` when it edited files, and zero when it did not.

### P4-T03 — The report section and the shared-worktree sentence
- **Intent:** Put the list where the next run will read it.
- **Touches:** `services/boards/automation.rs`.
- **Steps:**
  - Render, from a non-empty list only: a blank line, `## Files changed since this run started`,
    then one line per file `{M|A|D} {path}` in the list's order.
  - When `board.settings.max_live_runs() > 1`, append the sentence `Other runs share this worktree;
    some of these changes may be theirs.` after the list.
  - Append the rendered section to the report comment's body **after** the excerpt cap has been
    applied, so the cap elides the agent's prose and never the file list.
  - No section is written when the list is empty, and none is assembled anywhere in
    `start_for_card`: the next run receives it through `## Previous run reports`, which `brief()`
    already carries.
- **Verification:** `cargo test -p fleet-daemon services::boards`, `make lint`.
- **Done when:** A run that changed nothing produces a comment with no heading, and a second run of
  the same card quotes the first run's file list in its brief.

### P4-T04 — Tests and the two documents
- **Intent:** Prove the diff in a real temporary git worktree, and say so in the authoritative
  documents.
- **Touches:** `services/checkpoints/tests.rs`, `docs/BOARD.md`, `docs/NATIVE-AGENTS.md`.
- **Steps:**
  - With the existing `Fixture` (`tests.rs:68`):
    `changed_since_reports_modified_added_and_deleted_paths` — capture a turn, `write` a change to
    one tracked file, `write` a new file, delete a third, then assert `M`, `A`, `D` and a count of
    three.
  - `changed_since_on_a_directory_that_is_not_a_git_worktree_is_empty` and
    `changed_since_without_a_checkpoint_is_empty`, both `Ok(vec![])`.
  - `changed_since_uses_the_first_checkpoint_not_the_latest`: two captures, then assert the list
    spans both turns.
  - `docs/BOARD.md` §11 gains the files-section paragraph: when the diff is taken, what the heading
    means, where the section lives, and the shared-worktree sentence. `docs/NATIVE-AGENTS.md` §5
    gains one sentence: a card run's changed-file list is the diff between its thread's first
    checkpoint and a snapshot taken when the run ends.
- **Verification:** `cargo test -p fleet-daemon checkpoints`, `make test`, `make lint`.
- **Done when:** Four tests pass and both documents match the code.

## Verification

```sh
make lint
cargo test -p fleet-daemon checkpoints
cargo test -p fleet-daemon services::boards
cargo test -p fleet-daemon --test boards_automation
make test && make restart
```

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean and `make test` passes.
- [ ] A non-git worktree and a checkpoint-less thread both answer an empty list, and the run still
  succeeds.
- [ ] `CardRun.files_changed` and `DelegationResult.files_changed` agree with the list.
- [ ] `docs/BOARD.md` §11 and `docs/NATIVE-AGENTS.md` §5 match the code.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **A slow diff delays a delivery.** The call sits on the delivery path; `GIT_TIMEOUT` bounds it and
  an `Err` is logged and skipped, never retried in a loop.
- **A shared worktree makes the list wrong-looking.** The sentence is the whole mitigation in v1; a
  worktree per run is the real fix and is not in this design.
- **Writing `result_files` after the record is terminal** must not resurrect a delegation; the seam
  only updates that one column and publishes nothing new.
- **Rollback.** Revert the phase PR. `files_changed` returns to `0` and no report comment carries a
  files section; every other behaviour of phase 3 is untouched.

## Contract resolutions

Resolved in the contracts file on 2026-09-20; the contracts wording wins over any earlier phrasing above.

1. The diff is taken at delivery inside `on_run_delivered`; the section goes into that run's report comment and reaches later briefs through the previous-reports part (contracts §3.4).
2. Paths go to `DelegationResult.files_changed` (`Vec<String>`) through `DelegationService::set_result_files`; the count goes to `CardRun.files_changed` (contracts §3.3, §3.4).
3. `set_result_files(&DelegationId, Vec<String>)` is a delegation-service verb owned by this phase (contracts §3.3).
4. The public kind is `ChangeKind { Modified, Added, Deleted }` on `ChangedFile { path, kind }`; the private `git.rs` `Change` is converted at the boundary (contracts §3.4).
