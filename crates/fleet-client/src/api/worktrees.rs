use super::{CreateWorktreeResult, Result, expect_ack, unexpected};
use crate::Client;
use fleet_core::{
    ids::{HostId, RepoId, WorktreeId},
    inspection::WorktreeInspection,
    model::RepoHooks,
    sessions::WorktreeStatus,
};
use fleet_proto::{
    request::RequestBody,
    response::{PruneResult, ResponseBody, SleepResult, WorktreeDeleteResult},
};

impl Client {
    /// Creates or idempotently returns a worktree.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_worktree(
        &self,
        repo: RepoId,
        slug: impl Into<String>,
        branch: Option<String>,
        base: Option<String>,
        host: Option<HostId>,
        hooks: RepoHooks,
    ) -> Result<CreateWorktreeResult> {
        let response = self
            .request(RequestBody::CreateWorktree {
                repo,
                slug: slug.into(),
                branch,
                base,
                host,
                hooks,
            })
            .await?;
        worktree_result("create_worktree", response)
    }

    /// Deletes worktrees while preserving individual outcomes.
    pub async fn delete_worktrees(
        &self,
        ids: Vec<WorktreeId>,
    ) -> Result<Vec<WorktreeDeleteResult>> {
        match self.request(RequestBody::DeleteWorktrees { ids }).await? {
            ResponseBody::WorktreesDeleted(results) => Ok(results),
            response => Err(unexpected("delete_worktrees", response)),
        }
    }

    /// Inspects selected or repository-filtered worktrees.
    pub async fn inspect_worktrees(
        &self,
        ids: Vec<WorktreeId>,
        repo: Option<RepoId>,
        fetch: bool,
    ) -> Result<Vec<WorktreeInspection>> {
        match self
            .request(RequestBody::InspectWorktrees { ids, repo, fetch })
            .await?
        {
            ResponseBody::Inspections(inspections) => Ok(inspections),
            response => Err(unexpected("inspect_worktrees", response)),
        }
    }

    /// Safely prunes merged worktrees.
    pub async fn prune_worktrees(
        &self,
        dry_run: bool,
        fetch: bool,
        kill_sessions: bool,
        repo: Option<RepoId>,
    ) -> Result<PruneResult> {
        match self
            .request(RequestBody::PruneWorktrees {
                dry_run,
                fetch,
                kill_sessions,
                repo,
                ids: None,
            })
            .await?
        {
            ResponseBody::Pruned(result) => Ok(result),
            response => Err(unexpected("prune_worktrees", response)),
        }
    }

    /// Hard-kills a worktree session.
    pub async fn kill_worktree(&self, id: WorktreeId) -> Result<()> {
        expect_ack(
            "kill_worktree",
            self.request(RequestBody::KillWorktree { id }).await?,
        )
    }

    /// Applies sleep policy to a worktree session.
    pub async fn sleep_worktree(&self, id: WorktreeId) -> Result<SleepResult> {
        match self.request(RequestBody::SleepWorktree { id }).await? {
            ResponseBody::Slept(result) => Ok(result),
            response => Err(unexpected("sleep_worktree", response)),
        }
    }

    /// Records that a worktree was opened.
    pub async fn touch_worktree_opened(&self, id: WorktreeId) -> Result<()> {
        expect_ack(
            "touch_worktree_opened",
            self.request(RequestBody::TouchWorktreeOpened { id })
                .await?,
        )
    }

    /// Resolves a local worktree's absolute path.
    pub async fn worktree_path(&self, id: WorktreeId) -> Result<String> {
        match self.request(RequestBody::WorktreePath { id }).await? {
            ResponseBody::Path { path, .. } => Ok(path),
            response => Err(unexpected("worktree_path", response)),
        }
    }

    /// Restores one recoverable trash entry.
    pub async fn restore_trash(&self, entry: impl Into<String>) -> Result<()> {
        expect_ack(
            "restore_trash",
            self.request(RequestBody::RestoreTrash {
                entry: entry.into(),
            })
            .await?,
        )
    }

    /// Refreshes runtime status for one repository or the complete fleet.
    pub async fn refresh_statuses(&self, repo: Option<RepoId>) -> Result<Vec<WorktreeStatus>> {
        match self.request(RequestBody::RefreshStatuses { repo }).await? {
            ResponseBody::Statuses(statuses) => Ok(statuses),
            response => Err(unexpected("refresh_statuses", response)),
        }
    }
}

fn worktree_result(operation: &str, response: ResponseBody) -> Result<CreateWorktreeResult> {
    match response {
        ResponseBody::Worktree {
            created,
            worktree,
            post_create_job,
        } => Ok(CreateWorktreeResult {
            created,
            worktree,
            post_create_job: post_create_job.map(|job| *job),
        }),
        response => Err(unexpected(operation, response)),
    }
}
