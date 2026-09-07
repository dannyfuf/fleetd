use fleet_core::ids::{JobId, RepoId, WorktreeId};

use crate::state::{AppState, HubPane, HubTab, RepoScope, Screen};

/// Stable identities from the exact rows currently prepared for the Hub.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisplayedHub {
    pub repos: Vec<DisplayedRepo>,
    pub repo_total: usize,
    pub worktrees: Vec<DisplayedWorktree>,
    pub worktree_total: usize,
    pub prs: Vec<DisplayedPr>,
    pub pr_total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayedRepo {
    pub kind: DisplayedRepoKind,
    pub repo: Option<RepoId>,
    pub job: Option<JobId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayedRepoKind {
    All,
    Repo,
    Cloning,
    CloneFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayedWorktree {
    pub id: WorktreeId,
    pub repo: RepoId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayedPr {
    pub repo: RepoId,
    pub number: u64,
    pub local: Option<WorktreeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayedTarget {
    AllRepos,
    Repo(RepoId),
    CloneFailed { repo: RepoId, job: Option<JobId> },
    Worktree(WorktreeId),
    PullRequest(DisplayedPr),
}

#[must_use]
pub fn filter_counts(state: &AppState) -> (usize, usize) {
    match (state.hub_pane, &state.screen) {
        (HubPane::Repos, _) => (
            state.displayed_hub.repos.len(),
            state.displayed_hub.repo_total,
        ),
        (
            HubPane::List,
            Screen::Hub {
                tab: HubTab::Worktrees,
            },
        ) => (
            state.displayed_hub.worktrees.len(),
            state.displayed_hub.worktree_total,
        ),
        (HubPane::List, Screen::Hub { tab: HubTab::Prs }) => {
            (state.displayed_hub.prs.len(), state.displayed_hub.pr_total)
        }
        (HubPane::List, Screen::Workspace { .. }) => (0, 0),
    }
}

#[must_use]
pub fn filter_target(state: &AppState) -> Option<DisplayedTarget> {
    match (state.hub_pane, &state.screen) {
        (HubPane::Repos, _) => state
            .displayed_hub
            .repos
            .get(state.cursors.repos)
            .cloned()
            .and_then(|row| match (row.kind, row.repo) {
                (DisplayedRepoKind::All, _) => Some(DisplayedTarget::AllRepos),
                (DisplayedRepoKind::CloneFailed, Some(repo)) => {
                    Some(DisplayedTarget::CloneFailed { repo, job: row.job })
                }
                (_, Some(repo)) => Some(DisplayedTarget::Repo(repo)),
                (_, None) => None,
            }),
        (
            HubPane::List,
            Screen::Hub {
                tab: HubTab::Worktrees,
            },
        ) => state
            .displayed_hub
            .worktrees
            .get(state.cursors.worktrees)
            .map(|row| DisplayedTarget::Worktree(row.id.clone())),
        (HubPane::List, Screen::Hub { tab: HubTab::Prs }) => {
            let cursor = match state.pr_tab {
                fleet_core::github::PrTab::Mine => state.cursors.prs_mine,
                fleet_core::github::PrTab::Review => state.cursors.prs_review,
            };
            state
                .displayed_hub
                .prs
                .get(cursor)
                .cloned()
                .map(DisplayedTarget::PullRequest)
        }
        (HubPane::List, Screen::Workspace { .. }) => None,
    }
}

#[must_use]
pub fn selected_worktree_id(state: &AppState) -> Option<WorktreeId> {
    if let Screen::Workspace { .. } = state.screen {
        return state
            .active_session()
            .and_then(|session| match &session.kind {
                fleet_core::sessions::SessionKind::Worktree(id) => Some(id.clone()),
                fleet_core::sessions::SessionKind::Agent { .. } => None,
            });
    }
    match filter_target(state)? {
        DisplayedTarget::Worktree(id) => Some(id),
        DisplayedTarget::PullRequest(row) => row.local,
        DisplayedTarget::AllRepos
        | DisplayedTarget::Repo(_)
        | DisplayedTarget::CloneFailed { .. } => None,
    }
}

#[must_use]
pub fn selected_repo_id(state: &AppState) -> Option<RepoId> {
    if let Screen::Workspace { .. } = state.screen {
        let id = selected_worktree_id(state)?;
        return state
            .snapshot
            .as_ref()?
            .worktrees
            .iter()
            .find_map(|worktree| (worktree.id == id).then(|| worktree.repo_id.clone()));
    }
    match filter_target(state) {
        Some(DisplayedTarget::Repo(repo)) => Some(repo),
        Some(DisplayedTarget::CloneFailed { repo, .. }) => Some(repo),
        Some(DisplayedTarget::Worktree(id)) => state
            .displayed_hub
            .worktrees
            .iter()
            .find_map(|row| (row.id == id).then(|| row.repo.clone())),
        Some(DisplayedTarget::PullRequest(row)) => Some(row.repo),
        Some(DisplayedTarget::AllRepos) => state
            .displayed_hub
            .worktrees
            .get(state.cursors.worktrees)
            .map(|row| row.repo.clone())
            .or_else(|| match &state.scope {
                RepoScope::Repo(repo) => Some(repo.clone()),
                RepoScope::All => None,
            }),
        None => match &state.scope {
            RepoScope::Repo(repo) => Some(repo.clone()),
            RepoScope::All => None,
        },
    }
}
