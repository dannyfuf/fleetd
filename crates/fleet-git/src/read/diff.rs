use crate::{
    ChangeKind, CommandKind, CommitFile, Diff, DiffSide, FileStatus, ObjectId, Ref, Repository,
    Result, command::GitCommand,
};
use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

const DIFF_ARGS: [&str; 4] = ["--no-color", "--no-ext-diff", "--patch", "--find-renames"];

impl Repository {
    /// [`DIFF_ARGS`] plus the `-U<n>` the repository is currently configured for.
    ///
    /// Every `diff` read goes through this, so widening the context with
    /// [`Repository::set_diff_context`] moves the display and the patches built from it together.
    fn diff_args(&self) -> [OsString; 5] {
        let [no_color, no_ext_diff, patch, find_renames] = DIFF_ARGS.map(OsString::from);
        let context = OsString::from(format!("-U{}", self.diff_context()));
        [no_color, no_ext_diff, patch, find_renames, context]
    }

    /// `git diff [--cached]` with the shared flags for one side of the index.
    fn diff_command(&self, side: DiffSide) -> GitCommand {
        self.command(CommandKind::Read)
            .arg("diff")
            .args(self.diff_args())
            .arg_if(side == DiffSide::Staged, "--cached")
    }

    /// Returns a parsed unstaged or staged file diff.
    pub async fn diff_file(&self, path: &Path, side: DiffSide) -> Result<Diff> {
        let status = if side == DiffSide::Unstaged {
            self.read_status().await?
        } else {
            Vec::new()
        };
        self.diff_file_with_status(path, side, &status).await
    }

    /// Reads a file diff using an already-loaded status list to identify untracked paths.
    ///
    /// `status` must describe this repository at the caller's refresh boundary. Call
    /// [`Self::diff_file`] when status freshness is unknown.
    pub async fn diff_file_with_status(
        &self,
        path: &Path,
        side: DiffSide,
        status: &[FileStatus],
    ) -> Result<Diff> {
        let untracked = side == DiffSide::Unstaged
            && status
                .iter()
                .any(|file| file.path == path && file.worktree == ChangeKind::Untracked);
        let command = self.diff_command(side);
        let command = if untracked {
            command
                .arg("--no-index")
                .paths([Path::new("/dev/null"), path])
                .accept_exit_code(1)
        } else {
            command.paths([path])
        };
        let output = self.runner.run(command).await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns one parsed diff covering several literal paths on one side.
    ///
    /// Tracked paths share one `git diff` call. Untracked paths follow them as
    /// individual `--no-index` patches.
    pub async fn diff_paths(&self, paths: &[PathBuf], side: DiffSide) -> Result<Diff> {
        if paths.is_empty() {
            return Ok(Diff::default());
        }
        let status = if side == DiffSide::Unstaged {
            self.read_status().await?
        } else {
            Vec::new()
        };
        self.diff_paths_with_status(paths, side, &status).await
    }

    /// Reads several paths using status from the same repository refresh boundary.
    ///
    /// As with [`Self::diff_file_with_status`], the caller owns status freshness.
    pub async fn diff_paths_with_status(
        &self,
        paths: &[PathBuf],
        side: DiffSide,
        status: &[FileStatus],
    ) -> Result<Diff> {
        let untracked: HashSet<&Path> = status
            .iter()
            .filter(|file| side == DiffSide::Unstaged && file.worktree == ChangeKind::Untracked)
            .map(|file| file.path.as_path())
            .collect();
        let mut diff = Diff::default();
        let tracked: Vec<&PathBuf> = paths
            .iter()
            .filter(|path| !untracked.contains(path.as_path()))
            .collect();
        if !tracked.is_empty() {
            let output = self
                .runner
                .run(self.diff_command(side).paths(tracked))
                .await?;
            diff.files
                .extend(crate::parse::diff::parse(&output.stdout)?.files);
        }
        for path in paths
            .iter()
            .filter(|path| untracked.contains(path.as_path()))
        {
            diff.files
                .extend(self.diff_file_with_status(path, side, status).await?.files);
        }
        Ok(diff)
    }

    /// Returns a parsed commit patch, optionally restricted to literal paths.
    pub async fn diff_commit(&self, oid: &ObjectId, paths: &[PathBuf]) -> Result<Diff> {
        let command = self
            .command(CommandKind::Read)
            .args(["show", "--format="])
            .args(self.diff_args())
            .arg(oid.as_str());
        let command = if paths.is_empty() {
            command
        } else {
            command.paths(paths)
        };
        let output = self.runner.run(command).await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns a parsed stash patch.
    pub async fn diff_stash(&self, index: usize) -> Result<Diff> {
        let stash = format!("stash@{{{index}}}");
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["stash", "show"])
                    .args(self.diff_args())
                    .arg(stash),
            )
            .await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns a parsed patch from `from` to `to`.
    pub async fn diff_range(&self, from: &Ref, to: &Ref) -> Result<Diff> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .arg("diff")
                    .args(self.diff_args())
                    .arg(&from.0)
                    .arg(&to.0),
            )
            .await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns the selected branch's changes since its merge-base with HEAD.
    pub async fn diff_branch(&self, name: &Ref) -> Result<Diff> {
        let base = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["merge-base", "HEAD"])
                    .arg(&name.0),
            )
            .await?;
        let base = String::from_utf8_lossy(&base.stdout).trim().to_owned();
        self.diff_range(&Ref(base), name).await
    }

    /// Lists files changed by one commit.
    pub async fn commit_files(&self, oid: &ObjectId) -> Result<Vec<CommitFile>> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "diff-tree",
                        "--root",
                        "--no-commit-id",
                        "--name-status",
                        "-r",
                        "-z",
                        "-M",
                    ])
                    .arg(oid.as_str()),
            )
            .await?;
        crate::parse::diff::commit_files(&output.stdout)
    }
}
