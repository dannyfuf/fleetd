//! Serialized index, worktree, ref, stash, and network mutations.

use std::path::{Path, PathBuf};

use crate::{
    ChangeKind, CommitOptions, ConflictChoice, FetchRequest, GitCommand, GitError, MergeOptions,
    MutationResult, ObjectId, PatchAction, PatchSelection, PullRequest, PushRequest, Ref,
    Repository, ResetMode, Result, StashOptions, model::CommandKind,
};

impl Repository {
    /// Stages literal repository-relative paths.
    pub async fn stage_paths(&self, paths: &[PathBuf]) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).arg("add").arg("--");
        for path in paths {
            command = command.arg(path);
        }
        self.run_commands(vec![command.literal_pathspecs()]).await
    }

    /// Unstages literal paths while retaining their worktree content.
    pub async fn unstage_paths(&self, paths: &[PathBuf]) -> Result<MutationResult> {
        let mut command = self
            .command(CommandKind::Mutation)
            .args(["reset", "-q", "--"]);
        for path in paths {
            command = command.arg(path);
        }
        self.run_commands(vec![command.literal_pathspecs()]).await
    }

    /// Stages all tracked and untracked changes.
    pub async fn stage_all(&self) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation).args(["add", "-A"]),
        ])
        .await
    }

    /// Unstages every index change.
    pub async fn unstage_all(&self) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation).args(["reset", "-q"]),
        ])
        .await
    }

    /// Discards all index/worktree changes for selected paths, deleting selected untracked paths.
    pub async fn discard_paths(&self, paths: &[PathBuf]) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let status_output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "status",
                        "--porcelain=v1",
                        "-z",
                        "--untracked-files=all",
                        "--find-renames",
                    ])
                    .foreground_read(),
            )
            .await?;
        let statuses = crate::parse::status::parse(&status_output.stdout)?;
        let mut records = vec![status_output.record];
        let mut reset = Vec::new();
        let mut checkout = Vec::new();
        let mut remove = Vec::new();
        for requested in paths {
            if let Some(status) = statuses.iter().find(|status| &status.path == requested) {
                if let Some(previous) = &status.previous_path {
                    reset.push(status.path.clone());
                    reset.push(previous.clone());
                    checkout.push(previous.clone());
                    remove.push(status.path.clone());
                } else {
                    if status.index != ChangeKind::Unmodified {
                        reset.push(status.path.clone());
                    }
                    if status.worktree == ChangeKind::Untracked || status.index == ChangeKind::Added
                    {
                        remove.push(status.path.clone());
                    } else {
                        checkout.push(status.path.clone());
                    }
                }
            } else {
                checkout.push(requested.clone());
            }
        }
        if !reset.is_empty() {
            let mut command = self
                .command(CommandKind::Mutation)
                .args(["reset", "-q", "HEAD", "--"]);
            for path in &reset {
                command = command.arg(path);
            }
            let output = self.run_one(command.literal_pathspecs()).await?;
            records.push(output.record);
        }
        if !checkout.is_empty() {
            let mut command = self.command(CommandKind::Mutation).args(["checkout", "--"]);
            for path in &checkout {
                command = command.arg(path);
            }
            let output = self.run_one(command.literal_pathspecs()).await?;
            records.push(output.record);
        }
        for path in remove {
            remove_worktree_path(&self.paths.worktree_root, &path).await?;
        }
        Ok(MutationResult {
            records,
            warning: None,
        })
    }

    /// Discards every tracked, staged, and untracked worktree change.
    pub async fn discard_all(&self) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args(["reset", "-q", "HEAD"]),
            self.command(CommandKind::Mutation)
                .args(["checkout", "--", "."])
                .literal_pathspecs(),
            self.command(CommandKind::Mutation).args(["clean", "-fd"]),
        ])
        .await
    }

    /// Applies selected hunks/lines to the index or worktree.
    pub async fn apply_patch_selection(
        &self,
        selection: PatchSelection,
        action: PatchAction,
    ) -> Result<MutationResult> {
        match (selection.side, action) {
            (crate::DiffSide::Unstaged, PatchAction::Stage | PatchAction::Discard)
            | (crate::DiffSide::Staged, PatchAction::Unstage) => {}
            _ => {
                return Err(GitError::parse(
                    "patch selection",
                    "action does not match the selected diff side",
                ));
            }
        }
        let _guard = self.mutation_lock.lock().await;
        let diff = self.diff_file(&selection.path, selection.side).await?;
        let reverse = matches!(action, PatchAction::Unstage | PatchAction::Discard);
        let patch = crate::patch::build(&diff, &selection.hunks, reverse)?;
        let mut command = self
            .command(CommandKind::Mutation)
            .args(["apply", "--whitespace=nowarn"]);
        match action {
            PatchAction::Stage => command = command.arg("--cached"),
            PatchAction::Unstage => command = command.args(["--reverse", "--cached"]),
            PatchAction::Discard => command = command.arg("--reverse"),
        }
        let output = self.run_one(command.stdin(patch)).await?;
        Ok(result_from_outputs(vec![output]))
    }

    /// Creates a commit with an explicit message.
    ///
    /// An **empty** `message` means "keep the message the commit already has": the command then
    /// runs with `--no-edit` instead of `-m ""`, because Git aborts a commit whose message is
    /// empty. That is the shape an amend-in-place needs (lazygit's `A`), so
    /// `commit("", CommitOptions { amend: true, .. })` rewrites HEAD with the current index and
    /// its original message.
    pub async fn commit(&self, message: &str, options: CommitOptions) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).arg("commit");
        if message.is_empty() {
            command = command.arg("--no-edit");
        } else {
            command = command.args(["-m", message]);
        }
        if options.amend {
            command = command.arg("--amend");
        }
        if options.allow_empty {
            command = command.arg("--allow-empty");
        }
        if options.signoff {
            command = command.arg("--signoff");
        }
        if options.no_verify {
            command = command.arg("--no-verify");
        }
        self.run_commands(vec![command]).await
    }

    /// Checks out a branch, tag, or commit quietly.
    pub async fn checkout(&self, reference: &Ref) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args(["checkout", "-q"])
                .arg(&reference.0),
        ])
        .await
    }

    /// Creates and checks out a branch from an optional start point.
    pub async fn checkout_new_branch(
        &self,
        name: &str,
        start_point: Option<&Ref>,
    ) -> Result<MutationResult> {
        let mut command = self
            .command(CommandKind::Mutation)
            .args(["checkout", "-q", "-b", name]);
        if let Some(start) = start_point {
            command = command.arg(&start.0);
        }
        self.run_commands(vec![command]).await
    }

    /// Creates a local branch without checking it out.
    pub async fn create_branch(
        &self,
        name: &str,
        start_point: Option<&Ref>,
    ) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).args(["branch", name]);
        if let Some(start) = start_point {
            command = command.arg(&start.0);
        }
        self.run_commands(vec![command]).await
    }

    /// Deletes a local branch.
    pub async fn delete_branch(&self, name: &str, force: bool) -> Result<MutationResult> {
        self.run_commands(vec![self.command(CommandKind::Mutation).args([
            "branch",
            if force { "-D" } else { "-d" },
            name,
        ])])
        .await
    }

    /// Renames a local branch.
    pub async fn rename_branch(&self, old: &str, new: &str) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args(["branch", "-m", old, new]),
        ])
        .await
    }

    /// Merges a ref into HEAD.
    pub async fn merge(&self, name: &Ref, options: MergeOptions) -> Result<MutationResult> {
        let mut command = self
            .command(CommandKind::Mutation)
            .arg("merge")
            .env("GIT_EDITOR", "true")
            .may_conflict();
        if options.ff_only {
            command = command.arg("--ff-only");
        }
        if options.no_ff {
            command = command.arg("--no-ff");
        }
        if options.squash {
            command = command.arg("--squash");
        }
        command = command.arg(&name.0);
        self.run_commands(vec![command]).await
    }

    /// Rebases HEAD onto a ref.
    pub async fn rebase_onto(&self, name: &Ref) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .arg("rebase")
                // lazygit always rebases with `--autostash`, so a dirty worktree does not turn a
                // rebase into "cannot rebase: You have unstaged changes".
                .arg("--autostash")
                .arg(&name.0)
                .env("GIT_EDITOR", "true")
                .may_conflict(),
        ])
        .await
    }

    /// Continues an active rebase.
    pub async fn rebase_continue(&self) -> Result<MutationResult> {
        self.continue_command("rebase", "--continue").await
    }
    /// Aborts an active rebase.
    pub async fn rebase_abort(&self) -> Result<MutationResult> {
        self.continue_command("rebase", "--abort").await
    }
    /// Skips the current rebase commit.
    pub async fn rebase_skip(&self) -> Result<MutationResult> {
        self.continue_command("rebase", "--skip").await
    }
    /// Aborts an active merge.
    pub async fn merge_abort(&self) -> Result<MutationResult> {
        self.continue_command("merge", "--abort").await
    }
    /// Continues an active merge.
    pub async fn merge_continue(&self) -> Result<MutationResult> {
        self.continue_command("merge", "--continue").await
    }

    /// Cherry-picks commits in the supplied order.
    pub async fn cherry_pick(&self, oids: &[ObjectId]) -> Result<MutationResult> {
        let mut command = self
            .command(CommandKind::Mutation)
            .arg("cherry-pick")
            .env("GIT_EDITOR", "true")
            .may_conflict();
        for oid in oids {
            command = command.arg(oid.as_str());
        }
        self.run_commands(vec![command]).await
    }
    /// Continues an active cherry-pick.
    pub async fn cherry_pick_continue(&self) -> Result<MutationResult> {
        self.continue_command("cherry-pick", "--continue").await
    }
    /// Aborts an active cherry-pick.
    pub async fn cherry_pick_abort(&self) -> Result<MutationResult> {
        self.continue_command("cherry-pick", "--abort").await
    }

    /// Reverts a commit, creating a new commit.
    pub async fn revert(&self, oid: &ObjectId) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .arg("revert")
                .arg(oid.as_str())
                .env("GIT_EDITOR", "true")
                .may_conflict(),
        ])
        .await
    }

    /// Resets HEAD with the requested index/worktree mode.
    pub async fn reset(&self, to: &Ref, mode: ResetMode) -> Result<MutationResult> {
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args(["reset", flag])
                .arg(&to.0),
        ])
        .await
    }

    /// Creates a lightweight or annotated tag.
    pub async fn create_tag(
        &self,
        name: &str,
        target: &Ref,
        message: Option<&str>,
    ) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).arg("tag");
        if let Some(message) = message {
            command = command.args(["-a", "-m", message]);
        }
        command = command.arg(name).arg(&target.0);
        self.run_commands(vec![command]).await
    }

    /// Deletes a local tag.
    pub async fn delete_tag(&self, name: &str) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args(["tag", "-d", name]),
        ])
        .await
    }

    /// Creates and checks out a local branch tracking `remote/name`.
    pub async fn checkout_remote_branch(&self, remote: &str, name: &str) -> Result<MutationResult> {
        let tracking = format!("{remote}/{name}");
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args(["checkout", "-q", "-b", name, "--track", &tracking]),
        ])
        .await
    }

    /// Assigns a local branch's upstream.
    pub async fn set_upstream(
        &self,
        branch: &str,
        remote: &str,
        remote_branch: &str,
    ) -> Result<MutationResult> {
        let upstream = format!("{remote}/{remote_branch}");
        self.run_commands(vec![self.command(CommandKind::Mutation).args([
            "branch",
            &format!("--set-upstream-to={upstream}"),
            branch,
        ])])
        .await
    }

    /// Removes a local branch's upstream configuration.
    pub async fn unset_upstream(&self, branch: &str) -> Result<MutationResult> {
        self.run_commands(vec![self.command(CommandKind::Mutation).args([
            "branch",
            "--unset-upstream",
            branch,
        ])])
        .await
    }

    /// Creates a stash.
    pub async fn stash_push(&self, options: StashOptions) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).args(["stash", "push"]);
        if let Some(message) = options.message {
            command = command.args(["-m", &message]);
        }
        if options.include_untracked {
            command = command.arg("--include-untracked");
        }
        if options.staged_only {
            command = command.arg("--staged");
        }
        if options.keep_index {
            command = command.arg("--keep-index");
        }
        self.run_commands(vec![command]).await
    }

    /// Applies a stash without dropping it.
    pub async fn stash_apply(&self, index: usize) -> Result<MutationResult> {
        self.stash_command("apply", index).await
    }
    /// Applies and drops a stash.
    pub async fn stash_pop(&self, index: usize) -> Result<MutationResult> {
        self.stash_command("pop", index).await
    }
    /// Drops a stash.
    pub async fn stash_drop(&self, index: usize) -> Result<MutationResult> {
        self.stash_command("drop", index).await
    }

    /// Creates a branch from a stash and applies it.
    pub async fn stash_branch(&self, name: &str, index: usize) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args(["stash", "branch", name, &format!("stash@{{{index}}}")])
                .may_conflict(),
        ])
        .await
    }

    /// Fetches refs without writing FETCH_HEAD.
    pub async fn fetch(&self, request: FetchRequest) -> Result<MutationResult> {
        let mut command = self
            .command(CommandKind::Network)
            .args(["fetch", "--no-write-fetch-head"]);
        if request.all {
            command = command.arg("--all");
        }
        if request.prune {
            command = command.arg("--prune");
        }
        if let Some(remote) = request.remote {
            command = command.arg(remote);
        }
        self.run_commands(vec![command]).await
    }

    /// Pulls and integrates a remote branch.
    pub async fn pull(&self, request: PullRequest) -> Result<MutationResult> {
        let mut command = self
            .command(CommandKind::Network)
            .args(["pull", "--no-edit"])
            .env("GIT_SEQUENCE_EDITOR", ":")
            .may_conflict();
        if request.rebase {
            command = command.arg("--rebase");
        }
        if request.ff_only {
            command = command.arg("--ff-only");
        }
        if let Some(remote) = request.remote {
            command = command.arg(remote);
        }
        if let Some(branch) = request.branch {
            command = command.arg(format!("refs/heads/{branch}"));
        }
        self.run_commands(vec![command]).await
    }

    /// Pushes a branch and/or tags.
    pub async fn push(&self, request: PushRequest) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Network).arg("push");
        if request.set_upstream {
            command = command.arg("--set-upstream");
        }
        if request.force_with_lease {
            command = command.arg("--force-with-lease");
        }
        if request.force {
            command = command.arg("--force");
        }
        if request.tags {
            command = command.arg("--tags");
        }
        if let Some(remote) = request.remote {
            command = command.arg(remote);
        }
        if let Some(branch) = request.branch {
            command = command.arg(format!("refs/heads/{branch}"));
        }
        self.run_commands(vec![command]).await
    }

    /// Replaces conflict regions according to `choice` and stages the file.
    pub async fn resolve_conflict(
        &self,
        path: &Path,
        choice: ConflictChoice,
    ) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let conflict = self.conflicted_file(path).await?;
        if conflict.conflicts.is_empty() {
            return Err(GitError::parse(
                "conflict resolution",
                "file contains no conflict markers",
            ));
        }
        let mut output = Vec::with_capacity(conflict.content.len());
        let mut cursor = 0;
        for section in &conflict.conflicts {
            output.extend_from_slice(&conflict.content[cursor..section.start]);
            match choice {
                ConflictChoice::Ours => output.extend_from_slice(&section.ours),
                ConflictChoice::Theirs => output.extend_from_slice(&section.theirs),
                ConflictChoice::Both => {
                    output.extend_from_slice(&section.ours);
                    output.extend_from_slice(&section.theirs);
                }
            }
            cursor = section.end;
        }
        output.extend_from_slice(&conflict.content[cursor..]);
        let absolute = self.paths.worktree_root.join(path);
        tokio::fs::write(&absolute, output)
            .await
            .map_err(|source| GitError::Spawn {
                argv: vec![format!("write {}", absolute.display())],
                source,
            })?;
        let command = self
            .command(CommandKind::Mutation)
            .args(["add", "--"])
            .arg(path)
            .literal_pathspecs();
        let applied = self.run_one(command).await?;
        Ok(result_from_outputs(vec![applied]))
    }

    async fn continue_command(&self, operation: &str, action: &str) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args([operation, action])
                .env("GIT_EDITOR", "true")
                .may_conflict(),
        ])
        .await
    }

    async fn stash_command(&self, action: &str, index: usize) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).args([
            "stash",
            action,
            &format!("stash@{{{index}}}"),
        ]);
        if action != "drop" {
            command = command.may_conflict();
        }
        self.run_commands(vec![command]).await
    }

    pub(crate) async fn run_commands(&self, commands: Vec<GitCommand>) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let mut outputs = Vec::new();
        for command in commands {
            outputs.push(self.run_one(command).await?);
        }
        Ok(result_from_outputs(outputs))
    }

    pub(crate) async fn run_one(&self, command: GitCommand) -> Result<crate::GitOutput> {
        let may_conflict = command.may_conflict;
        match self.runner.run(command).await {
            Err(GitError::Exit {
                status,
                stdout,
                stderr,
                argv,
                message,
            }) if may_conflict
                && (indicates_conflict(&stdout, &stderr)
                    || operation_sentinel_exists(&self.paths.git_dir)) =>
            {
                Err(GitError::Conflict {
                    status,
                    stdout,
                    stderr,
                    argv,
                    message,
                })
            }
            result => result,
        }
    }
}

fn result_from_outputs(outputs: Vec<crate::GitOutput>) -> MutationResult {
    let warning = outputs
        .iter()
        .flat_map(|output| [&output.stderr, &output.stdout])
        .find(|bytes| !bytes.is_empty())
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
        .filter(|text| !text.is_empty());
    MutationResult {
        records: outputs.into_iter().map(|output| output.record).collect(),
        warning,
    }
}

fn indicates_conflict(stdout: &[u8], stderr: &[u8]) -> bool {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_ascii_lowercase();
    text.contains("conflict") || text.contains("resolve all conflicts")
}

fn operation_sentinel_exists(git_dir: &Path) -> bool {
    [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ]
    .iter()
    .any(|name| git_dir.join(name).exists())
}

async fn remove_worktree_path(root: &Path, relative: &Path) -> Result<()> {
    let absolute = root.join(relative);
    let result = if absolute.is_dir() {
        tokio::fs::remove_dir_all(&absolute).await
    } else {
        tokio::fs::remove_file(&absolute).await
    };
    match result {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(GitError::Spawn {
            argv: vec![format!("remove {}", absolute.display())],
            source,
        }),
    }
}
