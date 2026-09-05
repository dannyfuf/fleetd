# fleet-git

The Git backend for Fleet's native lazygit clone. It shells out to `git` with
explicit argv (never through a shell, never through `git2`/`gix`), preserves
Git's exact bytes, and hands the UI owned, typed snapshots.

See [API.md](API.md) for the complete list of public types and signatures.

## Design rules

- **One process per operation, explicit argv.** `tokio::process::Command` only.
  Every child gets `GIT_TERMINAL_PROMPT=0` and `LC_ALL=C`; path operations add
  `GIT_LITERAL_PATHSPECS=1` and background reads add `GIT_OPTIONAL_LOCKS=0`.
- **Bytes in, bytes out.** Paths are `PathBuf` built from raw bytes and diff
  line content is `Vec<u8>`, so non-UTF-8 paths and content survive intact.
  Lossy decoding happens only where a type says `String`.
- **Mutations are serialized.** Every mutating call takes one per-repository
  async mutex, so two UI actions can never interleave `git` invocations.
- **Failures are typed.** `GitError::Conflict` is reported only for commands
  that can stop on conflicts; everything else that exits non-zero is
  `GitError::Exit`, carrying the full stdout/stderr and a redacted argv.

## Discover and snapshot

```rust
use fleet_git::{Repository, SnapshotOptions, Head, OperationState};

let repository = Repository::discover("/path/to/worktree").await?;
let paths = repository.paths();          // worktree_root, git_dir, common_dir

let snapshot = repository.snapshot(SnapshotOptions::default()).await?;
match &snapshot.head {
    Head::Branch { name, description, .. } => println!("on {name}: {description}"),
    Head::Detached { oid, .. } => println!("detached at {oid}"),
    Head::Unborn { name } => println!("unborn {name}"),
}
if let OperationState::Rebasing { done, total, .. } = &snapshot.operation {
    println!("rebasing {done:?}/{total:?}");
}
for file in &snapshot.files {
    println!("{:?} {:?} {}", file.index, file.worktree, file.path.display());
}
for commit in &snapshot.commits {
    let marker = if commit.pushed { ' ' } else { '*' };   // '*' = not on upstream
    println!("{marker} {} {}", commit.oid, commit.subject);
}
```

`snapshot` runs its independent reads concurrently and bumps
`snapshot.generation`, so the UI can discard a stale refresh by comparing
generations. Narrow the work with `SnapshotOptions { commit_limit,
reflog_limit, include_tags, include_remotes }`.

## Diffs

```rust
use fleet_git::{DiffSide, ObjectId, Ref};
use std::path::Path;

// Working tree vs index; an untracked path renders as an all-added diff.
let diff = repository.diff_file(Path::new("src/main.rs"), DiffSide::Unstaged).await?;

// Other diff sources.
let staged  = repository.diff_file(Path::new("src/main.rs"), DiffSide::Staged).await?;
let commit  = repository.diff_commit(&ObjectId::from("HEAD"), &[]).await?;
let stashed = repository.diff_stash(0).await?;
let range   = repository.diff_range(&Ref::from("main"), &Ref::from("topic")).await?;
let branch  = repository.diff_branch(&Ref::from("topic")).await?;  // vs merge-base with HEAD
let summary = repository.commit_files(&ObjectId::from("HEAD")).await?;

for file in &diff.files {
    for hunk in &file.hunks {
        println!("@@ -{},{} +{},{} @@", hunk.old.start, hunk.old.count,
                                        hunk.new.start, hunk.new.count);
        for line in &hunk.lines {
            println!("{:?} {}", line.kind, String::from_utf8_lossy(&line.content));
        }
    }
}
println!("{}", diff.to_text_lossy());   // renders back to a unified patch
```

Hunk bodies are framed by the counts in their `@@` header, so a patch whose
content itself contains `diff --git` or `@@` lines still parses correctly.

## Stage individual lines

`HunkSelection::lines` holds indexes into `Hunk::lines`; `None` selects the
whole hunk. The crate rebuilds a valid patch (unselected pre-image lines become
context, unselected post-image lines are omitted, every hunk range is
recomputed) and pipes it to `git apply` on stdin.

```rust
use fleet_git::{DiffSide, HunkSelection, LineKind, PatchAction, PatchSelection};
use std::path::{Path, PathBuf};

let diff = repository.diff_file(Path::new("notes.txt"), DiffSide::Unstaged).await?;
let selected: Vec<usize> = diff.files[0].hunks[0]
    .lines
    .iter()
    .enumerate()
    .filter(|(_, line)| matches!(line.kind, LineKind::Added | LineKind::Removed))
    .map(|(index, _)| index)
    .take(2)
    .collect();

repository
    .apply_patch_selection(
        PatchSelection {
            path: PathBuf::from("notes.txt"),
            side: DiffSide::Unstaged,
            hunks: vec![HunkSelection { hunk_index: 0, lines: Some(selected) }],
        },
        PatchAction::Stage,
    )
    .await?;
```

Only three side/action pairs are valid; anything else is rejected before a
process runs:

| `side`     | `action`  | effect                                     |
| ---------- | --------- | ------------------------------------------ |
| `Unstaged` | `Stage`   | add the selection to the index             |
| `Unstaged` | `Discard` | revert the selection in the worktree       |
| `Staged`   | `Unstage` | remove the selection from the index        |

## Interactive rebase

Git drives interactive rebases through `GIT_SEQUENCE_EDITOR`. This crate points
that variable at a helper executable and passes the plan as JSON in
`FLEET_GIT_SEQUENCE_INSTRUCTION`; the helper rewrites the todo file in place
(preserving comments, writing atomically) and exits.

**The UI binary must call `maybe_run_from_env()` as the very first statement in
`main`, before initializing gpui.** When Git re-invokes the binary as a sequence
editor the call returns `Some(exit_code)` and the process must exit
immediately; in a normal launch it returns `None`.

```rust
fn main() -> std::process::ExitCode {
    if let Some(code) = fleet_git::sequence_editor::maybe_run_from_env() {
        return code;   // invoked by Git as GIT_SEQUENCE_EDITOR
    }
    // ... normal gpui startup ...
    std::process::ExitCode::SUCCESS
}
```

Pass the helper path (the UI's own executable, or the `fleet-git-seqedit`
binary this crate ships) to each rebase call:

```rust
use fleet_git::{MoveDirection, ObjectId, RebaseAction, RebaseBase, RebasePlan, TodoEdit};

let helper = std::env::current_exe()?;
let oid = ObjectId::from("a1b2c3d");

repository.squash_into_previous(&oid, &helper).await?;
repository.fixup_into_previous(&oid, &helper).await?;
repository.drop_commit(&oid, &helper).await?;
repository.reword_commit(&oid, "clearer subject", &helper).await?;
repository.move_commit(&oid, MoveDirection::Up, &helper).await?;   // Up == toward HEAD
repository.edit_commit(&oid, &helper).await?;                      // pauses the rebase

// Or drive the todo file directly.
repository
    .interactive_rebase(
        RebasePlan {
            base: RebaseBase::Reference("HEAD~5".to_owned()),
            onto: None,
            edits: vec![
                TodoEdit::Change { oid: oid.clone(), action: RebaseAction::Fixup { flag: None } },
                TodoEdit::Move { oids: vec![oid], offset: -1 },
            ],
            autostash: true,
            keep_empty: true,
        },
        &helper,
    )
    .await?;
```

A rebase that stops on conflicts returns `GitError::Conflict`; resolve with
`resolve_conflict` / `stage_paths` and then `rebase_continue`, `rebase_skip`, or
`rebase_abort`. The subsequent `snapshot` reports
`OperationState::Rebasing { .. }`.

## Watcher

```rust
use fleet_git::watch::RepoWatcher;

let (watcher, changes) = RepoWatcher::new(repository.paths())?;
// Keep `watcher` alive; dropping it stops the debounce task.
while let Ok(event) = changes.recv().await {
    println!("{} paths changed", event.paths.len());
    let snapshot = repository.snapshot(Default::default()).await?;
    // ... refresh the UI ...
}
```

It watches the worktree recursively plus the real and common Git directories
(`HEAD`, `index`, `refs/`, `packed-refs`, `logs/HEAD`, `config`, and the
operation sentinels), coalesces bursts over ~150 ms, and drops
`.git/objects/**` and `index.lock` churn.

## Command log

Every invocation is recorded with a redacted argv (URL userinfo becomes
`[REDACTED]`) and capped output previews, so the UI can show a lazygit-style
command log.

```rust
use fleet_git::{CommandEvent, CommandKind, CommandOutcome};

let mut events = repository.subscribe_commands();
tokio::spawn(async move {
    while let Ok(event) = events.recv().await {
        match event {
            CommandEvent::Started(record) => println!("→ {}", record.display_argv.join(" ")),
            CommandEvent::Finished(record) => match record.outcome {
                CommandOutcome::Success { .. } => println!("✓ {:?}", record.elapsed),
                CommandOutcome::Failed { status, .. } => println!("✗ exit {status:?}"),
                other => println!("… {other:?}"),
            },
        }
    }
});

for record in repository.recent_commands() {
    if record.kind == CommandKind::Network {
        println!("{}", record.display_argv.join(" "));
    }
}
```

The broadcast channel is bounded (256 events by default); a slow consumer sees
`RecvError::Lagged` and should fall back to `recent_commands()`.

## Testing

```
cargo test -p fleet-git
```

Unit tests cover the parsers and the patch builder with byte fixtures; the
integration tests in `tests/repository.rs` drive real temporary repositories.
They isolate themselves from the developer's Git configuration with
`GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1` on the `Runner`, and
configure identity and `commit.gpgsign=false` inside each temp repo.
