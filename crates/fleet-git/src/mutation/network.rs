use crate::{
    CommandKind, FetchRequest, MutationResult, PullRequest, PushRequest, Repository, Result,
};

impl Repository {
    /// Fetches refs without writing FETCH_HEAD.
    pub async fn fetch(&self, request: FetchRequest) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Network)
            .args(["fetch", "--no-write-fetch-head"])
            .arg_if(request.all, "--all")
            .arg_if(request.prune, "--prune")
            .arg_opt(request.remote)])
            .await
    }

    /// Pulls and integrates a remote branch.
    pub async fn pull(&self, request: PullRequest) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Network)
            .args(["pull", "--no-edit"])
            .env("GIT_SEQUENCE_EDITOR", ":")
            .may_conflict()
            .arg_if(request.rebase, "--rebase")
            .arg_if(request.ff_only, "--ff-only")
            .arg_opt(request.remote)
            .arg_opt(request.branch.map(|branch| format!("refs/heads/{branch}")))])
            .await
    }

    /// Pushes a branch and/or tags.
    pub async fn push(&self, request: PushRequest) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Network)
            .arg("push")
            .arg_if(request.set_upstream, "--set-upstream")
            .arg_if(request.force_with_lease, "--force-with-lease")
            .arg_if(request.force, "--force")
            .arg_if(request.tags, "--tags")
            .arg_opt(request.remote)
            .arg_opt(request.branch.map(|branch| format!("refs/heads/{branch}")))])
            .await
    }
}
