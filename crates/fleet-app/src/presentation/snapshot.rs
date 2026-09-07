use fleet_core::{
    ids::{HostId, RepoId, WorktreeId},
    model::{Repo, Worktree},
    sessions::{Session, SessionKind, WorktreeStatus},
};
use fleet_proto::{
    job::JobRecord,
    snapshot::{HostStatus, Snapshot},
};
use std::collections::HashMap;

/// Borrowed joins over a single snapshot. Build at a projection boundary and discard before
/// mutating the source. Duplicate keys retain the first record, matching slice `find` calls.
pub struct SnapshotIndex<'a> {
    repos: HashMap<&'a RepoId, &'a Repo>,
    worktrees: HashMap<&'a WorktreeId, &'a Worktree>,
    statuses: HashMap<&'a WorktreeId, &'a WorktreeStatus>,
    hosts: HashMap<&'a HostId, &'a HostStatus>,
    worktree_sessions: HashMap<&'a WorktreeId, Vec<&'a Session>>,
    jobs_by_target: HashMap<&'a str, Vec<&'a JobRecord>>,
}

impl<'a> SnapshotIndex<'a> {
    pub fn new(snapshot: &'a Snapshot) -> Self {
        let mut index = Self {
            repos: HashMap::with_capacity(snapshot.repos.len()),
            worktrees: HashMap::with_capacity(snapshot.worktrees.len()),
            statuses: HashMap::with_capacity(snapshot.statuses.len()),
            hosts: HashMap::with_capacity(snapshot.hosts.len()),
            worktree_sessions: HashMap::new(),
            jobs_by_target: HashMap::new(),
        };
        for repo in &snapshot.repos {
            index.repos.entry(&repo.id).or_insert(repo);
        }
        for worktree in &snapshot.worktrees {
            index.worktrees.entry(&worktree.id).or_insert(worktree);
        }
        for status in &snapshot.statuses {
            index.statuses.entry(&status.worktree_id).or_insert(status);
        }
        for host in &snapshot.hosts {
            index.hosts.entry(&host.id).or_insert(host);
        }
        for session in &snapshot.sessions {
            if let SessionKind::Worktree(id) = &session.kind {
                index.worktree_sessions.entry(id).or_default().push(session);
            }
        }
        for job in &snapshot.jobs {
            index
                .jobs_by_target
                .entry(&job.target)
                .or_default()
                .push(job);
        }
        index
    }

    pub fn repo(&self, id: &RepoId) -> Option<&'a Repo> {
        self.repos.get(id).copied()
    }
    pub fn worktree(&self, id: &WorktreeId) -> Option<&'a Worktree> {
        self.worktrees.get(id).copied()
    }
    pub fn status(&self, id: &WorktreeId) -> Option<&'a WorktreeStatus> {
        self.statuses.get(id).copied()
    }
    pub fn host(&self, id: &HostId) -> Option<&'a HostStatus> {
        self.hosts.get(id).copied()
    }
    pub fn sessions_for_worktree(&self, id: &WorktreeId) -> &[&'a Session] {
        self.worktree_sessions.get(id).map_or(&[], Vec::as_slice)
    }
    /// Exact wire-target match: normalization and active-status policy remain the caller's choice.
    pub fn jobs_for_target(&self, target: &str) -> &[&'a JobRecord] {
        self.jobs_by_target.get(target).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::{
        model::{Context, RepoHooks},
        sessions::{AgentActivity, SessionState},
    };
    use fleet_proto::snapshot::DaemonInfo;

    #[test]
    fn joins_borrow_original_records_and_keep_duplicate_order() {
        let context = Context {
            id: "work".try_into().expect("context"),
            name: "Work".into(),
            owners: vec![],
            created_at: String::new(),
        };
        let repo = Repo {
            id: "acme/api".try_into().expect("repo"),
            owner: "acme".into(),
            name: "api".into(),
            url: String::new(),
            context_id: context.id.clone(),
            default_branch: "main".into(),
            path: "/tmp/api".into(),
            cloned_at: String::new(),
            hooks: RepoHooks::default(),
        };
        let worktree = Worktree {
            id: "acme/api#feature".try_into().expect("worktree"),
            repo_id: repo.id.clone(),
            slug: "feature".into(),
            branch: "feature".into(),
            base_ref: "main".into(),
            path: "/tmp/api-feature".into(),
            session: "acme/api#feature".into(),
            host: None,
            created_at: String::new(),
            last_opened_at: None,
            degraded: None,
        };
        let session = Session {
            id: "acme/api#feature".try_into().expect("session"),
            kind: SessionKind::Worktree(worktree.id.clone()),
            cwd: worktree.path.clone(),
            terminals: vec![],
            active_terminal: None,
            slept_at: None,
            kept_terminals: vec![],
        };
        let status = WorktreeStatus {
            worktree_id: worktree.id.clone(),
            session: SessionState::Attached,
            windows: vec![],
            running: vec![],
            agent_activity: AgentActivity::Unknown,
            agent_activity_changed_at: None,
        };
        let mut duplicate = status.clone();
        duplicate.session = SessionState::Detached;
        let snapshot = Snapshot {
            generated_at: String::new(),
            contexts: vec![context],
            repos: vec![repo],
            clones: vec![],
            worktrees: vec![worktree],
            active_context: None,
            sessions: vec![session],
            statuses: vec![status, duplicate],
            pools: vec![],
            hosts: vec![],
            jobs: vec![],
            daemon: DaemonInfo {
                version: String::new(),
                pid: 1,
                started_at: String::new(),
                home: String::new(),
            },
        };
        let index = SnapshotIndex::new(&snapshot);
        let worktree = &snapshot.worktrees[0];
        assert!(std::ptr::eq(
            index.worktree(&worktree.id).expect("worktree"),
            worktree
        ));
        assert!(std::ptr::eq(
            index.repo(&worktree.repo_id).expect("repo"),
            &snapshot.repos[0]
        ));
        assert!(std::ptr::eq(
            index.status(&worktree.id).expect("status"),
            &snapshot.statuses[0]
        ));
        assert_eq!(
            index.sessions_for_worktree(&worktree.id),
            [&snapshot.sessions[0]]
        );
        assert!(index.jobs_for_target("missing").is_empty());
    }
}
