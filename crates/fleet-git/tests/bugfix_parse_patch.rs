#[allow(dead_code)]
mod support;

use std::{path::PathBuf, process::Command};

use fleet_git::{DiffSide, HunkSelection, PatchAction, PatchSelection};
use support::{TestRepo, git, git_output};

#[cfg(unix)]
#[tokio::test]
async fn executable_partial_patch_is_accepted_by_git() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new().await;
    repo.write("script", "echo hi\n");
    std::fs::set_permissions(
        repo.path().join("script"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("script"),
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

    assert!(
        git_output(repo.path(), &["ls-files", "--stage", "--", "script"]).starts_with("100755 ")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn quoted_special_path_partial_patch_is_accepted_by_git() {
    let repo = TestRepo::new().await;
    let relative = PathBuf::from("name-\t");
    std::fs::write(repo.path().join(&relative), b"old\n").unwrap();
    repo.commit("base");
    std::fs::write(repo.path().join(&relative), b"new\n").unwrap();

    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: relative.clone(),
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

    let staged = repo
        .repository
        .diff_file(&relative, DiffSide::Staged)
        .await
        .unwrap();
    assert_eq!(
        staged.files[0].new_path.as_deref(),
        Some(relative.as_path())
    );
    assert_eq!(staged.files[0].hunks[0].lines[1].content, b"new");
}

#[tokio::test]
async fn partial_deleted_file_line_can_be_staged() {
    let repo = TestRepo::new().await;
    repo.write("deleted", "one\ntwo\nthree\n");
    repo.commit("base");
    std::fs::remove_file(repo.path().join("deleted")).unwrap();

    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("deleted"),
                side: DiffSide::Unstaged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: Some(vec![0]),
                }],
            },
            PatchAction::Stage,
        )
        .await
        .unwrap();

    assert_eq!(
        git_output(repo.path(), &["show", ":deleted"]),
        "two\nthree\n"
    );
}

#[tokio::test]
async fn partial_new_file_line_can_be_unstaged() {
    let repo = TestRepo::new().await;
    repo.write("added", "one\ntwo\nthree\n");
    git(repo.path(), &["add", "added"]);

    repo.repository
        .apply_patch_selection(
            PatchSelection {
                path: PathBuf::from("added"),
                side: DiffSide::Staged,
                hunks: vec![HunkSelection {
                    hunk_index: 0,
                    lines: Some(vec![0]),
                }],
            },
            PatchAction::Unstage,
        )
        .await
        .unwrap();

    assert_eq!(git_output(repo.path(), &["show", ":added"]), "two\nthree\n");
}

#[tokio::test]
async fn configured_diff_prefixes_do_not_break_diff_parsing() {
    let repo = TestRepo::new().await;
    repo.write("file", "old\n");
    repo.commit("base");
    repo.write("file", "new\n");

    git(repo.path(), &["config", "diff.mnemonicPrefix", "true"]);
    let mnemonic = repo
        .repository
        .diff_file(&PathBuf::from("file"), DiffSide::Unstaged)
        .await
        .unwrap();
    assert_eq!(mnemonic.files.len(), 1);
    assert_eq!(mnemonic.files[0].hunks.len(), 1);

    git(repo.path(), &["config", "diff.mnemonicPrefix", "false"]);
    git(repo.path(), &["config", "diff.noprefix", "true"]);
    let no_prefix = repo
        .repository
        .diff_file(&PathBuf::from("file"), DiffSide::Unstaged)
        .await
        .unwrap();
    assert_eq!(no_prefix.files.len(), 1);
    assert_eq!(no_prefix.files[0].hunks.len(), 1);
}

#[tokio::test]
async fn combined_diff_does_not_hide_other_requested_files() {
    let repo = TestRepo::new().await;
    repo.write("dir/a", "base a\n");
    repo.write("dir/b", "base b\n");
    repo.commit("base");

    git(repo.path(), &["checkout", "-b", "other"]);
    repo.write("dir/b", "other b\n");
    repo.commit("other changes b");
    git(repo.path(), &["checkout", "main"]);
    repo.write("dir/b", "main b\n");
    repo.commit("main changes b");
    repo.write("dir/a", "changed a\n");

    let merge = Command::new("git")
        .current_dir(repo.path())
        .args(["merge", "other"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(!merge.status.success());

    let diff = repo
        .repository
        .diff_paths(
            &[PathBuf::from("dir/a"), PathBuf::from("dir/b")],
            DiffSide::Unstaged,
        )
        .await
        .unwrap();

    assert_eq!(diff.files.len(), 1);
    assert_eq!(
        diff.files[0].new_path.as_deref(),
        Some(std::path::Path::new("dir/a"))
    );
}
