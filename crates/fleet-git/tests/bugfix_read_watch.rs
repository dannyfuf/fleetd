use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use fleet_git::{GitError, Head, Ref, Repository, Runner, SnapshotOptions};

fn git(directory: &Path, arguments: &[&str]) {
    let _ = git_stdout(directory, arguments);
}

fn git_stdout(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn runner() -> Arc<Runner> {
    Arc::new(
        Runner::default()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1"),
    )
}

fn init(directory: &Path) {
    git(directory, &["init", "-b", "main"]);
    git(directory, &["config", "user.name", "Fleet Test"]);
    git(directory, &["config", "user.email", "fleet@example.test"]);
    git(directory, &["config", "commit.gpgsign", "false"]);
}

#[cfg(unix)]
struct GitBarrier {
    signal: PathBuf,
    release: PathBuf,
}

#[cfg(unix)]
impl GitBarrier {
    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !tokio::fs::try_exists(&self.signal).await.unwrap() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    fn release(&self) {
        std::fs::write(&self.release, b"continue").unwrap();
    }
}

#[cfg(unix)]
fn runner_blocking_first_head_description(directory: &Path) -> (Arc<Runner>, GitBarrier) {
    use std::os::unix::fs::PermissionsExt;

    let real_git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout)
        .unwrap()
        .trim()
        .to_owned();
    let wrapper_directory = directory.join(".git/fleet-test-bin");
    std::fs::create_dir_all(&wrapper_directory).unwrap();
    let wrapper = wrapper_directory.join("git");
    std::fs::write(
        &wrapper,
        b"#!/bin/sh\n\
if [ \"$1\" = show ] && [ \"$2\" = -s ] && [ \"$3\" = --format=%s ] && \
   ( set -C; : > \"$FLEET_TEST_ONCE\" ) 2>/dev/null; then\n\
  : > \"$FLEET_TEST_SIGNAL\"\n\
  while [ ! -e \"$FLEET_TEST_RELEASE\" ]; do sleep 0.01; done\n\
fi\n\
exec \"$FLEET_TEST_REAL_GIT\" \"$@\"\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).unwrap();
    let signal = wrapper_directory.join("signal");
    let release = wrapper_directory.join("release");
    let once = wrapper_directory.join("once");
    let path = format!(
        "{}:{}",
        wrapper_directory.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let runner = Runner::default()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH", path)
        .env("FLEET_TEST_REAL_GIT", real_git)
        .env("FLEET_TEST_SIGNAL", signal.as_os_str())
        .env("FLEET_TEST_RELEASE", release.as_os_str())
        .env("FLEET_TEST_ONCE", once.as_os_str());
    (Arc::new(runner), GitBarrier { signal, release })
}

#[cfg(unix)]
#[tokio::test]
async fn discovers_newline_non_utf8_path() {
    use std::ffi::OsString;
    #[cfg(not(target_os = "macos"))]
    use std::os::unix::ffi::OsStringExt;

    let parent = tempfile::tempdir().unwrap();
    #[cfg(target_os = "macos")]
    let name = OsString::from("repo\nline");
    #[cfg(not(target_os = "macos"))]
    let name = OsString::from_vec(b"repo\n\xff".to_vec());
    let repository_path = parent.path().join(name);
    std::fs::create_dir(&repository_path).unwrap();
    init(&repository_path);

    let repository = Repository::discover_with_runner(&repository_path, runner())
        .await
        .unwrap();
    let expected = std::fs::canonicalize(&repository_path).unwrap();

    assert_eq!(repository.paths().worktree_root, expected);
    assert_eq!(repository.paths().git_dir, expected.join(".git"));
    assert_eq!(repository.paths().common_dir, expected.join(".git"));
}

#[tokio::test]
async fn corrupt_repo_is_not_absence() {
    let directory = tempfile::tempdir().unwrap();
    init(directory.path());
    std::fs::write(
        directory.path().join(".git/refs/heads/main"),
        b"not-an-oid\n",
    )
    .unwrap();
    let repository = Repository::discover_with_runner(directory.path(), runner())
        .await
        .unwrap();

    let error = repository
        .commits_for_ref(&Ref::from("main"), None, 10)
        .await
        .unwrap_err();

    assert!(matches!(error, GitError::Exit { .. }));
}

#[tokio::test]
async fn checkout_recency_uses_event_time() {
    let directory = tempfile::tempdir().unwrap();
    init(directory.path());
    std::fs::write(directory.path().join("file"), "base\n").unwrap();
    git(directory.path(), &["add", "file"]);
    let output = Command::new("git")
        .current_dir(directory.path())
        .args(["commit", "-m", "old commit"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "@1000000000 +0000")
        .env("GIT_COMMITTER_DATE", "@1000000000 +0000")
        .output()
        .unwrap();
    assert!(output.status.success());
    git(directory.path(), &["switch", "-c", "topic"]);
    git(directory.path(), &["switch", "main"]);

    let head_log = std::fs::read_to_string(directory.path().join(".git/logs/HEAD")).unwrap();
    let event_time: i64 = head_log
        .lines()
        .next_back()
        .unwrap()
        .split_once('\t')
        .unwrap()
        .0
        .split_whitespace()
        .rev()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let repository = Repository::discover_with_runner(directory.path(), runner())
        .await
        .unwrap();

    let snapshot = repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    let topic = snapshot
        .local_branches
        .iter()
        .find(|branch| branch.name == "topic")
        .unwrap();

    assert_eq!(topic.checked_out_at, Some(event_time));
    assert_ne!(topic.checked_out_at, Some(1_000_000_000));
}

#[cfg(unix)]
#[tokio::test]
async fn snapshot_recollects_a_head_that_moves_after_its_pinned_read() {
    let directory = tempfile::tempdir().unwrap();
    init(directory.path());
    std::fs::write(directory.path().join("file"), "old\n").unwrap();
    git(directory.path(), &["add", "file"]);
    git(directory.path(), &["commit", "-m", "old"]);
    let old_oid = git_stdout(directory.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let (runner, barrier) = runner_blocking_first_head_description(directory.path());
    let repository = Arc::new(
        Repository::discover_with_runner(directory.path(), runner)
            .await
            .unwrap(),
    );
    let snapshot_repository = Arc::clone(&repository);
    let snapshot = tokio::spawn(async move {
        snapshot_repository
            .snapshot(SnapshotOptions::default())
            .await
    });

    barrier.wait().await;
    std::fs::write(directory.path().join("file"), "new\n").unwrap();
    git(directory.path(), &["add", "file"]);
    git(directory.path(), &["commit", "-m", "new"]);
    let new_oid = git_stdout(directory.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    barrier.release();

    let snapshot = tokio::time::timeout(Duration::from_secs(10), snapshot)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.generation, 1);
    assert!(matches!(
        snapshot.head,
        Head::Branch { oid: Some(ref oid), .. } if oid.as_str() == new_oid
    ));
    let described_oids: Vec<_> = repository
        .recent_commands()
        .into_iter()
        .filter_map(|record| match record.display_argv.as_slice() {
            [_, show, short, format, oid]
                if show == "show" && short == "-s" && format == "--format=%s" =>
            {
                Some(oid.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(described_oids, vec![old_oid, new_oid]);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn concurrent_snapshot_does_not_invalidate_in_flight_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    init(directory.path());
    std::fs::write(directory.path().join("file"), "content\n").unwrap();
    git(directory.path(), &["add", "file"]);
    git(directory.path(), &["commit", "-m", "base"]);
    let (runner, barrier) = runner_blocking_first_head_description(directory.path());
    let repository = Arc::new(
        Repository::discover_with_runner(directory.path(), runner)
            .await
            .unwrap(),
    );
    let first_repository = Arc::clone(&repository);
    let first =
        tokio::spawn(async move { first_repository.snapshot(SnapshotOptions::default()).await });
    barrier.wait().await;
    let second_repository = Arc::clone(&repository);
    let second =
        tokio::spawn(async move { second_repository.snapshot(SnapshotOptions::default()).await });
    tokio::task::yield_now().await;
    barrier.release();

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let descriptions = repository
        .recent_commands()
        .into_iter()
        .filter(|record| {
            matches!(
                record.display_argv.as_slice(),
                [_, show, short, format, _]
                    if show == "show" && short == "-s" && format == "--format=%s"
            )
        })
        .count();
    assert_eq!(descriptions, 2);
}

#[tokio::test]
async fn configured_marker_length() {
    let directory = tempfile::tempdir().unwrap();
    init(directory.path());
    std::fs::write(
        directory.path().join(".gitattributes"),
        b"conflict.txt conflict-marker-size=4\n",
    )
    .unwrap();
    let content = b"inline <<<< not a marker\n<<<< HEAD\nours\n====\ntheirs\n>>>> topic\n";
    std::fs::write(directory.path().join("conflict.txt"), content).unwrap();
    let repository = Repository::discover_with_runner(directory.path(), runner())
        .await
        .unwrap();

    let conflict = repository
        .conflicted_file(Path::new("conflict.txt"))
        .await
        .unwrap();

    assert_eq!(conflict.conflicts.len(), 1);
    assert_eq!(conflict.conflicts[0].ours, b"ours\n");
    assert_eq!(conflict.conflicts[0].theirs, b"theirs\n");
}
