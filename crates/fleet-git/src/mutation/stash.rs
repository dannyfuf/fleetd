use crate::{CommandKind, MutationResult, Repository, Result, StashOptions};

impl Repository {
    /// Creates a stash.
    pub async fn stash_push(&self, options: StashOptions) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).args(["stash", "push"]);
        if let Some(message) = options.message {
            command = command.args(["-m", &message]);
        }
        self.run_commands([command
            .arg_if(options.include_untracked, "--include-untracked")
            .arg_if(options.staged_only, "--staged")
            .arg_if(options.keep_index, "--keep-index")])
            .await
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
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["stash", "branch", name, &format!("stash@{{{index}}}")])
            .may_conflict()])
            .await
    }

    /// `git stash <action> stash@{index}`; every action but `drop` can conflict.
    async fn stash_command(&self, action: &str, index: usize) -> Result<MutationResult> {
        let mut command = self.command(CommandKind::Mutation).args([
            "stash",
            action,
            &format!("stash@{{{index}}}"),
        ]);
        if action != "drop" {
            command = command.may_conflict();
        }
        self.run_commands([command]).await
    }
}
