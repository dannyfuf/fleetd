//! Mixed-host lifecycle request expansion and deterministic per-item response merging.

use std::collections::BTreeMap;

use fleet_core::{
    ids::{HostId, RepoId, WorktreeId},
    inspection::WorktreeInspection,
    sessions::SessionState,
};
use fleet_proto::{
    request::RequestBody,
    response::{PruneResult, PruneSkipped, ResponseBody, WorktreeDeleteResult},
};

use crate::{DaemonError, DaemonResult};

use super::{Resolver, Router};

impl Router {
    /// Expands an unscoped inspect or prune request into one explicit-id request per remote host.
    ///
    /// The local dispatcher must still execute the original unscoped request. Explicit ids on
    /// each remote part let an unavailable endpoint produce one failure for every known item.
    #[must_use]
    pub fn lifecycle_fanout(&self, body: &RequestBody) -> Option<Vec<(HostId, RequestBody)>> {
        let repo = match body {
            RequestBody::InspectWorktrees { ids, repo, .. } if ids.is_empty() => repo.as_ref(),
            RequestBody::PruneWorktrees {
                ids: None, repo, ..
            } => repo.as_ref(),
            _ => return None,
        };
        let mut by_host = BTreeMap::<HostId, Vec<WorktreeId>>::new();
        for worktree in self
            .mirror
            .worktrees()
            .into_iter()
            .filter(|worktree| repo.is_none_or(|repo| &worktree.repo_id == repo))
        {
            if let Some(host) = worktree.host {
                by_host.entry(host).or_default().push(worktree.id);
            }
        }
        if by_host.is_empty() {
            return None;
        }
        Some(
            by_host
                .into_iter()
                .map(|(host, ids)| {
                    let request = match body {
                        RequestBody::InspectWorktrees { repo, fetch, .. } => {
                            RequestBody::InspectWorktrees {
                                ids,
                                repo: repo.clone(),
                                fetch: *fetch,
                            }
                        }
                        RequestBody::PruneWorktrees {
                            dry_run,
                            fetch,
                            kill_sessions,
                            repo,
                            ..
                        } => RequestBody::PruneWorktrees {
                            dry_run: *dry_run,
                            fetch: *fetch,
                            kill_sessions: *kill_sessions,
                            repo: repo.clone(),
                            ids: Some(ids),
                        },
                        _ => unreachable!("lifecycle request was checked above"),
                    };
                    (host, request)
                })
                .collect(),
        )
    }

    /// Returns mirrored remote worktrees belonging to a repository.
    #[must_use]
    pub fn remote_worktrees_for_repo(&self, repo: &RepoId) -> Vec<WorktreeId> {
        self.mirror
            .worktrees()
            .into_iter()
            .filter(|worktree| &worktree.repo_id == repo)
            .map(|worktree| worktree.id)
            .collect()
    }

    /// Deletes known remote worktrees on their owning daemons and keeps every item outcome.
    pub async fn delete_remote_worktrees(
        &self,
        ids: Vec<WorktreeId>,
    ) -> DaemonResult<Vec<WorktreeDeleteResult>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let original = RequestBody::DeleteWorktrees { ids: ids.clone() };
        let mut by_host = BTreeMap::<HostId, Vec<WorktreeId>>::new();
        for id in ids {
            let host = self
                .host_of_worktree(&id)
                .ok_or_else(|| DaemonError::NotFound(format!("remote worktree {id}")))?;
            by_host.entry(host).or_default().push(id);
        }
        let parts = by_host
            .into_iter()
            .map(|(host, ids)| (host, RequestBody::DeleteWorktrees { ids }))
            .collect();
        match merge_lifecycle_fanout(&original, self.fanout(parts).await) {
            Some(Ok(ResponseBody::WorktreesDeleted(results))) => Ok(results),
            Some(Ok(other)) => Err(unexpected("delete results", &other)),
            Some(Err(error)) => Err(error),
            None => Err(DaemonError::Protocol(
                "delete lifecycle fanout was not mergeable".to_owned(),
            )),
        }
    }
}

pub(crate) fn merge_lifecycle_fanout(
    original: &RequestBody,
    parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> Option<DaemonResult<ResponseBody>> {
    match original {
        RequestBody::DeleteWorktrees { ids } => Some(merge_delete(ids, parts)),
        RequestBody::InspectWorktrees { ids, .. } => Some(merge_inspect(ids, parts)),
        RequestBody::PruneWorktrees { dry_run, ids, .. } => {
            Some(merge_prune(*dry_run, ids.as_deref(), parts))
        }
        _ => None,
    }
}

fn merge_delete(
    requested: &[WorktreeId],
    parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> DaemonResult<ResponseBody> {
    let mut outcomes = Vec::new();
    let mut failures = Vec::new();
    for (host, part) in parts {
        match part {
            Ok(ResponseBody::WorktreesDeleted(mut values)) => outcomes.append(&mut values),
            Ok(other) => return Err(unexpected("delete results", &other)),
            Err(error) => failures.push((host, error.to_string())),
        }
    }
    if requested.is_empty() {
        return Ok(ResponseBody::WorktreesDeleted(outcomes));
    }
    let mut merged = Vec::with_capacity(requested.len());
    for id in requested {
        if let Some(index) = outcomes
            .iter()
            .position(|outcome| &outcome.worktree_id == id)
        {
            merged.push(outcomes.remove(index));
        } else if let Some((host, reason)) = failures.first() {
            merged.push(WorktreeDeleteResult {
                worktree_id: id.clone(),
                ok: false,
                reason: Some(format!("host {host}: {reason}")),
                trash_entry: None,
            });
        } else {
            return Err(missing_outcome("delete", id));
        }
    }
    ensure_no_extra("delete", outcomes.len())?;
    Ok(ResponseBody::WorktreesDeleted(merged))
}

fn merge_inspect(
    requested: &[WorktreeId],
    parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> DaemonResult<ResponseBody> {
    let mut outcomes = Vec::new();
    let mut failures = Vec::new();
    for (host, part) in parts {
        match part {
            Ok(ResponseBody::Inspections(mut values)) => outcomes.append(&mut values),
            Ok(other) => return Err(unexpected("inspection results", &other)),
            Err(error) => failures.push((host, error.to_string())),
        }
    }
    if requested.is_empty() {
        return Ok(ResponseBody::Inspections(outcomes));
    }
    let mut merged = Vec::with_capacity(requested.len());
    for id in requested {
        if let Some(index) = outcomes
            .iter()
            .position(|outcome| &outcome.worktree_id == id)
        {
            merged.push(outcomes.remove(index));
        } else if let Some((host, reason)) = failures.first() {
            merged.push(unavailable_inspection(id, host, reason)?);
        } else {
            return Err(missing_outcome("inspection", id));
        }
    }
    ensure_no_extra("inspection", outcomes.len())?;
    Ok(ResponseBody::Inspections(merged))
}

fn merge_prune(
    dry_run: bool,
    requested: Option<&[WorktreeId]>,
    parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> DaemonResult<ResponseBody> {
    let mut deleted = Vec::new();
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    for (host, part) in parts {
        match part {
            Ok(ResponseBody::Pruned(mut result)) if result.dry_run == dry_run => {
                deleted.append(&mut result.deleted);
                skipped.append(&mut result.skipped);
            }
            Ok(ResponseBody::Pruned(result)) => {
                return Err(DaemonError::Protocol(format!(
                    "prune fanout changed dry-run from {dry_run} to {}",
                    result.dry_run
                )));
            }
            Ok(other) => return Err(unexpected("prune results", &other)),
            Err(error) => failures.push((host, error.to_string())),
        }
    }
    let Some(requested) = requested else {
        return Ok(ResponseBody::Pruned(PruneResult {
            dry_run,
            deleted,
            skipped,
        }));
    };
    let mut ordered_deleted = Vec::new();
    let mut ordered_skipped = Vec::new();
    for id in requested {
        if let Some(index) = deleted.iter().position(|outcome| outcome == id) {
            ordered_deleted.push(deleted.remove(index));
        } else if let Some(index) = skipped
            .iter()
            .position(|outcome| &outcome.worktree_id == id)
        {
            ordered_skipped.push(skipped.remove(index));
        } else if let Some((host, reason)) = failures.first() {
            ordered_skipped.push(PruneSkipped {
                worktree_id: id.clone(),
                reason: format!("host {host}: {reason}"),
                merged: false,
                dirty: false,
                unique_commits: None,
                running: Vec::new(),
            });
        } else {
            return Err(missing_outcome("prune", id));
        }
    }
    ensure_no_extra("prune", deleted.len() + skipped.len())?;
    Ok(ResponseBody::Pruned(PruneResult {
        dry_run,
        deleted: ordered_deleted,
        skipped: ordered_skipped,
    }))
}

fn unavailable_inspection(
    id: &WorktreeId,
    host: &HostId,
    reason: &str,
) -> DaemonResult<WorktreeInspection> {
    let repo_id = RepoId::try_from(id.repo())
        .map_err(|error| DaemonError::Protocol(format!("invalid worktree id {id}: {error}")))?;
    Ok(WorktreeInspection {
        repo_id,
        worktree_id: id.clone(),
        host: host.to_string(),
        path: String::new(),
        branch: String::new(),
        base_ref: String::new(),
        head: None,
        target_branch: String::new(),
        upstream: None,
        ahead: None,
        behind: None,
        upstream_gone: false,
        dirty: false,
        dirty_files: None,
        merged_into_target: false,
        unique_commits: None,
        published: false,
        merged: false,
        pr: None,
        session: SessionState::Unknown,
        running: Vec::new(),
        inspected_at: chrono::Utc::now().to_rfc3339(),
        warnings: Vec::new(),
        error: Some(reason.to_owned()),
    })
}

fn ensure_no_extra(operation: &str, extra: usize) -> DaemonResult<()> {
    if extra == 0 {
        Ok(())
    } else {
        Err(DaemonError::Protocol(format!(
            "{operation} fanout returned {extra} unrequested outcomes"
        )))
    }
}

fn missing_outcome(operation: &str, id: &WorktreeId) -> DaemonError {
    DaemonError::Protocol(format!("{operation} fanout omitted outcome for {id}"))
}

fn unexpected(expected: &str, actual: &ResponseBody) -> DaemonError {
    DaemonError::Protocol(format!(
        "lifecycle fanout expected {expected}, received {actual:?}"
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fleet_core::{config::default_config, model::Worktree};
    use fleet_proto::snapshot::{DaemonInfo, Snapshot};

    use crate::{DaemonError, machines::Machines, services::mirror::Mirror};

    use super::*;

    #[test]
    fn unscoped_inspect_expands_mirrored_items_by_host() {
        let mirror = Arc::new(Mirror::new());
        let first_host = host("alpha");
        let second_host = host("beta");
        mirror.apply(&first_host, snapshot(vec![worktree("one")]));
        mirror.apply(&second_host, snapshot(vec![worktree("two")]));
        let router = Router::new(
            Arc::new(Machines::from_config(&default_config("/tmp/fleet"))),
            mirror,
        );

        let parts = router
            .lifecycle_fanout(&RequestBody::InspectWorktrees {
                ids: Vec::new(),
                repo: Some(repo()),
                fetch: true,
            })
            .expect("remote fanout");

        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|(_, part)| {
            matches!(part, RequestBody::InspectWorktrees { ids, fetch: true, .. } if ids.len() == 1)
        }));
    }

    #[test]
    fn raw_endpoint_failure_is_synthesized_for_each_requested_item() {
        let first = id("one");
        let second = id("two");
        let request = RequestBody::DeleteWorktrees {
            ids: vec![first.clone(), second.clone()],
        };

        let response = merge_lifecycle_fanout(
            &request,
            vec![(
                host("down"),
                Err(DaemonError::Remote("link is down".to_owned())),
            )],
        )
        .expect("lifecycle request")
        .expect("per-item response");
        let ResponseBody::WorktreesDeleted(results) = response else {
            panic!("expected delete results");
        };
        assert_eq!(
            results
                .iter()
                .map(|result| (&result.worktree_id, result.ok))
                .collect::<Vec<_>>(),
            vec![(&first, false), (&second, false)]
        );
        assert!(results.iter().all(|result| {
            result
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("down"))
        }));
    }

    fn host(value: &str) -> HostId {
        value.parse().expect("host")
    }

    fn repo() -> RepoId {
        "acme/api".parse().expect("repo")
    }

    fn id(slug: &str) -> WorktreeId {
        format!("acme/api#{slug}").parse().expect("worktree")
    }

    fn worktree(slug: &str) -> Worktree {
        let id = id(slug);
        Worktree {
            repo_id: repo(),
            slug: slug.to_owned(),
            branch: slug.to_owned(),
            base_ref: "origin/main".to_owned(),
            path: format!("/srv/fleet/{slug}"),
            session: id.to_string(),
            host: None,
            created_at: "2026-09-08T00:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
            id,
        }
    }

    fn snapshot(worktrees: Vec<Worktree>) -> Snapshot {
        Snapshot {
            boards: Vec::new(),
            generated_at: "2026-09-08T00:00:00Z".to_owned(),
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees,
            active_context: None,
            sessions: Vec::new(),
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: DaemonInfo {
                version: "fleetd test".to_owned(),
                pid: 1,
                started_at: "2026-09-08T00:00:00Z".to_owned(),
                home: "/srv/fleet".to_owned(),
            },
        }
    }
}
