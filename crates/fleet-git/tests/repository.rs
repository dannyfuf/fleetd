use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use fleet_git::{
    CommitOptions, ConflictChoice, DiffSide, FetchRequest, GitError, HunkSelection, MergeOptions,
    MoveDirection, ObjectId, PatchAction, PatchSelection, PullRequest, PushRequest, Ref,
    Repository, ResetMode, Runner, SnapshotOptions, StashOptions, watch::RepoWatcher,
};
use tempfile::TempDir;

struct TestRepo {
    directory: TempDir,
    repository: Repository,
}

impl TestRepo {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        git(directory.path(), &["config", "user.name", "Fleet Test"]);
        git(
            directory.path(),
            &["config", "user.email", "fleet@example.test"],
        );
        git(directory.path(), &["config", "commit.gpgsign", "false"]);
        let repository = Repository::discover_with_runner(directory.path(), test_runner())
            .await
            .unwrap();
        Self {
            directory,
            repository,
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.path().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn commit(&self, message: &str) -> ObjectId {
        git(self.path(), &["add", "-A"]);
        git(self.path(), &["commit", "-m", message]);
        ObjectId(
            git_output(self.path(), &["rev-parse", "HEAD"])
                .trim()
                .to_owned(),
        )
    }
}

fn test_runner() -> Arc<Runner> {
    Arc::new(
        Runner::default()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1"),
    )
}

fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn snapshot_stage_lines_commit_amend_and_discard() {
    let repo = TestRepo::new().await;
    repo.write("notes.txt", "one\ntwo\nthree\n");
    repo.commit("base");
    repo.write("notes.txt", "one\nTWO\nthree\nfour\n");
    repo.write("untracked name.txt", "new\n");

    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.files.len(), 2);
    assert_eq!(snapshot.local_branches[0].name, "main");
    assert_eq!(snapshot.commits[0].subject, "base");

    let diff = repo
        .repository
        .diff_file(Path::new("notes.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    let hunk = &diff.files[0].hunks[0];
    let selected: Vec<usize> = hunk
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.content == b"two" || line.content == b"TWO")
        .map(|(index, _)| index)
        .collect();
    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("notes.txt"),
                side: DiffSide::Unstaged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: Some(selected),
                }],
            },
            PatchAction::Stage,
        )
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["show", ":notes.txt"]),
        "one\nTWO\nthree\n"
    );

    repo.repository
        .commit("partial", CommitOptions::default())
        .await
        .unwrap();
    repo.repository.stage_all().await.unwrap();
    repo.repository
        .commit(
            "amended",
            CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["log", "-1", "--format=%s"]).trim(),
        "amended"
    );

    repo.write("notes.txt", "discard me\n");
    repo.repository
        .discard_paths(&[PathBuf::from("notes.txt")])
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("notes.txt")).unwrap(),
        "one\nTWO\nthree\nfour\n"
    );
    repo.repository.unstage_all().await.unwrap();
}

#[tokio::test]
async fn branches_merge_conflict_resolution_and_rebase() {
    let repo = TestRepo::new().await;
    repo.write("conflict.txt", "base\n");
    repo.commit("base");
    repo.repository
        .checkout_new_branch("feature", None)
        .await
        .unwrap();
    repo.write("conflict.txt", "feature\n");
    repo.commit("feature");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    repo.write("conflict.txt", "main\n");
    repo.commit("main");

    let error = repo
        .repository
        .merge(&Ref::from("feature"), MergeOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(error, GitError::Conflict { .. }));
    assert!(matches!(
        repo.repository
            .snapshot(SnapshotOptions::default())
            .await
            .unwrap()
            .operation,
        fleet_git::OperationState::Merging
    ));
    let conflict = repo
        .repository
        .conflicted_file(Path::new("conflict.txt"))
        .await
        .unwrap();
    assert_eq!(conflict.conflicts.len(), 1);
    repo.repository
        .resolve_conflict(Path::new("conflict.txt"), ConflictChoice::Both)
        .await
        .unwrap();
    repo.repository.merge_continue().await.unwrap();

    repo.repository
        .checkout_new_branch("topic", None)
        .await
        .unwrap();
    repo.write("topic.txt", "topic\n");
    repo.commit("topic");
    repo.repository
        .rebase_onto(&Ref::from("main"))
        .await
        .unwrap();
    repo.repository
        .rename_branch("topic", "renamed-topic")
        .await
        .unwrap();
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    repo.repository
        .delete_branch("renamed-topic", true)
        .await
        .unwrap();
}

#[tokio::test]
async fn cherry_pick_revert_reset_stash_and_tags() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    let base = repo.commit("base");
    repo.repository
        .checkout_new_branch("source", None)
        .await
        .unwrap();
    repo.write("picked.txt", "picked\n");
    let picked = repo.commit("picked");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    repo.repository
        .cherry_pick(std::slice::from_ref(&picked))
        .await
        .unwrap();
    repo.repository.revert(&picked).await.unwrap();
    assert!(!repo.path().join("picked.txt").exists());
    repo.repository
        .reset(&Ref(base.0.clone()), ResetMode::Hard)
        .await
        .unwrap();

    repo.write("stash.txt", "stashed\n");
    repo.repository
        .stash_push(StashOptions {
            message: Some("saved".to_owned()),
            include_untracked: true,
            ..StashOptions::default()
        })
        .await
        .unwrap();
    assert!(!repo.path().join("stash.txt").exists());
    repo.repository.stash_apply(0).await.unwrap();
    assert!(repo.path().join("stash.txt").exists());
    git(repo.path(), &["reset", "--hard"]);
    git(repo.path(), &["clean", "-fd"]);
    repo.repository.stash_drop(0).await.unwrap();

    repo.repository
        .create_tag("v1", &Ref::from("HEAD"), Some("release"))
        .await
        .unwrap();
    assert_eq!(
        repo.repository
            .snapshot(SnapshotOptions::default())
            .await
            .unwrap()
            .tags[0]
            .name,
        "v1"
    );
    repo.repository.delete_tag("v1").await.unwrap();
}

#[tokio::test]
async fn local_remote_push_fetch_and_tracking_snapshot() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare"]);
    git(
        repo.path(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );
    repo.repository
        .push(PushRequest {
            remote: Some("origin".to_owned()),
            branch: Some("main".to_owned()),
            set_upstream: true,
            ..PushRequest::default()
        })
        .await
        .unwrap();
    repo.repository
        .fetch(FetchRequest {
            remote: Some("origin".to_owned()),
            prune: true,
            all: false,
        })
        .await
        .unwrap();
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(snapshot.remotes[0].name, "origin");
    assert_eq!(snapshot.remote_branches[0].remote, "origin");
    assert_eq!(
        snapshot.local_branches[0].upstream.as_ref().unwrap().name,
        "origin/main"
    );
}

async fn history_repo() -> TestRepo {
    let repo = TestRepo::new().await;
    for name in ["base", "one", "two", "three"] {
        repo.write(&format!("{name}.txt"), &format!("{name}\n"));
        repo.commit(name);
    }
    repo
}

fn oid_for(repo: &TestRepo, subject: &str) -> ObjectId {
    ObjectId(
        git_output(
            repo.path(),
            &[
                "log",
                "--format=%H",
                "--grep",
                &format!("^{subject}$"),
                "-1",
            ],
        )
        .trim()
        .to_owned(),
    )
}

#[tokio::test]
async fn interactive_rebase_squash_fixup_drop_reword_and_move() {
    let helper = Path::new(env!("CARGO_BIN_EXE_fleet-git-seqedit"));

    let repo = history_repo().await;
    repo.repository
        .squash_into_previous(&oid_for(&repo, "two"), helper)
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "3"
    );

    let repo = history_repo().await;
    repo.repository
        .fixup_into_previous(&oid_for(&repo, "two"), helper)
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "3"
    );

    let repo = history_repo().await;
    repo.repository
        .drop_commit(&oid_for(&repo, "two"), helper)
        .await
        .unwrap();
    assert!(
        !git_output(repo.path(), &["log", "--format=%s"])
            .lines()
            .any(|line| line == "two")
    );

    let repo = history_repo().await;
    repo.repository
        .reword_commit(&oid_for(&repo, "two"), "TWO", helper)
        .await
        .unwrap();
    assert!(
        git_output(repo.path(), &["log", "--format=%s"])
            .lines()
            .any(|line| line == "TWO")
    );

    let repo = history_repo().await;
    repo.repository
        .move_commit(&oid_for(&repo, "two"), MoveDirection::Up, helper)
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["log", "-1", "--format=%s"]).trim(),
        "two"
    );
}

#[tokio::test]
async fn watcher_emits_after_worktree_write() {
    let repo = TestRepo::new().await;
    let (_watcher, receiver) = RepoWatcher::new(repo.repository.paths()).unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    repo.write("watched.txt", "changed\n");
    let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(event.paths.iter().any(|path| path.ends_with("watched.txt")));
}

#[tokio::test]
async fn discovers_linked_worktree_git_indirection() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    let linked = tempfile::tempdir().unwrap();
    let linked_path = linked.path().join("tree");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "linked",
            linked_path.to_str().unwrap(),
        ],
    );
    let discovered = Repository::discover_with_runner(&linked_path, test_runner())
        .await
        .unwrap();
    assert_ne!(discovered.paths().git_dir, discovered.paths().common_dir);
    assert_eq!(
        discovered.paths().worktree_root,
        std::fs::canonicalize(linked_path).unwrap()
    );
}

#[tokio::test]
async fn rejects_a_directory_that_is_not_a_working_tree() {
    let directory = tempfile::tempdir().unwrap();
    let error = Repository::discover_with_runner(directory.path(), test_runner())
        .await
        .unwrap_err();
    assert!(matches!(error, GitError::NotARepository(_)));
}

#[tokio::test]
async fn snapshots_an_unborn_repository() {
    let repo = TestRepo::new().await;
    repo.write("pending.txt", "pending\n");
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        snapshot.head,
        fleet_git::Head::Unborn { ref name } if name == "main"
    ));
    assert_eq!(snapshot.operation, fleet_git::OperationState::None);
    assert!(snapshot.commits.is_empty());
    assert!(snapshot.local_branches.is_empty());
    assert!(snapshot.reflog.is_empty());
    assert!(snapshot.stashes.is_empty());
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.generation, 1);

    // A second snapshot advances the generation counter.
    let next = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(next.generation, 2);
}

#[tokio::test]
async fn diffs_untracked_files_as_pure_additions() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    repo.write("brand new.txt", "one\ntwo\n");

    let diff = repo
        .repository
        .diff_file(Path::new("brand new.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    assert_eq!(diff.files.len(), 1);
    assert_eq!(
        diff.files[0].new_path.as_deref(),
        Some(Path::new("brand new.txt"))
    );
    let lines = &diff.files[0].hunks[0].lines;
    assert_eq!(lines.len(), 2);
    assert!(
        lines
            .iter()
            .all(|line| line.kind == fleet_git::LineKind::Added)
    );
}

#[tokio::test]
async fn diff_context_widens_and_narrows_every_read() {
    let repo = TestRepo::new().await;
    let body: String = (1..=40).map(|line| format!("line{line}\n")).collect();
    repo.write("wide.txt", &body);
    let base = repo.commit("base");
    repo.write("wide.txt", &body.replace("line20\n", "line20 CHANGED\n"));

    let count = |diff: &fleet_git::Diff| diff.files[0].hunks[0].lines.len();
    assert_eq!(
        repo.repository.diff_context(),
        fleet_git::DEFAULT_DIFF_CONTEXT
    );
    let default = repo
        .repository
        .diff_file(Path::new("wide.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    // Three context lines either side of one removal and one addition.
    assert_eq!(count(&default), 8);

    repo.repository.set_diff_context(0);
    let none = repo
        .repository
        .diff_file(Path::new("wide.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    assert_eq!(count(&none), 2);

    repo.repository.set_diff_context(10);
    let wide = repo
        .repository
        .diff_file(Path::new("wide.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    assert_eq!(count(&wide), 22);

    // The commit reads share the same width, so a patch built from a displayed hunk lines up.
    let head = repo.repository.diff_commit(&base, &[]).await;
    assert!(head.is_ok());
    assert_eq!(repo.repository.diff_context(), 10);

    // And it is clamped rather than trusted.
    repo.repository.set_diff_context(u32::MAX);
    assert_eq!(repo.repository.diff_context(), fleet_git::MAX_DIFF_CONTEXT);
}

#[tokio::test]
async fn stages_unstages_and_discards_individual_lines() {
    let repo = TestRepo::new().await;
    repo.write("lines.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.commit("base");
    repo.write("lines.txt", "ONE\ntwo\nthree\nfour\nFIVE\n");

    // Stage only the first change.
    let diff = repo
        .repository
        .diff_file(Path::new("lines.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    let selection = changed_line_indexes(&diff, b"ONE", b"one");
    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("lines.txt"),
                side: DiffSide::Unstaged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: Some(selection),
                }],
            },
            PatchAction::Stage,
        )
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["show", ":lines.txt"]),
        "ONE\ntwo\nthree\nfour\nfive\n"
    );

    // Unstage it again from the staged side; the worktree keeps both edits.
    let staged = repo
        .repository
        .diff_file(Path::new("lines.txt"), DiffSide::Staged)
        .await
        .unwrap();
    let selection = changed_line_indexes(&staged, b"ONE", b"one");
    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("lines.txt"),
                side: DiffSide::Staged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: Some(selection),
                }],
            },
            PatchAction::Unstage,
        )
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["show", ":lines.txt"]),
        "one\ntwo\nthree\nfour\nfive\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("lines.txt")).unwrap(),
        "ONE\ntwo\nthree\nfour\nFIVE\n"
    );

    // Discarding one selected change reverts only that line in the worktree.
    let diff = repo
        .repository
        .diff_file(Path::new("lines.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    let selection = changed_line_indexes(&diff, b"FIVE", b"five");
    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("lines.txt"),
                side: DiffSide::Unstaged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: Some(selection),
                }],
            },
            PatchAction::Discard,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("lines.txt")).unwrap(),
        "ONE\ntwo\nthree\nfour\nfive\n"
    );

    // A side/action mismatch is rejected before any Git process runs.
    let error = repo
        .repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("lines.txt"),
                side: DiffSide::Staged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: None,
                }],
            },
            PatchAction::Stage,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GitError::Parse { .. }));
}

/// Returns the indexes of the hunk lines whose content matches either side of
/// one changed pair.
fn changed_line_indexes(diff: &fleet_git::Diff, added: &[u8], removed: &[u8]) -> Vec<usize> {
    diff.files[0].hunks[0]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.content == added || line.content == removed)
        .filter(|(_, line)| line.kind != fleet_git::LineKind::Context)
        .map(|(index, _)| index)
        .collect()
}

#[tokio::test]
async fn stages_unstages_and_discards_whole_paths() {
    let repo = TestRepo::new().await;
    repo.write("kept.txt", "kept\n");
    repo.commit("base");
    repo.write("kept.txt", "changed\n");
    repo.write("added file.txt", "added\n");

    repo.repository
        .stage_paths(&[PathBuf::from("kept.txt"), PathBuf::from("added file.txt")])
        .await
        .unwrap();
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert!(
        snapshot
            .files
            .iter()
            .all(|file| file.index == fleet_git::ChangeKind::Modified
                || file.index == fleet_git::ChangeKind::Added)
    );

    repo.repository
        .unstage_paths(&[PathBuf::from("kept.txt")])
        .await
        .unwrap();
    assert_eq!(git_output(repo.path(), &["show", ":kept.txt"]), "kept\n");

    // Discarding removes the untracked path and restores the tracked one.
    repo.repository
        .discard_paths(&[PathBuf::from("kept.txt"), PathBuf::from("added file.txt")])
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("kept.txt")).unwrap(),
        "kept\n"
    );
    assert!(!repo.path().join("added file.txt").exists());

    // Discarding a path with nothing to discard is a no-op, not an error.
    repo.repository
        .discard_paths(&[PathBuf::from("kept.txt")])
        .await
        .unwrap();

    repo.write("kept.txt", "changed again\n");
    repo.write("second.txt", "second\n");
    repo.repository.discard_all().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("kept.txt")).unwrap(),
        "kept\n"
    );
    assert!(!repo.path().join("second.txt").exists());
}

#[tokio::test]
async fn reads_commit_diffs_ranges_messages_and_blobs() {
    let repo = TestRepo::new().await;
    repo.write("one.txt", "one\n");
    let first = repo.commit("first");
    repo.write("one.txt", "one changed\n");
    repo.write("two.txt", "two\n");
    let second = repo.commit("second\n\nlonger body\n");

    let files = repo.repository.commit_files(&second).await.unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files.iter().any(
            |file| file.path == Path::new("two.txt") && file.kind == fleet_git::DiffKind::Added
        )
    );

    let diff = repo.repository.diff_commit(&second, &[]).await.unwrap();
    assert_eq!(diff.files.len(), 2);
    let scoped = repo
        .repository
        .diff_commit(&second, &[PathBuf::from("two.txt")])
        .await
        .unwrap();
    assert_eq!(scoped.files.len(), 1);

    let range = repo
        .repository
        .diff_range(&Ref(first.0.clone()), &Ref(second.0.clone()))
        .await
        .unwrap();
    assert_eq!(range.files.len(), 2);

    let message = repo.repository.show_commit_message(&second).await.unwrap();
    assert!(message.starts_with("second"));
    assert!(message.contains("longer body"));

    let blob = repo
        .repository
        .file_at_ref(&Ref(first.0.clone()), Path::new("one.txt"))
        .await
        .unwrap();
    assert_eq!(blob, b"one\n");

    // A branch diff is measured against its merge base with HEAD.
    repo.repository
        .checkout_new_branch("side", Some(&Ref(first.0.clone())))
        .await
        .unwrap();
    repo.write("side.txt", "side\n");
    repo.commit("side");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    let branch = repo
        .repository
        .diff_branch(&Ref::from("side"))
        .await
        .unwrap();
    assert_eq!(branch.files.len(), 1);
    assert_eq!(
        branch.files[0].new_path.as_deref(),
        Some(Path::new("side.txt"))
    );
}

#[tokio::test]
async fn reports_a_rebase_conflict_and_supports_abort() {
    let repo = TestRepo::new().await;
    repo.write("shared.txt", "base\n");
    repo.commit("base");
    repo.repository
        .checkout_new_branch("feature", None)
        .await
        .unwrap();
    repo.write("shared.txt", "feature\n");
    repo.commit("feature");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    repo.write("shared.txt", "main\n");
    repo.commit("main");
    repo.repository
        .checkout(&Ref::from("feature"))
        .await
        .unwrap();

    let error = repo
        .repository
        .rebase_onto(&Ref::from("main"))
        .await
        .unwrap_err();
    let GitError::Conflict { stderr, stdout, .. } = &error else {
        panic!("expected a conflict, got {error:?}");
    };
    assert!(!stderr.is_empty() || !stdout.is_empty());

    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    let fleet_git::OperationState::Rebasing { total, .. } = snapshot.operation else {
        panic!(
            "expected a rebase in progress, got {:?}",
            snapshot.operation
        );
    };
    assert!(total.is_none() || total.is_some_and(|total| total >= 1));
    assert!(snapshot.files.iter().any(|file| file.conflict.is_some()));

    repo.repository.rebase_abort().await.unwrap();
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(snapshot.operation, fleet_git::OperationState::None);
}

#[tokio::test]
async fn resolves_conflicts_by_taking_one_side() {
    for (choice, expected) in [
        (ConflictChoice::Ours, "main\n"),
        (ConflictChoice::Theirs, "feature\n"),
    ] {
        let repo = TestRepo::new().await;
        repo.write("shared.txt", "base\n");
        repo.commit("base");
        repo.repository
            .checkout_new_branch("feature", None)
            .await
            .unwrap();
        repo.write("shared.txt", "feature\n");
        repo.commit("feature");
        repo.repository.checkout(&Ref::from("main")).await.unwrap();
        repo.write("shared.txt", "main\n");
        repo.commit("main");
        repo.repository
            .merge(&Ref::from("feature"), MergeOptions::default())
            .await
            .unwrap_err();

        repo.repository
            .resolve_conflict(Path::new("shared.txt"), choice)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.path().join("shared.txt")).unwrap(),
            expected
        );
        assert_eq!(git_output(repo.path(), &["show", ":shared.txt"]), expected);
        repo.repository.merge_abort().await.unwrap();
    }
}

#[tokio::test]
async fn stash_pop_branch_and_remote_tracking_helpers() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");

    repo.write("base.txt", "stashed\n");
    repo.repository
        .stash_push(StashOptions {
            message: Some("first".to_owned()),
            ..StashOptions::default()
        })
        .await
        .unwrap();
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(snapshot.stashes.len(), 1);
    assert_eq!(snapshot.stashes[0].index, 0);
    assert!(snapshot.stashes[0].subject.contains("first"));
    let stash_diff = repo.repository.diff_stash(0).await.unwrap();
    assert_eq!(stash_diff.files.len(), 1);

    repo.repository.stash_pop(0).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("base.txt")).unwrap(),
        "stashed\n"
    );

    repo.repository
        .stash_push(StashOptions::default())
        .await
        .unwrap();
    repo.repository.stash_branch("from-stash", 0).await.unwrap();
    assert_eq!(
        git_output(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "from-stash"
    );
}

#[tokio::test]
async fn tracks_remote_branches_upstreams_and_pushed_state() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare", "-b", "main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );
    repo.repository
        .push(PushRequest {
            remote: Some("origin".to_owned()),
            branch: Some("main".to_owned()),
            set_upstream: true,
            ..PushRequest::default()
        })
        .await
        .unwrap();

    repo.write("base.txt", "local only\n");
    repo.commit("unpushed");
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(snapshot.commits[0].subject, "unpushed");
    assert!(!snapshot.commits[0].pushed, "newest commit is not upstream");
    assert!(snapshot.commits[1].pushed, "older commit is upstream");
    let upstream = snapshot.local_branches[0].upstream.as_ref().unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!((upstream.ahead, upstream.behind), (1, 0));
    assert!(!snapshot.reflog.is_empty());

    let remotes = repo.repository.remotes().await.unwrap();
    assert_eq!(remotes.len(), 1);
    assert!(remotes[0].fetch_url.is_some());

    // A second clone can check the remote branch out as a tracking branch.
    let clone = tempfile::tempdir().unwrap();
    git(
        clone.path(),
        &["clone", bare.path().to_str().unwrap(), "checkout"],
    );
    let clone_path = clone.path().join("checkout");
    git(clone_path.as_path(), &["config", "user.name", "Fleet Test"]);
    git(
        clone_path.as_path(),
        &["config", "user.email", "fleet@example.test"],
    );
    let cloned = Repository::discover_with_runner(&clone_path, test_runner())
        .await
        .unwrap();
    cloned
        .fetch(FetchRequest {
            all: true,
            prune: true,
            ..FetchRequest::default()
        })
        .await
        .unwrap();
    cloned.create_branch("scratch", None).await.unwrap();
    cloned
        .set_upstream("scratch", "origin", "main")
        .await
        .unwrap();
    let snapshot = cloned.snapshot(SnapshotOptions::default()).await.unwrap();
    let scratch = snapshot
        .local_branches
        .iter()
        .find(|branch| branch.name == "scratch")
        .unwrap();
    assert_eq!(scratch.upstream.as_ref().unwrap().name, "origin/main");
    cloned.unset_upstream("scratch").await.unwrap();
    let snapshot = cloned.snapshot(SnapshotOptions::default()).await.unwrap();
    assert!(
        snapshot
            .local_branches
            .iter()
            .find(|branch| branch.name == "scratch")
            .unwrap()
            .upstream
            .is_none()
    );
}

#[tokio::test]
async fn edit_commit_pauses_the_rebase_until_it_is_continued() {
    let helper = Path::new(env!("CARGO_BIN_EXE_fleet-git-seqedit"));
    let repo = history_repo().await;
    let target = oid_for(&repo, "two");

    repo.repository.edit_commit(&target, helper).await.unwrap();
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        snapshot.operation,
        fleet_git::OperationState::Rebasing {
            interactive: true,
            ..
        }
    ));

    repo.write("extra.txt", "extra\n");
    repo.repository.stage_all().await.unwrap();
    repo.repository
        .commit(
            "amended two",
            CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
        )
        .await
        .unwrap();
    repo.repository.rebase_continue().await.unwrap();

    let subjects = git_output(repo.path(), &["log", "--format=%s"]);
    assert!(subjects.lines().any(|line| line == "amended two"));
    assert_eq!(
        git_output(repo.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "4"
    );
}

#[tokio::test]
async fn records_and_broadcasts_every_command() {
    let repo = TestRepo::new().await;
    let mut events = repo.repository.subscribe_commands();
    repo.write("base.txt", "base\n");
    repo.repository.stage_all().await.unwrap();

    let mut started = 0;
    let mut finished = 0;
    while let Ok(event) = events.try_recv() {
        match event {
            fleet_git::CommandEvent::Started(record) => {
                started += 1;
                assert_eq!(record.display_argv[0], "git");
                assert_eq!(record.outcome, fleet_git::CommandOutcome::Running);
            }
            fleet_git::CommandEvent::Finished(record) => {
                finished += 1;
                assert!(record.elapsed.is_some());
                assert!(matches!(
                    record.outcome,
                    fleet_git::CommandOutcome::Success { .. }
                ));
            }
        }
    }
    assert_eq!(started, 1);
    assert_eq!(finished, 1);

    let recent = repo.repository.recent_commands();
    assert!(!recent.is_empty());
    let last = recent.last().unwrap();
    assert_eq!(last.kind, fleet_git::CommandKind::Mutation);
    assert!(last.display_argv.iter().any(|argument| argument == "add"));

    // A failing command still produces a record and a typed error.
    let error = repo
        .repository
        .checkout(&Ref::from("no-such-branch"))
        .await
        .unwrap_err();
    let GitError::Exit { stderr, argv, .. } = &error else {
        panic!("expected an exit failure, got {error:?}");
    };
    assert!(!stderr.is_empty());
    assert_eq!(argv[0], "git");
    assert!(matches!(
        repo.repository.recent_commands().last().unwrap().outcome,
        fleet_git::CommandOutcome::Failed { .. }
    ));
}

#[tokio::test]
async fn pull_checkout_remote_branch_and_reset_modes() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare", "-b", "main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );
    repo.repository
        .push(PushRequest {
            remote: Some("origin".to_owned()),
            branch: Some("main".to_owned()),
            set_upstream: true,
            ..PushRequest::default()
        })
        .await
        .unwrap();

    // A second clone publishes a commit that the first repository then pulls.
    let other = tempfile::tempdir().unwrap();
    git(
        other.path(),
        &["clone", bare.path().to_str().unwrap(), "work"],
    );
    let other_path = other.path().join("work");
    git(other_path.as_path(), &["config", "user.name", "Fleet Test"]);
    git(
        other_path.as_path(),
        &["config", "user.email", "fleet@example.test"],
    );
    std::fs::write(other_path.join("remote.txt"), "remote\n").unwrap();
    git(other_path.as_path(), &["add", "-A"]);
    git(other_path.as_path(), &["commit", "-m", "from remote"]);
    git(other_path.as_path(), &["push", "origin", "main"]);
    // Publish a second branch for the tracking-checkout below.
    git(other_path.as_path(), &["push", "origin", "main:published"]);

    repo.repository
        .pull(PullRequest {
            remote: Some("origin".to_owned()),
            branch: Some("main".to_owned()),
            ff_only: true,
            ..PullRequest::default()
        })
        .await
        .unwrap();
    assert!(repo.path().join("remote.txt").exists());

    repo.repository
        .fetch(FetchRequest {
            remote: Some("origin".to_owned()),
            ..FetchRequest::default()
        })
        .await
        .unwrap();
    repo.repository
        .checkout_remote_branch("origin", "published")
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "published"
    );
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    let published = snapshot
        .local_branches
        .iter()
        .find(|branch| branch.name == "published")
        .unwrap();
    assert!(published.is_head);
    assert_eq!(
        published.upstream.as_ref().unwrap().name,
        "origin/published"
    );

    // Soft and mixed resets move HEAD while keeping the worktree intact.
    repo.repository
        .reset(&Ref::from("HEAD~1"), ResetMode::Soft)
        .await
        .unwrap();
    assert!(repo.path().join("remote.txt").exists());
    assert_eq!(
        git_output(repo.path(), &["diff", "--cached", "--name-only"]).trim(),
        "remote.txt"
    );
    repo.repository
        .reset(&Ref::from("HEAD"), ResetMode::Mixed)
        .await
        .unwrap();
    assert!(
        git_output(repo.path(), &["diff", "--cached", "--name-only"])
            .trim()
            .is_empty()
    );
    assert!(repo.path().join("remote.txt").exists());
}

#[tokio::test]
async fn merge_options_produce_squash_and_no_ff_results() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    repo.repository
        .checkout_new_branch("topic", None)
        .await
        .unwrap();
    repo.write("topic.txt", "topic\n");
    repo.commit("topic");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();

    // `--squash` stages the change without recording a merge commit.
    repo.repository
        .merge(
            &Ref::from("topic"),
            MergeOptions {
                squash: true,
                ..MergeOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "1"
    );
    repo.repository
        .commit("squashed topic", CommitOptions::default())
        .await
        .unwrap();

    // `--ff-only` refuses a merge that would need a merge commit.
    repo.repository
        .checkout_new_branch("second", Some(&Ref::from("topic")))
        .await
        .unwrap();
    repo.write("second.txt", "second\n");
    repo.commit("second");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    let error = repo
        .repository
        .merge(
            &Ref::from("second"),
            MergeOptions {
                ff_only: true,
                ..MergeOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GitError::Exit { .. }));

    repo.repository
        .merge(
            &Ref::from("second"),
            MergeOptions {
                no_ff: true,
                ..MergeOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        git_output(repo.path(), &["rev-list", "--count", "--merges", "HEAD"]).trim(),
        "1"
    );
}

#[tokio::test]
async fn an_empty_message_amends_in_place_and_keeps_the_subject() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("only commit");

    repo.write("extra.txt", "extra\n");
    repo.repository.stage_all().await.unwrap();
    repo.repository
        .commit(
            "",
            CommitOptions {
                amend: true,
                allow_empty: true,
                ..CommitOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        git_output(repo.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "1"
    );
    assert_eq!(
        git_output(repo.path(), &["log", "-1", "--format=%s"]).trim(),
        "only commit"
    );
    assert_eq!(
        git_output(repo.path(), &["show", "--name-only", "--format=", "HEAD"])
            .split_whitespace()
            .collect::<Vec<_>>(),
        vec!["base.txt", "extra.txt"]
    );
}

#[tokio::test]
async fn rebase_onto_autostashes_a_dirty_worktree() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    repo.repository
        .checkout_new_branch("topic", None)
        .await
        .unwrap();
    repo.write("topic.txt", "topic\n");
    repo.commit("topic");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    repo.write("main.txt", "main\n");
    repo.commit("main moves on");
    repo.repository.checkout(&Ref::from("topic")).await.unwrap();

    // A dirty worktree would otherwise make `git rebase` refuse to start.
    repo.write("base.txt", "base dirty\n");
    repo.repository
        .rebase_onto(&Ref::from("main"))
        .await
        .unwrap();

    assert_eq!(
        git_output(repo.path(), &["log", "--format=%s"])
            .lines()
            .collect::<Vec<_>>(),
        vec!["topic", "main moves on", "base"]
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("base.txt")).unwrap(),
        "base dirty\n"
    );
}

#[tokio::test]
async fn diff_paths_combines_tracked_and_untracked_children() {
    let repo = TestRepo::new().await;
    repo.write("src/app/one.txt", "one\n");
    repo.write("src/app/two.txt", "two\n");
    repo.commit("base");
    repo.write("src/app/one.txt", "one changed\n");
    repo.write("src/app/two.txt", "two changed\n");
    git(repo.path(), &["add", "src/app/two.txt"]);
    repo.write("src/app/new.txt", "brand new\n");

    let unstaged = repo
        .repository
        .diff_paths(
            &[
                PathBuf::from("src/app/one.txt"),
                PathBuf::from("src/app/two.txt"),
                PathBuf::from("src/app/new.txt"),
            ],
            DiffSide::Unstaged,
        )
        .await
        .unwrap();
    // The tracked worktree change first, then the untracked file as an all-added patch.
    let paths: Vec<_> = unstaged
        .files
        .iter()
        .map(|file| file.new_path.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        paths,
        vec![
            PathBuf::from("src/app/one.txt"),
            PathBuf::from("src/app/new.txt")
        ]
    );

    let staged = repo
        .repository
        .diff_paths(
            &[
                PathBuf::from("src/app/one.txt"),
                PathBuf::from("src/app/two.txt"),
            ],
            DiffSide::Staged,
        )
        .await
        .unwrap();
    assert_eq!(staged.files.len(), 1);
    assert_eq!(
        staged.files[0].new_path.as_deref(),
        Some(Path::new("src/app/two.txt"))
    );

    let empty = repo
        .repository
        .diff_paths(&[], DiffSide::Unstaged)
        .await
        .unwrap();
    assert!(empty.files.is_empty());
}
