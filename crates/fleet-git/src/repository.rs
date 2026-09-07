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
    CommandEvent, CommandRecord, GitError, RepoPaths, Result, Runner, command::GitCommand,
    model::CommandKind,
};

/// Git backend rooted at one non-bare worktree.
#[derive(Debug)]
pub struct Repository {
    pub(crate) paths: RepoPaths,
    pub(crate) runner: Arc<Runner>,
    pub(crate) mutation_lock: Mutex<()>,
    pub(crate) generation: AtomicU64,
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
        let command = GitCommand::new(&cwd, CommandKind::Read)
            .args([
                "rev-parse",
                "--show-toplevel",
                "--absolute-git-dir",
                "--path-format=absolute",
                "--git-common-dir",
                "--is-bare-repository",
            ])
            .foreground_read();
        let output = match runner.run(command).await {
            Ok(output) => output,
            Err(GitError::Exit { .. }) => return Err(GitError::NotARepository(supplied)),
            Err(error) => return Err(error),
        };
        let lines: Vec<&[u8]> = output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() != 4 || lines[3] == b"true" {
            return Err(GitError::NotARepository(supplied));
        }
        let worktree_root = PathBuf::from(String::from_utf8_lossy(lines[0]).into_owned());
        let git_dir = PathBuf::from(String::from_utf8_lossy(lines[1]).into_owned());
        let common_dir = PathBuf::from(String::from_utf8_lossy(lines[2]).into_owned());
        Ok(Self {
            paths: RepoPaths {
                worktree_root,
                git_dir,
                common_dir,
            },
            runner,
            mutation_lock: Mutex::new(()),
            generation: AtomicU64::new(0),
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
