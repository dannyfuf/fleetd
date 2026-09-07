use crate::{
    CommandKind, MergeOptions, MutationResult, ObjectId, Ref, Repository, ResetMode, Result,
};

impl Repository {
    /// Checks out a branch, tag, or commit quietly.
    pub async fn checkout(&self, reference: &Ref) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["checkout", "-q"])
            .arg(&reference.0)])
            .await
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
