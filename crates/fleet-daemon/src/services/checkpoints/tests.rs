//! Checkpoint and revert behaviour, against a real `git` in a real temporary worktree.
//!
//! The subject here *is* Git and the filesystem, which is the one case `rust-gpui-testing` says a
//! real tempdir is for: a fake shell replaying canned stdout would prove that this service builds
//! the argv it builds, and nothing about whether reverting a tree restores it. `fleet-git`'s own
//! suite makes the same call for the same reason.
//!
//! Every invocation — the service's and the fixture's — runs with the developer's global and
//! system Git configuration switched off, so a machine with `core.autocrlf` or a `commit.gpgsign`
//! default cannot change what these tests mean.

use std::{collections::HashSet, path::Path, sync::Arc};

use async_trait::async_trait;
use fleet_core::agents::{ThreadId, TurnId};
use fleet_proto::agents::{CheckpointId, CheckpointScope};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use crate::adapters::shell::{
    DetachedProcess, LineCallback, RealShell, Shell, ShellCommand, ShellResult,
};

use super::{CheckpointError, Checkpoints};

/// A real shell with the developer's own Git configuration switched off.
///
/// Production deliberately inherits the user's configuration — their `core.autocrlf`, their
/// filters — because a checkpoint has to round-trip the bytes Git itself would write. A test
/// cannot afford that, so the isolation is injected at the one boundary the service has.
struct IsolatedShell {
    inner: RealShell,
}

#[async_trait]
impl Shell for IsolatedShell {
    async fn run(&self, command: ShellCommand) -> crate::DaemonResult<ShellResult> {
        self.inner.run(isolate(command)).await
    }

    async fn run_detached(
        &self,
        command: ShellCommand,
        log_path: &Path,
    ) -> crate::DaemonResult<DetachedProcess> {
        self.inner.run_detached(isolate(command), log_path).await
    }

    async fn run_streaming(
        &self,
        command: ShellCommand,
        cancel: CancellationToken,
        on_line: LineCallback,
    ) -> crate::DaemonResult<ShellResult> {
        self.inner
            .run_streaming(isolate(command), cancel, on_line)
            .await
    }
}

fn isolate(command: ShellCommand) -> ShellCommand {
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
}

/// One temporary repository, the service under test, and the Git calls the assertions need.
struct Fixture {
    directory: TempDir,
    checkpoints: Checkpoints,
    thread: ThreadId,
    turn: TurnId,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let fixture = Self {
            directory,
            checkpoints: Checkpoints::new(Arc::new(IsolatedShell { inner: RealShell })),
            thread: ThreadId::new(),
            turn: TurnId::new(),
        };
        fixture.git(&["init", "-b", "main"]).await;
        fixture.git(&["config", "user.name", "Fleet Test"]).await;
        fixture
            .git(&["config", "user.email", "fleet@example.test"])
            .await;
        fixture.git(&["config", "commit.gpgsign", "false"]).await;
        fixture.write("a.txt", "one\n");
        fixture.write("sub/b.txt", "two\n");
        fixture.write(".gitignore", "ignored/\n");
        fixture.write("ignored/cache.bin", "do not touch\n");
        fixture.git(&["add", "-A"]).await;
        fixture.git(&["commit", "-m", "init"]).await;
        fixture
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|error| panic!("{error}"));
        }
        std::fs::write(path, content).unwrap_or_else(|error| panic!("{error}"));
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.path().join(relative)).ok()
    }

    fn exists(&self, relative: &str) -> bool {
        self.path().join(relative).exists()
    }

    async fn git(&self, arguments: &[&str]) -> String {
        self.git_in(self.path(), arguments).await
    }

    async fn git_in(&self, directory: &Path, arguments: &[&str]) -> String {
        let output = tokio::process::Command::new("git")
            .current_dir(directory)
            .args(arguments)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Fleet Test")
            .env("GIT_AUTHOR_EMAIL", "fleet@example.test")
            .env("GIT_COMMITTER_NAME", "Fleet Test")
            .env("GIT_COMMITTER_EMAIL", "fleet@example.test")
            .output()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The tree the whole non-ignored working tree would make right now.
    ///
    /// The same snapshot a capture takes, computed independently through the Git CLI, so an
    /// assertion that a revert restored "exactly the recorded tree" compares object identities
    /// rather than a handful of file contents.
    async fn tree_now(&self) -> String {
        // Unique, and outside the worktree: an index file *inside* it would be staged by the
        // very `git add --all` that writes it, and a shared name would race the other tests.
        let index =
            std::env::temp_dir().join(format!("fleet-fixture-{}.index", uuid::Uuid::new_v4()));
        self.git_with_index(&index, &["read-tree", "--empty"]).await;
        self.git_with_index(&index, &["add", "--all"]).await;
        let tree = self.git_with_index(&index, &["write-tree"]).await;
        match std::fs::remove_file(&index) {
            Ok(()) => {}
            Err(error) => panic!("{error}"),
        }
        tree.trim().to_owned()
    }

    async fn git_with_index(&self, index: &Path, arguments: &[&str]) -> String {
        let output = tokio::process::Command::new("git")
            .current_dir(self.path())
            .args(arguments)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_INDEX_FILE", index)
            .output()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    async fn tree_of(&self, checkpoint: &CheckpointId) -> String {
        let reference = format!(
            "refs/fleet/checkpoints/{}/{checkpoint}^{{tree}}",
            self.thread
        );
        self.git(&["rev-parse", &reference]).await.trim().to_owned()
    }

    async fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).await.trim().to_owned()
    }

    async fn status(&self) -> String {
        self.git(&["status", "--porcelain"]).await
    }

    async fn checkpoint_refs(&self) -> Vec<String> {
        self.git(&["for-each-ref", "--format=%(refname)", "refs/fleet/"])
            .await
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[tokio::test]
async fn a_turn_checkpoint_records_the_worktree_as_it_was_before_the_turn() {
    let fixture = Fixture::new().await;
    // Uncommitted work is the interesting case: a checkpoint that only recorded `HEAD` would
    // lose everything the user had not committed when the turn started.
    fixture.write("a.txt", "edited before the turn\n");
    let expected = fixture.tree_now().await;

    let checkpoint = fixture
        .checkpoints
        .capture_turn(fixture.path(), fixture.thread, fixture.turn)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(checkpoint.ordinal, 1);
    assert_eq!(checkpoint.scope, CheckpointScope::Turn);
    assert_eq!(checkpoint.turn, fixture.turn);
    assert_eq!(fixture.tree_of(&checkpoint.id).await, expected);
    assert_eq!(
        fixture.checkpoint_refs().await,
        vec![format!(
            "refs/fleet/checkpoints/{}/{}",
            fixture.thread, checkpoint.id
        )]
    );
}

#[tokio::test]
async fn reverting_restores_exactly_the_recorded_tree() {
    let fixture = Fixture::new().await;
    let before = fixture.tree_now().await;
    let status_before = fixture.status().await;
    let head_before = fixture.head().await;
    let checkpoint = fixture
        .checkpoints
        .capture_turn(fixture.path(), fixture.thread, fixture.turn)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    // What a turn does: rewrite a file, delete another, and generate a new directory.
    fixture.write("a.txt", "the agent's version\n");
    std::fs::remove_file(fixture.path().join("sub/b.txt"))
        .unwrap_or_else(|error| panic!("{error}"));
    fixture.write("generated/out.rs", "fn main() {}\n");
    fixture.write("ignored/cache.bin", "rebuilt\n");

    let report = fixture
        .checkpoints
        .revert(fixture.path(), fixture.thread, &checkpoint.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        fixture.tree_now().await,
        before,
        "the tree must be restored"
    );
    assert_eq!(report.restored, 2, "a.txt and sub/b.txt");
    assert_eq!(report.deleted, 1, "generated/out.rs");
    assert_eq!(
        report.paths,
        vec![
            "a.txt".to_owned(),
            "generated/out.rs".to_owned(),
            "sub/b.txt".to_owned()
        ]
    );
    assert_eq!(fixture.read("a.txt").as_deref(), Some("one\n"));
    assert_eq!(fixture.read("sub/b.txt").as_deref(), Some("two\n"));
    assert!(!fixture.exists("generated/out.rs"));
    assert!(
        !fixture.exists("generated"),
        "a directory the turn created and the revert emptied must not be left behind"
    );
    // An ignored file is in neither tree, so a revert must leave it exactly as the turn left it:
    // wiping `target/` because a turn rebuilt it would be a far bigger surprise than keeping it.
    assert_eq!(
        fixture.read("ignored/cache.bin").as_deref(),
        Some("rebuilt\n")
    );
    assert_eq!(fixture.head().await, head_before, "HEAD must not move");
    assert_eq!(fixture.status().await, status_before);
}

#[tokio::test]
async fn a_revert_recreates_a_directory_the_turn_removed() {
    let fixture = Fixture::new().await;
    fixture.write("deep/nest/kept.txt", "original\n");
    let before = fixture.tree_now().await;
    let checkpoint = fixture
        .checkpoints
        .capture_turn(fixture.path(), fixture.thread, fixture.turn)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    std::fs::remove_dir_all(fixture.path().join("deep")).unwrap_or_else(|error| panic!("{error}"));

    let report = fixture
        .checkpoints
        .revert(fixture.path(), fixture.thread, &checkpoint.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    // `git checkout-index --force` creates the leading directories, which is the only reason a
    // restore can put a file back where the turn removed its whole tree.
    assert_eq!(report.restored, 1);
    assert_eq!(
        fixture.read("deep/nest/kept.txt").as_deref(),
        Some("original\n")
    );
    assert_eq!(fixture.tree_now().await, before);
}

#[tokio::test]
async fn a_revert_leaves_the_users_staged_work_staged() {
    let fixture = Fixture::new().await;
    fixture.write("staged.txt", "deliberately staged\n");
    fixture.git(&["add", "staged.txt"]).await;
    let staged_before = fixture.git(&["diff", "--cached", "--name-only"]).await;
    let checkpoint = fixture
        .checkpoints
        .capture_turn(fixture.path(), fixture.thread, fixture.turn)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    fixture.write("a.txt", "the agent's version\n");
    fixture
        .checkpoints
        .revert(fixture.path(), fixture.thread, &checkpoint.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]).await,
        staged_before,
        "the revert ran against a scratch index and must not have rewritten the user's"
    );
    assert_eq!(fixture.read("a.txt").as_deref(), Some("one\n"));
}

#[tokio::test]
async fn reverting_a_thread_with_no_checkpoint_is_a_typed_refusal_and_changes_nothing() {
    let fixture = Fixture::new().await;
    fixture.write("a.txt", "the agent's version\n");
    let before = fixture.tree_now().await;
    let absent = CheckpointId::from_parts(1, CheckpointScope::Turn, fixture.turn);

    let error = fixture
        .checkpoints
        .revert(fixture.path(), fixture.thread, &absent)
        .await
        .expect_err("a thread with no checkpoints cannot be reverted");

    match error {
        CheckpointError::Missing { thread, checkpoint } => {
            assert_eq!(thread, fixture.thread);
            assert_eq!(checkpoint, absent);
        }
        other => panic!("expected a missing-checkpoint refusal, got {other}"),
    }
    assert_eq!(
        fixture.tree_now().await,
        before,
        "a refused revert must not touch the worktree"
    );
}

#[tokio::test]
async fn a_checkpoint_identifier_no_daemon_wrote_never_reaches_git() {
    let fixture = Fixture::new().await;
    let head = fixture.head().await;

    for hostile in ["../../heads/main", "00001-turn-nope", ""] {
        let error = fixture
            .checkpoints
            .revert(
                fixture.path(),
                fixture.thread,
                &CheckpointId::from_parts(1, CheckpointScope::Turn, fixture.turn),
            )
            .await
            .expect_err("nothing is checkpointed yet");
        assert!(matches!(error, CheckpointError::Missing { .. }));

        let hostile: CheckpointId =
            serde_json::from_value(serde_json::Value::String(hostile.to_owned()))
                .unwrap_or_else(|error| panic!("{error}"));
        let error = fixture
            .checkpoints
            .revert(fixture.path(), fixture.thread, &hostile)
            .await
            .expect_err("a malformed identifier must be refused");
        assert!(
            matches!(error, CheckpointError::InvalidId(_)),
            "{hostile} produced {error}"
        );
    }
    assert_eq!(fixture.head().await, head);
    assert_eq!(fixture.git(&["branch", "--list"]).await.trim(), "* main");
}

#[tokio::test]
async fn checkpoint_refs_are_invisible_to_the_users_refs_and_are_never_pushed() {
    let fixture = Fixture::new().await;
    fixture
        .checkpoints
        .capture_turn(fixture.path(), fixture.thread, fixture.turn)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    fixture
        .checkpoints
        .capture_files(
            fixture.path(),
            fixture.thread,
            fixture.turn,
            &["a.txt".to_owned()],
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(fixture.git(&["branch", "--list"]).await.trim(), "* main");
    assert_eq!(fixture.git(&["branch", "--all"]).await.trim(), "* main");
    assert_eq!(fixture.git(&["tag", "--list"]).await.trim(), "");
    assert_eq!(
        fixture
            .git(&[
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/",
                "refs/tags/",
                "refs/remotes/",
            ])
            .await
            .trim(),
        "refs/heads/main"
    );
    assert_eq!(fixture.checkpoint_refs().await.len(), 2);

    // A default `git push` carries `refs/heads` and nothing else, which is the other half of
    // "hidden": a checkpoint of a user's half-finished worktree must never reach their remote.
    let remote = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    fixture
        .git_in(remote.path(), &["init", "--bare", "-b", "main"])
        .await;
    let url = remote.path().to_string_lossy().into_owned();
    fixture.git(&["remote", "add", "origin", &url]).await;
    fixture.git(&["push", "origin", "main"]).await;

    let remote_refs = fixture
        .git_in(remote.path(), &["for-each-ref", "--format=%(refname)"])
        .await;
    assert_eq!(remote_refs.trim(), "refs/heads/main");
}

#[tokio::test]
async fn garbage_collection_removes_every_ref_of_a_deleted_thread_and_no_other() {
    let fixture = Fixture::new().await;
    let survivor = ThreadId::new();
    let orphan = ThreadId::new();
    for thread in [fixture.thread, survivor, orphan] {
        for _ in 0..2 {
            fixture
                .checkpoints
                .capture_turn(fixture.path(), thread, TurnId::new())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    }
    assert_eq!(fixture.checkpoint_refs().await.len(), 6);

    // Deletion is per thread: the two refs of one thread go, and nothing else is considered.
    let deleted = fixture
        .checkpoints
        .forget_thread(fixture.path(), fixture.thread)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(deleted, 2);
    assert_eq!(fixture.checkpoint_refs().await.len(), 4);
    assert!(
        fixture
            .checkpoints
            .list(fixture.path(), fixture.thread)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .is_empty()
    );
    // Idempotent: a thread deleted twice is not an error.
    assert_eq!(
        fixture
            .checkpoints
            .forget_thread(fixture.path(), fixture.thread)
            .await
            .unwrap_or_else(|error| panic!("{error}")),
        0
    );

    // And the sweep finds what an interrupted deletion left behind, keeping every live thread.
    let live = HashSet::from([survivor]);
    let swept = fixture
        .checkpoints
        .retain(fixture.path(), &live)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(swept, 2, "the orphan's two refs");
    assert_eq!(
        fixture
            .checkpoints
            .list(fixture.path(), survivor)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .len(),
        2
    );
    assert!(
        fixture
            .checkpoints
            .list(fixture.path(), orphan)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .is_empty()
    );
}

#[tokio::test]
async fn a_ref_the_sweep_cannot_attribute_to_a_thread_is_left_alone() {
    let fixture = Fixture::new().await;
    let head = fixture.head().await;
    fixture
        .git(&["update-ref", "refs/fleet/something-else", &head])
        .await;

    let swept = fixture
        .checkpoints
        .retain(fixture.path(), &HashSet::new())
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(swept, 0);
    assert_eq!(
        fixture.checkpoint_refs().await,
        vec!["refs/fleet/something-else".to_owned()]
    );
}

#[tokio::test]
async fn a_file_checkpoint_restores_only_the_paths_it_covered() {
    let fixture = Fixture::new().await;
    // `brand-new.rs` does not exist yet: the checkpoint records its absence, and reverting has
    // to delete the file the edit went on to create.
    let checkpoint = fixture
        .checkpoints
        .capture_files(
            fixture.path(),
            fixture.thread,
            fixture.turn,
            &[
                "a.txt".to_owned(),
                format!("{}/brand-new.rs", fixture.path().display()),
            ],
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    fixture.write("a.txt", "the agent's version\n");
    fixture.write("brand-new.rs", "fn new() {}\n");
    fixture.write("sub/b.txt", "the agent touched this too\n");

    let report = fixture
        .checkpoints
        .revert(fixture.path(), fixture.thread, &checkpoint.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(checkpoint.scope, CheckpointScope::File);
    assert_eq!(report.restored, 1);
    assert_eq!(report.deleted, 1);
    assert_eq!(fixture.read("a.txt").as_deref(), Some("one\n"));
    assert!(!fixture.exists("brand-new.rs"));
    assert_eq!(
        fixture.read("sub/b.txt").as_deref(),
        Some("the agent touched this too\n"),
        "a file outside the checkpoint's scope is not the edit's to revert"
    );
}

#[tokio::test]
async fn a_file_checkpoint_whose_edit_never_ran_reverts_to_nothing() {
    let fixture = Fixture::new().await;
    let before = fixture.tree_now().await;
    let checkpoint = fixture
        .checkpoints
        .capture_files(
            fixture.path(),
            fixture.thread,
            fixture.turn,
            &["never-written.rs".to_owned()],
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    // The edit was refused, or the turn died before it ran: the path the checkpoint covers still
    // does not exist. `git add -- <pathspec>` fails the whole call on a pathspec it cannot
    // match, so this is the case that has to be narrowed before Git sees it.
    let report = fixture
        .checkpoints
        .revert(fixture.path(), fixture.thread, &checkpoint.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(report.is_noop(), "{report:?}");
    assert_eq!(fixture.tree_now().await, before);
}

#[tokio::test]
async fn a_path_that_looks_like_a_glob_is_treated_as_a_path() {
    let fixture = Fixture::new().await;
    fixture.write("report[1].txt", "the real file\n");
    fixture.write("reportX.txt", "a file the pattern would match\n");
    let checkpoint = fixture
        .checkpoints
        .capture_files(
            fixture.path(),
            fixture.thread,
            fixture.turn,
            &["report[1].txt".to_owned()],
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    fixture.write("report[1].txt", "the agent's version\n");
    fixture.write("reportX.txt", "the agent's version\n");
    let report = fixture
        .checkpoints
        .revert(fixture.path(), fixture.thread, &checkpoint.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(report.paths, vec!["report[1].txt".to_owned()]);
    assert_eq!(
        fixture.read("report[1].txt").as_deref(),
        Some("the real file\n")
    );
    assert_eq!(
        fixture.read("reportX.txt").as_deref(),
        Some("the agent's version\n"),
        "a character class in a filename must not widen the checkpoint's scope"
    );
}

#[tokio::test]
async fn ordinals_are_derived_from_the_refs_so_they_survive_a_restart() {
    let fixture = Fixture::new().await;
    let first = fixture
        .checkpoints
        .capture_turn(fixture.path(), fixture.thread, fixture.turn)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    // A second service with no memory of the first: the refs are the only state there is.
    let restarted = Checkpoints::new(Arc::new(IsolatedShell { inner: RealShell }));
    let second_turn = TurnId::new();
    let second = restarted
        .capture_turn(fixture.path(), fixture.thread, second_turn)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(first.ordinal, 1);
    assert_eq!(second.ordinal, 2);
    let listed = restarted
        .list(fixture.path(), fixture.thread)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        listed.iter().map(|entry| entry.ordinal).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(listed[1].turn, second_turn);
    assert_eq!(listed[0].id, first.id);
}

#[tokio::test]
async fn a_capture_refuses_a_scope_it_cannot_honour() {
    let fixture = Fixture::new().await;

    let empty = fixture
        .checkpoints
        .capture_files(fixture.path(), fixture.thread, fixture.turn, &[])
        .await
        .expect_err("an empty scope would capture an empty tree");
    assert!(matches!(empty, CheckpointError::EmptyScope { .. }));

    for hostile in ["../outside.rs", "/etc/passwd", "sub/../../escape.rs", ""] {
        let error = fixture
            .checkpoints
            .capture_files(
                fixture.path(),
                fixture.thread,
                fixture.turn,
                &[hostile.to_owned()],
            )
            .await
            .expect_err("a path outside the worktree must be refused");
        assert!(
            matches!(error, CheckpointError::PathOutsideWorktree { .. }),
            "{hostile} produced {error}"
        );
    }
    assert!(fixture.checkpoint_refs().await.is_empty());
}

#[tokio::test]
async fn a_worktree_that_is_no_longer_a_repository_refuses_a_capture_and_lists_nothing() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let checkpoints = Checkpoints::new(Arc::new(IsolatedShell { inner: RealShell }));
    let thread = ThreadId::new();

    let error = checkpoints
        .capture_turn(directory.path(), thread, TurnId::new())
        .await
        .expect_err("there is no repository to checkpoint");

    assert!(matches!(error, CheckpointError::NotAWorktree { .. }));
    // Listing answers "nothing to revert to" instead, because that is the true answer and it is
    // what keeps `[u]` from being drawn over a worktree that is gone.
    assert!(
        checkpoints
            .list(directory.path(), thread)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .is_empty()
    );
    assert_eq!(
        checkpoints
            .retain(directory.path(), &HashSet::new())
            .await
            .unwrap_or_else(|error| panic!("{error}")),
        0
    );
}
