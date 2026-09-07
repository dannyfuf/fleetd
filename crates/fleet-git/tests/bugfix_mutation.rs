#[allow(dead_code)]
mod support;

use fleet_git::{
    CommitOptions, ConflictChoice, DiffSide, GitError, HunkSelection, MergeOptions, PatchAction,
    PatchSelection, Ref,
};
use std::path::{Path, PathBuf};
use support::{TestRepo, git, git_output};

#[tokio::test]
async fn discard_unborn_staged_addition() {
    let repo = TestRepo::new().await;
    repo.write("selected.txt", "staged\n");
    repo.write("remaining.txt", "remaining\n");
    git(repo.path(), &["add", "selected.txt", "remaining.txt"]);
    repo.write("selected.txt", "modified after staging\n");

    repo.repository
        .discard_paths(&[PathBuf::from("selected.txt")])
        .await
        .unwrap();
    assert!(!repo.path().join("selected.txt").exists());
    assert_eq!(git_output(repo.path(), &["ls-files"]), "remaining.txt\n");

    repo.repository.discard_all().await.unwrap();
    assert!(!repo.path().join("remaining.txt").exists());
    assert!(git_output(repo.path(), &["ls-files"]).is_empty());
}

#[tokio::test]
async fn partial_patch_zero_context() {
    let repo = TestRepo::new().await;
    repo.write("lines.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.commit("base");
    repo.write("lines.txt", "ONE\ntwo\nthree\nfour\nFIVE\n");
    repo.repository.set_diff_context(0);

    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("lines.txt"),
                side: DiffSide::Unstaged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: None,
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
    assert_eq!(
        std::fs::read_to_string(repo.path().join("lines.txt")).unwrap(),
        "ONE\ntwo\nthree\nfour\nFIVE\n"
    );
}

#[tokio::test]
async fn partial_patch_rejects_changed_preimage() {
    let repo = TestRepo::new().await;
    repo.write("lines.txt", "one\ntwo\nthree\n");
    repo.commit("base");
    repo.write("lines.txt", "ONE\ntwo\nthree\n");
    let displayed = repo
        .repository
        .diff_file(Path::new("lines.txt"), DiffSide::Unstaged)
        .await
        .unwrap();
    repo.write("lines.txt", "zero\nONE\ntwo\nthree\n");

    let error = repo
        .repository
        .apply_patch_selection_verified(
            PatchSelection {
                path: PathBuf::from("lines.txt"),
                side: DiffSide::Unstaged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: None,
                }],
            },
            PatchAction::Stage,
            &displayed,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, GitError::Parse { .. }));
    assert_eq!(
        git_output(repo.path(), &["show", ":lines.txt"]),
        "one\ntwo\nthree\n"
    );
}

#[tokio::test]
async fn stash_index_shift_is_rejected() {
    let repo = TestRepo::new().await;
    repo.write("file.txt", "base\n");
    repo.commit("base");
    repo.write("file.txt", "first\n");
    git(repo.path(), &["stash", "push", "-m", "reviewed"]);
    let reviewed = repo
        .repository
        .snapshot(Default::default())
        .await
        .unwrap()
        .stashes[0]
        .oid
        .clone();
    repo.write("file.txt", "second\n");
    git(repo.path(), &["stash", "push", "-m", "newer"]);

    let error = repo
        .repository
        .stash_drop_verified(0, &reviewed)
        .await
        .unwrap_err();

    assert!(matches!(error, GitError::Parse { .. }));
    let stashes = repo
        .repository
        .snapshot(Default::default())
        .await
        .unwrap()
        .stashes;
    assert_eq!(stashes.len(), 2);
    assert!(stashes.iter().any(|stash| stash.oid == reviewed));
}

#[tokio::test]
async fn revert_uses_revert_commands() {
    let repo = TestRepo::new().await;
    repo.write("file.txt", "base\n");
    repo.commit("base");

    let continuing = repo.repository.revert_continue().await.unwrap_err();
    let aborting = repo.repository.revert_abort().await.unwrap_err();

    assert_command(continuing, &["revert", "--continue"]);
    assert_command(aborting, &["revert", "--abort"]);
}

fn assert_command(error: GitError, expected: &[&str]) {
    let argv = match error {
        GitError::Exit { argv, .. } | GitError::Conflict { argv, .. } => argv,
        other => panic!("expected command failure, got {other:?}"),
    };
    assert!(
        argv.windows(expected.len()).any(|window| window
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())),
        "expected {expected:?} in {argv:?}"
    );
}

#[tokio::test]
async fn checkout_ref_not_path() {
    let repo = TestRepo::new().await;
    repo.write("stale-ref", "committed\n");
    repo.commit("base");
    repo.write("stale-ref", "worktree change\n");

    let error = repo
        .repository
        .checkout(&Ref::from("stale-ref"))
        .await
        .unwrap_err();

    assert!(matches!(error, GitError::Exit { .. }));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("stale-ref")).unwrap(),
        "worktree change\n"
    );
}

#[tokio::test]
async fn branch_shadowing_tag_checks_out_branch() {
    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    git(repo.path(), &["tag", "main"]);
    repo.repository
        .checkout_new_branch("topic", None)
        .await
        .unwrap();

    repo.repository.checkout(&Ref::from("main")).await.unwrap();

    assert_eq!(
        git_output(repo.path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/main\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlink_and_changed_preimage() {
    use std::os::unix::fs::symlink;

    let repo = TestRepo::new().await;
    let outside = tempfile::NamedTempFile::new().unwrap();
    let conflict = b"<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n";
    std::fs::write(outside.path(), conflict).unwrap();
    symlink(outside.path(), repo.path().join("conflict.txt")).unwrap();

    let error = repo
        .repository
        .resolve_conflict(Path::new("conflict.txt"), ConflictChoice::Ours)
        .await
        .unwrap_err();

    assert!(matches!(error, GitError::Parse { .. }));
    assert_eq!(std::fs::read(outside.path()).unwrap(), conflict);
}

#[cfg(unix)]
#[tokio::test]
async fn hook_failure_preserves_error() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new().await;
    repo.write("base.txt", "base\n");
    repo.commit("base");
    repo.repository
        .checkout_new_branch("feature", None)
        .await
        .unwrap();
    repo.write("feature.txt", "feature\n");
    repo.commit("feature");
    repo.repository.checkout(&Ref::from("main")).await.unwrap();
    repo.write("main.txt", "main\n");
    repo.commit("main");

    let hook = repo.path().join(".git/hooks/pre-merge-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let error = repo
        .repository
        .merge(&Ref::from("feature"), MergeOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(error, GitError::Exit { .. }));
    assert!(repo.path().join(".git/MERGE_HEAD").exists());
    assert!(git_output(repo.path(), &["ls-files", "--unmerged"]).is_empty());
}

#[tokio::test]
async fn success_stdout_is_not_warning() {
    let repo = TestRepo::new().await;
    repo.write("committed.txt", "content\n");
    repo.repository.stage_all().await.unwrap();

    let result = repo
        .repository
        .commit("normal output", CommitOptions::default())
        .await
        .unwrap();

    assert_eq!(result.warning, None);
}
