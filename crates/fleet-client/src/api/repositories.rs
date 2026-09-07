use super::{Result, expect_ack, unexpected};
use crate::Client;
use fleet_core::{
    github::PrTab,
    ids::{ContextId, RepoId},
    model::{Context, Repo, RepoHooks},
};
use fleet_proto::{
    job::JobRecord,
    request::RequestBody,
    response::{BaseRefs, PrSlice, ResponseBody},
};

impl Client {
    /// Creates a context.
    pub async fn create_context(
        &self,
        name: impl Into<String>,
        owners: Vec<String>,
    ) -> Result<Context> {
        match self
            .request(RequestBody::CreateContext {
                name: name.into(),
                owners,
            })
            .await?
        {
            ResponseBody::Context(context) => Ok(context),
            response => Err(unexpected("create_context", response)),
        }
    }

    /// Deletes a context and its descendants.
    pub async fn delete_context(&self, id: ContextId) -> Result<()> {
        expect_ack(
            "delete_context",
            self.request(RequestBody::DeleteContext { id }).await?,
        )
    }

    /// Starts cloning and registering a repository.
    pub async fn clone_repo(
        &self,
        owner: impl Into<String>,
        name: impl Into<String>,
        url: impl Into<String>,
        context: ContextId,
        default_branch: Option<String>,
    ) -> Result<JobRecord> {
        match self
            .request(RequestBody::CloneRepo {
                owner: owner.into(),
                name: name.into(),
                url: url.into(),
                context,
                default_branch,
            })
            .await?
        {
            ResponseBody::CloneStarted(job) => Ok(job),
            response => Err(unexpected("clone_repo", response)),
        }
    }

    /// Deletes a repository and all of its worktrees.
    pub async fn delete_repo(&self, repo: RepoId) -> Result<()> {
        expect_ack(
            "delete_repo",
            self.request(RequestBody::DeleteRepo { repo }).await?,
        )
    }

    /// Lists cached or freshly fetched base refs for worktree creation.
    pub async fn list_base_refs(&self, repo: RepoId, force: bool) -> Result<BaseRefs> {
        match self
            .request(RequestBody::ListBaseRefs { repo, force })
            .await?
        {
            ResponseBody::BaseRefs(refs) => Ok(refs),
            response => Err(unexpected("list_base_refs", response)),
        }
    }

    /// Replaces a repository's prepare and post-create hooks.
    pub async fn set_repo_hooks(&self, repo: RepoId, hooks: RepoHooks) -> Result<Repo> {
        match self
            .request(RequestBody::SetRepoHooks { repo, hooks })
            .await?
        {
            ResponseBody::Repo(repo) => Ok(repo),
            response => Err(unexpected("set_repo_hooks", response)),
        }
    }

    /// Dismisses a retained failed clone row.
    pub async fn dismiss_clone(&self, repo: RepoId) -> Result<()> {
        expect_ack(
            "dismiss_clone",
            self.request(RequestBody::DismissClone { repo }).await?,
        )
    }

    /// Lists pull requests for a repository or context.
    pub async fn list_pull_requests(
        &self,
        repo: Option<RepoId>,
        context: Option<ContextId>,
        tab: PrTab,
        force: bool,
    ) -> Result<Vec<PrSlice>> {
        match self
            .request(RequestBody::ListPullRequests {
                repo,
                context,
                tab,
                force,
            })
            .await?
        {
            ResponseBody::PullRequests(requests) => Ok(requests),
            response => Err(unexpected("list_pull_requests", response)),
        }
    }
}
