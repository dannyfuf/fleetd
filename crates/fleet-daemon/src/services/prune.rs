//! Safe stale-worktree pruning workflow.

use std::sync::Arc;

use async_trait::async_trait;
use fleet_core::{
    ids::{RepoId, WorktreeId},
    inspection::WorktreeInspection,
    sessions::SessionState,
};
use fleet_proto::{
    job::JobKind,
    response::{PruneResult, PruneSkipped},
};

use crate::{
    DaemonError, DaemonResult,
    jobs::{JobCtx, JobManager},
    services::{inspect::Inspect, worktrees::Worktrees},
    stores::state::StateStore,
};

/// Deletion boundary used after prune eligibility has been decided.
#[async_trait]
pub trait WorktreeDeleter: Send + Sync {
    /// Deletes one worktree, including its runtime session and on-disk copy.
    async fn delete(&self, id: WorktreeId) -> DaemonResult<()>;
}

#[async_trait]
impl WorktreeDeleter for Worktrees {
    async fn delete(&self, id: WorktreeId) -> DaemonResult<()> {
        let mut results = self.delete(vec![id.clone()]).await?;
        let result = results
            .pop()
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if result.ok {
            Ok(())
        } else {
            Err(DaemonError::Conflict(result.reason.unwrap_or_else(|| {
                format!("worktree {id} was not deleted")
            })))
        }
    }
}

#[derive(Clone)]
struct RegistryDeleter {
    state: Arc<StateStore>,
}

#[async_trait]
impl WorktreeDeleter for RegistryDeleter {
    async fn delete(&self, id: WorktreeId) -> DaemonResult<()> {
        self.state
            .transaction(move |state| {
                let before = state.worktrees.len();
                state.worktrees.retain(|worktree| worktree.id != id);
                if state.worktrees.len() == before {
                    return Err(DaemonError::NotFound(format!("worktree {id}")));
                }
                Ok(())
            })
            .await
    }
}

/// Safe-prune service using one inspection pass followed by sequential deletions.
#[derive(Clone)]
pub struct Prune {
    jobs: Arc<JobManager>,
    inspect: Inspect,
    deleter: Arc<dyn WorktreeDeleter>,
}

impl Prune {
    /// Creates a facade-compatible prune service.
    ///
    /// The frozen facade does not supply `Worktrees`; use [`Self::with_deleter`] when wiring the
    /// complete deletion service. This fallback still makes the registry mutation transactionally.
    #[must_use]
    pub fn new(state: Arc<StateStore>, jobs: Arc<JobManager>, inspect: Inspect) -> Self {
        Self {
            jobs,
            inspect,
            deleter: Arc::new(RegistryDeleter { state }),
        }
    }

    /// Creates a prune service using the full worktree-deletion boundary.
    #[must_use]
    pub fn with_deleter(
        jobs: Arc<JobManager>,
        inspect: Inspect,
        deleter: Arc<dyn WorktreeDeleter>,
    ) -> Self {
        Self {
            jobs,
            inspect,
            deleter,
        }
    }

    /// Prunes only merged, clean, inactive worktrees and reports every skip.
    pub async fn worktrees(
        &self,
        dry_run: bool,
        fetch: bool,
        kill_sessions: bool,
        repo: Option<RepoId>,
    ) -> DaemonResult<PruneResult> {
        let service = self.clone();
        let target = format!("prune-{}", uuid::Uuid::new_v4());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.jobs.submit(
            JobKind::Prune,
            target,
            "Prune worktrees",
            true,
            true,
            move |context| async move {
                let result = service
                    .prune_inner(dry_run, fetch, kill_sessions, repo, &context)
                    .await;
                let outcome = result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| DaemonError::Git(error.to_string()));
                let _ignored = sender.send(result);
                outcome
            },
        );
        receiver.await.map_err(|_| DaemonError::Cancelled)?
    }

    async fn prune_inner(
        &self,
        dry_run: bool,
        fetch: bool,
        kill_sessions: bool,
        repo: Option<RepoId>,
        context: &JobCtx,
    ) -> DaemonResult<PruneResult> {
        context.progress("inspecting prune candidates")?;
        let inspections = self
            .inspect
            .inspect_inner(Vec::new(), repo, fetch, context)
            .await?;
        let mut eligible = Vec::new();
        let mut skipped = Vec::new();
        for inspection in inspections {
            if let Some(reason) = skip_reason(&inspection, kill_sessions) {
                skipped.push(skipped_result(inspection, reason));
            } else {
                eligible.push(inspection);
            }
        }

        if dry_run {
            let deleted = eligible
                .into_iter()
                .map(|inspection| inspection.worktree_id)
                .collect::<Vec<_>>();
            context.progress(format!("{} worktrees eligible", deleted.len()))?;
            return Ok(PruneResult {
                dry_run,
                deleted,
                skipped,
            });
        }

        let mut deleted = Vec::new();
        for inspection in eligible {
            if context.cancel.is_cancelled() {
                return Err(DaemonError::Cancelled);
            }
            context.progress(format!("deleting {}", inspection.worktree_id))?;
            match self.deleter.delete(inspection.worktree_id.clone()).await {
                Ok(()) => deleted.push(inspection.worktree_id),
                Err(error) => skipped.push(skipped_result(inspection, error.to_string())),
            }
        }
        context.progress(format!("deleted {} worktrees", deleted.len()))?;
        Ok(PruneResult {
            dry_run,
            deleted,
            skipped,
        })
    }
}

fn skip_reason(inspection: &WorktreeInspection, kill_sessions: bool) -> Option<String> {
    if let Some(error) = &inspection.error {
        return Some(error.clone());
    }
    if inspection.dirty {
        return Some("uncommitted changes".to_owned());
    }
    match inspection.session {
        SessionState::Attached => return Some("session attached".to_owned()),
        SessionState::Unknown => return Some("status unknown".to_owned()),
        SessionState::None | SessionState::Detached => {}
    }
    let unique = inspection
        .unique_commits
        .ok_or_else(|| "unique commit count unavailable".to_owned())
        .err();
    if unique.is_some() {
        return unique;
    }
    if !inspection.merged {
        return Some(match inspection.unique_commits {
            Some(0) => "not merged".to_owned(),
            Some(1) => "1 unique commit, not merged".to_owned(),
            Some(count) => format!("{count} unique commits, not merged"),
            None => "unique commit count unavailable".to_owned(),
        });
    }
    if !kill_sessions && !inspection.running.is_empty() {
        return Some(format!(
            "session has running commands: {}",
            inspection.running.join(", ")
        ));
    }
    None
}

fn skipped_result(inspection: WorktreeInspection, reason: String) -> PruneSkipped {
    PruneSkipped {
        worktree_id: inspection.worktree_id,
        reason,
        merged: inspection.merged,
        dirty: inspection.dirty,
        unique_commits: inspection.unique_commits,
        running: inspection.running,
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        ids::{RepoId, WorktreeId},
        inspection::WorktreeInspection,
        sessions::SessionState,
    };

    use super::skip_reason;

    fn inspection() -> WorktreeInspection {
        WorktreeInspection {
            worktree_id: WorktreeId::try_from("acme/api#done")
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            host: "local".to_owned(),
            path: "/tmp/done".to_owned(),
            branch: "done".to_owned(),
            base_ref: "origin/main".to_owned(),
            head: Some("abc".to_owned()),
            target_branch: "main".to_owned(),
            upstream: Some("origin/done".to_owned()),
            ahead: Some(0),
            behind: Some(0),
            upstream_gone: false,
            dirty: false,
            dirty_files: Some(0),
            merged_into_target: true,
            unique_commits: Some(0),
            published: true,
            merged: true,
            pr: None,
            session: SessionState::None,
            running: Vec::new(),
            inspected_at: "2026-09-04T00:00:00Z".to_owned(),
            warnings: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn eligibility_fails_closed_in_documented_order() {
        let mut value = inspection();
        assert_eq!(skip_reason(&value, false), None);
        value.running = vec!["claude".to_owned(), ":3000".to_owned()];
        assert_eq!(
            skip_reason(&value, false).as_deref(),
            Some("session has running commands: claude, :3000")
        );
        assert_eq!(skip_reason(&value, true), None);
        value.session = SessionState::Attached;
        assert_eq!(
            skip_reason(&value, true).as_deref(),
            Some("session attached")
        );
        value.dirty = true;
        assert_eq!(
            skip_reason(&value, true).as_deref(),
            Some("uncommitted changes")
        );
    }
}
