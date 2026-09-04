//! Worktree activity, process, and port inspection orchestration contracts.

use std::sync::Arc;

use fleet_core::{
    ids::{RepoId, WorktreeId},
    inspection::WorktreeInspection,
    model::Worktree,
    sessions::{SessionState, WorktreeStatus},
};

use crate::{
    DaemonError, DaemonResult,
    adapters::{git::Git, github::Github},
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Worktree inspection service coordinating Git, GitHub, process, and remote facts.
#[derive(Clone)]
pub struct Inspect {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
    _git: Arc<dyn Git>,
    _github: Arc<dyn Github>,
}

impl Inspect {
    /// Creates the inspection service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        github: Arc<dyn Github>,
    ) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
            _git: git,
            _github: github,
        }
    }

    /// Inspects selected worktrees, preserving fail-closed warning semantics (inventory sections 1, 6, and 7).
    pub async fn worktrees(
        &self,
        _ids: Vec<WorktreeId>,
        _repo: Option<RepoId>,
        _fetch: bool,
    ) -> DaemonResult<Vec<WorktreeInspection>> {
        Err(DaemonError::Unimplemented("inspect::worktrees"))
    }

    /// Refreshes runtime worktree statuses for one repository or all repositories.
    pub async fn refresh_statuses(
        &self,
        _repo: Option<RepoId>,
    ) -> DaemonResult<Vec<WorktreeStatus>> {
        Err(DaemonError::Unimplemented("inspect::refresh_statuses"))
    }

    /// Produces fail-closed status placeholders until the status poller has observed each worktree.
    #[must_use]
    pub fn unknown_statuses(worktrees: &[Worktree]) -> Vec<WorktreeStatus> {
        worktrees
            .iter()
            .map(|worktree| WorktreeStatus {
                worktree_id: worktree.id.clone(),
                session: SessionState::Unknown,
                windows: Vec::new(),
                running: Vec::new(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        ids::{RepoId, WorktreeId},
        model::Worktree,
        sessions::SessionState,
    };

    use super::Inspect;

    #[test]
    fn unobserved_worktrees_fail_closed() {
        let worktree = Worktree {
            id: WorktreeId::try_from("owner/repo#feature").unwrap(),
            repo_id: RepoId::try_from("owner/repo").unwrap(),
            slug: "feature".to_owned(),
            branch: "feature".to_owned(),
            base_ref: "main".to_owned(),
            path: "/tmp/feature".to_owned(),
            session: "fleet-owner-repo-feature".to_owned(),
            host: None,
            created_at: "2026-09-04T00:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        };

        let statuses = Inspect::unknown_statuses(&[worktree]);

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].session, SessionState::Unknown);
        assert!(statuses[0].windows.is_empty());
        assert!(statuses[0].running.is_empty());
    }
}
