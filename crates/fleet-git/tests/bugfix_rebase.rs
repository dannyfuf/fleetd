#![cfg(unix)]

use std::{
    future::{Future, poll_fn},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    task::Poll,
};

use fleet_git::{CommandKind, MoveDirection};

#[allow(dead_code)]
mod support;
use support::{TestRepo, git_output};

fn sequence_editor() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_fleet-git-seqedit"))
}

fn shell_literal(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn barrier_editor() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().expect("barrier directory");
    let marker = directory.path().join("entered");
    let release = directory.path().join("release");
    let editor = directory.path().join("sequence-editor");
    let script = format!(
        "#!/bin/sh\n: > {}\nwhile [ ! -e {} ]; do sleep 0.01; done\nexec {} \"$@\"\n",
        shell_literal(&marker),
        shell_literal(&release),
        shell_literal(sequence_editor()),
    );
    std::fs::write(&editor, script).expect("write barrier editor");
    let mut permissions = std::fs::metadata(&editor)
        .expect("barrier editor metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&editor, permissions).expect("make barrier editor executable");
    (directory, marker, release, editor)
}

async fn wait_for_file(
    path: &Path,
    task: &tokio::task::JoinHandle<fleet_git::Result<fleet_git::MutationResult>>,
) {
    while !path.exists() {
        assert!(
            !task.is_finished(),
            "reword finished before reaching barrier"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn reword_excludes_concurrent_mutation() {
    let repo = TestRepo::new().await;
    repo.write("root.txt", "root\n");
    repo.commit("root");
    repo.write("middle.txt", "middle\n");
    let middle = repo.commit("middle");
    repo.write("tip.txt", "tip\n");
    repo.commit("tip");
    let repository = Arc::new(repo.repository);
    let (_barrier, marker, release, editor) = barrier_editor();

    let reword_repository = Arc::clone(&repository);
    let reword = tokio::spawn(async move {
        reword_repository
            .reword_commit(&middle, "MIDDLE", &editor)
            .await
    });
    wait_for_file(&marker, &reword).await;

    let mut concurrent_mutation = Box::pin(repository.stage_all());
    let queued = poll_fn(|context| {
        Poll::Ready(matches!(
            concurrent_mutation.as_mut().poll(context),
            Poll::Pending
        ))
    })
    .await;
    assert!(
        queued,
        "concurrent mutation should wait for the rebase lock"
    );

    std::fs::write(release, b"").expect("release sequence editor");
    let (reword, concurrent_mutation) = tokio::join!(reword, concurrent_mutation);
    reword.expect("reword task").expect("reword commit");
    concurrent_mutation.expect("concurrent mutation");

    let mutation_commands: Vec<_> = repository
        .recent_commands()
        .into_iter()
        .filter(|record| record.kind == CommandKind::Mutation)
        .filter_map(|record| record.display_argv.get(1).cloned())
        .collect();
    assert_eq!(mutation_commands, ["rebase", "commit", "rebase", "add"]);
}

#[tokio::test]
async fn reword_failure_aborts_rebase() {
    let repo = TestRepo::new().await;
    repo.write("root.txt", "root\n");
    repo.commit("root");
    repo.write("middle.txt", "middle\n");
    let middle = repo.commit("middle");
    repo.write("tip.txt", "tip\n");
    repo.commit("tip");
    let hook = repo.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").expect("write failing hook");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .expect("make hook executable");

    let error = repo
        .repository
        .reword_commit(&middle, "MIDDLE", sequence_editor())
        .await
        .expect_err("amend hook should fail");

    assert!(matches!(error, fleet_git::GitError::Exit { .. }));
    assert!(!repo.path().join(".git/rebase-merge").exists());
    assert!(!repo.path().join(".git/rebase-apply").exists());
    assert_eq!(
        git_output(repo.path(), &["log", "--format=%s", "--reverse"]),
        "root\nmiddle\ntip\n"
    );
    assert!(git_output(repo.path(), &["status", "--porcelain"]).is_empty());
}

#[tokio::test]
async fn root_rewrite_selects_valid_base() {
    let repo = TestRepo::new().await;
    repo.write("root.txt", "root\n");
    let root = repo.commit("root");
    repo.write("second.txt", "second\n");
    repo.commit("second");
    repo.write("third.txt", "third\n");
    repo.commit("third");

    repo.repository
        .reword_commit(&root, "ROOT", sequence_editor())
        .await
        .expect("reword root commit");
    assert_eq!(
        git_output(repo.path(), &["log", "--format=%s", "--reverse"]),
        "ROOT\nsecond\nthird\n"
    );

    let repo = TestRepo::new().await;
    repo.write("root.txt", "root\n");
    repo.commit("root");
    repo.write("second.txt", "second\n");
    let second = repo.commit("second");
    repo.write("third.txt", "third\n");
    repo.commit("third");

    repo.repository
        .move_commit(&second, MoveDirection::Up, sequence_editor())
        .await
        .expect("move the root's immediate successor");
    assert_eq!(
        git_output(repo.path(), &["log", "--format=%s", "--reverse"]),
        "root\nthird\nsecond\n"
    );
}
