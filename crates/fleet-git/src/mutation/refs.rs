use crate::{
    CommandKind, GitError, MergeOptions, MutationResult, ObjectId, Ref, Repository, ResetMode,
    Result,
};

impl Repository {
    /// Checks out a branch, tag, or commit quietly.
    pub async fn checkout(&self, reference: &Ref) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let resolved_name = if reference.0.starts_with("refs/") {
            reference.0.clone()
        } else {
            let local = format!("refs/heads/{}", reference.0);
            if self.exact_ref_exists(&local).await? {
                local
            } else {
                let mut candidates = Vec::new();
                for namespace in ["refs/tags", "refs/remotes"] {
                    let candidate = format!("{namespace}/{}", reference.0);
                    if self.exact_ref_exists(&candidate).await? {
                        candidates.push(candidate);
                    }
                }
                match candidates.len() {
                    0 => reference.0.clone(),
                    1 => candidates.remove(0),
                    _ => {
                        return Err(GitError::parse(
                            "checkout reference",
                            "short reference is ambiguous",
                        ));
                    }
                }
            }
        };
        let revision = format!("{resolved_name}^{{commit}}");
        let resolved = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-parse", "--verify", "--end-of-options"])
                    .arg(revision)
                    .foreground_read(),
            )
            .await?;
        let oid = String::from_utf8_lossy(&resolved.stdout).trim().to_owned();
        if oid.is_empty() {
            return Err(GitError::parse(
                "checkout reference",
                "reference did not resolve to a commit",
            ));
        }

        let local_name = resolved_name.strip_prefix("refs/heads/").map(str::to_owned);
        let command = if let Some(local_name) = local_name {
            self.command(CommandKind::Mutation)
                .args(["switch", "-q", "--no-guess", "--"])
                .arg(local_name)
        } else {
            self.command(CommandKind::Mutation)
                .args(["switch", "-q", "--detach"])
                .arg(oid)
        };
        let output = self.run_one(command).await?;
        Ok(super::result_from_outputs(vec![output]))
    }

    async fn exact_ref_exists(&self, reference: &str) -> Result<bool> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["show-ref", "--verify", "--quiet"])
                    .arg(reference)
                    .accept_exit_code(1)
                    .foreground_read(),
            )
            .await?;
        Ok(matches!(
            output.record.outcome,
            crate::CommandOutcome::Success {
                status: Some(0),
                ..
            }
        ))
    }

    /// Creates and checks out a branch from an optional start point.
    pub async fn checkout_new_branch(
        &self,
        name: &str,
        start_point: Option<&Ref>,
    ) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["checkout", "-q", "-b", name])
            .arg_opt(start_point.map(|start| start.0.as_str()))])
            .await
    }

    /// Creates a local branch without checking it out.
    pub async fn create_branch(
        &self,
        name: &str,
        start_point: Option<&Ref>,
    ) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["branch", name])
            .arg_opt(start_point.map(|start| start.0.as_str()))])
            .await
    }

    /// Deletes a local branch.
    pub async fn delete_branch(&self, name: &str, force: bool) -> Result<MutationResult> {
        self.run_commands([self.command(CommandKind::Mutation).args([
            "branch",
            if force { "-D" } else { "-d" },
            name,
        ])])
        .await
    }

    /// Renames a local branch.
    pub async fn rename_branch(&self, old: &str, new: &str) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["branch", "-m", old, new])])
            .await
    }

    /// Merges a ref into HEAD.
    pub async fn merge(&self, name: &Ref, options: MergeOptions) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .arg("merge")
            .env("GIT_EDITOR", "true")
            .may_conflict()
            .arg_if(options.ff_only, "--ff-only")
            .arg_if(options.no_ff, "--no-ff")
            .arg_if(options.squash, "--squash")
            .arg(&name.0)])
            .await
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
        self.run_commands([self
            .command(CommandKind::Mutation)
            .arg("cherry-pick")
            .env("GIT_EDITOR", "true")
            .may_conflict()
            .args(oids.iter().map(ObjectId::as_str))])
            .await
    }

    /// Continues an active cherry-pick.
    pub async fn cherry_pick_continue(&self) -> Result<MutationResult> {
        self.continue_command("cherry-pick", "--continue").await
    }

    /// Aborts an active cherry-pick.
    pub async fn cherry_pick_abort(&self) -> Result<MutationResult> {
        self.continue_command("cherry-pick", "--abort").await
    }

    /// Continues an active revert.
    pub async fn revert_continue(&self) -> Result<MutationResult> {
        self.continue_command("revert", "--continue").await
    }

    /// Aborts an active revert.
    pub async fn revert_abort(&self) -> Result<MutationResult> {
        self.continue_command("revert", "--abort").await
    }

    /// Reverts a commit, creating a new commit.
    pub async fn revert(&self, oid: &ObjectId) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .arg("revert")
            .arg(oid.as_str())
            .env("GIT_EDITOR", "true")
            .may_conflict()])
            .await
    }

    /// Resets HEAD with the requested index/worktree mode.
    pub async fn reset(&self, to: &Ref, mode: ResetMode) -> Result<MutationResult> {
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["reset", flag])
            .arg(&to.0)])
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
        self.run_commands([command.arg(name).arg(&target.0)]).await
    }

    /// Deletes a local tag.
    pub async fn delete_tag(&self, name: &str) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["tag", "-d", name])])
            .await
    }

    /// Creates and checks out a local branch tracking `remote/name`.
    pub async fn checkout_remote_branch(&self, remote: &str, name: &str) -> Result<MutationResult> {
        let tracking = format!("{remote}/{name}");
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["checkout", "-q", "-b", name, "--track", &tracking])])
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
        self.run_commands([self.command(CommandKind::Mutation).args([
            "branch",
            &format!("--set-upstream-to={upstream}"),
            branch,
        ])])
        .await
    }

    /// Removes a local branch's upstream configuration.
    pub async fn unset_upstream(&self, branch: &str) -> Result<MutationResult> {
        self.run_commands([self.command(CommandKind::Mutation).args([
            "branch",
            "--unset-upstream",
            branch,
        ])])
        .await
    }
}
