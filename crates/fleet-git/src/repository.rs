//! Repository discovery and shared repository state.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

use tokio::sync::{Mutex, broadcast};

use crate::{
    CommandEvent, CommandRecord, GitError, RepoPaths, Result, Runner,
    command::{GitCommand, GitOutput},
    model::CommandKind,
};

/// Git backend rooted at one non-bare worktree.
#[derive(Debug)]
pub struct Repository {
    pub(crate) paths: RepoPaths,
    pub(crate) runner: Arc<Runner>,
    pub(crate) mutation_lock: Mutex<()>,
    pub(crate) generation: AtomicU64,
    pub(crate) snapshot_invalidation: AtomicU64,
    pub(crate) diff_context: AtomicU32,
}

/// Git's own default number of context lines around a hunk.
pub const DEFAULT_DIFF_CONTEXT: u32 = 3;

/// The largest context a caller may ask for. Past this a "diff" is the whole file.
pub const MAX_DIFF_CONTEXT: u32 = 200;

impl Repository {
    /// Discovers a worktree with a default command runner.
    pub async fn discover(path: impl AsRef<Path>) -> Result<Self> {
        Self::discover_with_runner(path, Arc::new(Runner::default())).await
    }

    /// Discovers a worktree using an injected runner.
    pub async fn discover_with_runner(path: impl AsRef<Path>, runner: Arc<Runner>) -> Result<Self> {
        let supplied = path.as_ref().to_path_buf();
        let cwd = discovery_directory(&supplied).await;
        let worktree = discovery_value(&runner, &cwd, "--show-toplevel");
        let git_dir = discovery_value(&runner, &cwd, "--absolute-git-dir");
        let common_dir = discovery_value(&runner, &cwd, "--git-common-dir");
        let bare = discovery_value(&runner, &cwd, "--is-bare-repository");
        let (worktree, git_dir, common_dir, bare) =
            match tokio::try_join!(worktree, git_dir, common_dir, bare) {
                Ok(values) => values,
                Err(GitError::Exit { .. }) => return Err(GitError::NotARepository(supplied)),
                Err(error) => return Err(error),
            };
        if bare != b"false" {
            return Err(GitError::NotARepository(supplied));
        }
        Ok(Self {
            paths: RepoPaths {
                worktree_root: path_from_git(worktree)?,
                git_dir: path_from_git(git_dir)?,
                common_dir: path_from_git(common_dir)?,
            },
            runner,
            mutation_lock: Mutex::new(()),
            generation: AtomicU64::new(0),
            snapshot_invalidation: AtomicU64::new(0),
            diff_context: AtomicU32::new(DEFAULT_DIFF_CONTEXT),
        })
    }

    /// How many context lines every `diff` read currently asks git for.
    #[must_use]
    pub fn diff_context(&self) -> u32 {
        self.diff_context.load(Ordering::Relaxed)
    }

    /// Sets the context width used by every subsequent diff read, clamped to
    /// `0..=`[`MAX_DIFF_CONTEXT`].
    ///
    /// Display reads and partial-patch construction share this context width.
    pub fn set_diff_context(&self, lines: u32) {
        self.diff_context
            .store(lines.min(MAX_DIFF_CONTEXT), Ordering::Relaxed);
    }

    /// Returns resolved worktree, Git-dir, and common-dir paths.
    #[must_use]
    pub fn paths(&self) -> &RepoPaths {
        &self.paths
    }

    /// Subscribes to this repository's command events.
    #[must_use]
    pub fn subscribe_commands(&self) -> broadcast::Receiver<CommandEvent> {
        self.runner.subscribe()
    }

    /// Returns the runner's bounded command history.
    #[must_use]
    pub fn recent_commands(&self) -> Vec<CommandRecord> {
        self.runner.recent()
    }

    pub(crate) fn command(&self, kind: CommandKind) -> GitCommand {
        GitCommand::new(&self.paths.worktree_root, kind)
    }

    pub(crate) async fn run_optional(&self, command: GitCommand) -> Result<Option<GitOutput>> {
        match self.runner.run(command).await {
            Ok(output) => Ok(Some(output)),
            Err(GitError::Exit {
                status: Some(1),
                stdout,
                stderr,
                ..
            }) if stdout.is_empty() && stderr.is_empty() => Ok(None),
            Err(error) => Err(error),
        }
    }
}

async fn discovery_value(runner: &Runner, cwd: &Path, argument: &str) -> Result<Vec<u8>> {
    let output = runner
        .run(
            GitCommand::new(cwd, CommandKind::Read)
                .args(["rev-parse", "--path-format=absolute", argument])
                .foreground_read(),
        )
        .await?;
    let mut value = output.stdout;
    if value.last() == Some(&b'\n') {
        value.pop();
    }
    if value.is_empty() {
        return Err(GitError::parse(
            "repository discovery",
            "empty rev-parse output",
        ));
    }
    Ok(value)
}

fn path_from_git(bytes: Vec<u8>) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|error| GitError::parse("repository path", error.to_string()))
    }
}

async fn discovery_directory(path: &Path) -> PathBuf {
    if tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
    {
        path.parent().unwrap_or(path).to_path_buf()
    } else {
        path.to_path_buf()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{Repository, path_from_git};
    use crate::{CommandKind, RepoPaths, Runner};
    use std::os::unix::ffi::OsStrExt;
    use std::{path::PathBuf, sync::Arc, sync::atomic::Ordering};
    use tokio::sync::Mutex;

    #[test]
    fn non_utf8_discovery_value_is_preserved() {
        let path = path_from_git(b"/repo/\xff".to_vec()).unwrap();
        assert_eq!(path.as_os_str().as_bytes(), b"/repo/\xff");
    }

    #[test]
    fn constructing_mutation_command_does_not_invalidate_snapshot() {
        let repository = Repository {
            paths: RepoPaths {
                worktree_root: PathBuf::from("/repo"),
                git_dir: PathBuf::from("/repo/.git"),
                common_dir: PathBuf::from("/repo/.git"),
            },
            runner: Arc::new(Runner::default()),
            mutation_lock: Mutex::new(()),
            generation: std::sync::atomic::AtomicU64::new(0),
            snapshot_invalidation: std::sync::atomic::AtomicU64::new(0),
            diff_context: std::sync::atomic::AtomicU32::new(super::DEFAULT_DIFF_CONTEXT),
        };

        let _command = repository.command(CommandKind::Mutation);

        assert_eq!(repository.snapshot_invalidation.load(Ordering::Acquire), 0);
    }
}
