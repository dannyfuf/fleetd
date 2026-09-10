//! Remote repository-ensure and worktree-create orchestration.

use std::time::Duration;

use fleet_core::{
    config::Config,
    ids::{HostId, WorktreeId},
    model::{CloneStatus, Context, Repo, Worktree},
    slug::normalize_context_id,
};
use fleet_proto::{request::RequestBody, response::ResponseBody, snapshot::Snapshot};

use crate::{DaemonError, DaemonResult};

use super::Router;

const CLONE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CLONE_TIMEOUT: Duration = Duration::from_secs(300);

impl Router {
    /// Applies daemon-side default placement to a create request whose host was omitted.
    #[must_use]
    pub fn place_create_on_default(&self, mut body: RequestBody, config: &Config) -> RequestBody {
        let Some(default_host) = config.default_host().cloned() else {
            return body;
        };
        match &mut body {
            RequestBody::CreateWorktree {
                host: host @ None, ..
            }
            | RequestBody::CreateWorktreeFromPr {
                host: host @ None, ..
            }
            | RequestBody::CreateWorktreeFromCard {
                host: host @ None, ..
            } => *host = Some(default_host),
            _ => {}
        }
        body
    }

    /// Ensures the local repository record exists on `host`, then creates its worktree there.
    ///
    /// The caller supplies its local worktree records because the router deliberately does not
    /// own durable local state. Remote records are checked through the mirror. Clone progress is
    /// mapped as soon as the `CloneStarted` response passes through `forward`, before subsequent
    /// remote events can be re-emitted by the router's event pump.
    pub async fn ensure_repo_then_create(
        &self,
        host: &HostId,
        context: &Context,
        repo: &Repo,
        local_worktrees: &[Worktree],
        create: RequestBody,
    ) -> DaemonResult<ResponseBody> {
        ensure_create_matches_repo(repo, &create)?;
        if let Some(id) = requested_worktree_id(&create)? {
            self.refuse_duplicate_on_another_host(host, &id, local_worktrees)?;
        }

        self.ensure_remote_context(host, context).await?;
        self.ensure_remote_repo(host, repo).await?;
        let response = self
            .forward(
                host,
                RequestBody::SetRepoHooks {
                    repo: repo.id.clone(),
                    hooks: repo.hooks.clone(),
                },
            )
            .await?;
        if !matches!(response, ResponseBody::Repo(_)) {
            return Err(unexpected_response("repository hooks", &response));
        }

        let response = self.forward(host, create).await?;
        if let ResponseBody::Worktree { worktree, .. } = &response {
            self.ids.register_worktree(host, worktree.id.clone());
        }
        Ok(response)
    }

    fn refuse_duplicate_on_another_host(
        &self,
        target: &HostId,
        id: &WorktreeId,
        local_worktrees: &[Worktree],
    ) -> DaemonResult<()> {
        let local_conflict = local_worktrees.iter().any(|worktree| {
            &worktree.id == id && worktree.host.as_ref().is_none_or(|host| host != target)
        });
        let remote_conflict = self.mirror.worktrees().iter().any(|worktree| {
            &worktree.id == id && worktree.host.as_ref().is_some_and(|host| host != target)
        });
        if local_conflict || remote_conflict {
            return Err(DaemonError::Conflict(format!(
                "worktree {id} already exists on another host"
            )));
        }
        Ok(())
    }

    /// Ensures the local context exists with matching display fields on `host`.
    ///
    /// `CreateContext` derives its id from `name` with [`normalize_context_id`]. Context ids are
    /// required to be canonical, so creating with the id as the name produces that exact id;
    /// `UpdateContext` then applies the display name. Do not simplify this to creating with
    /// `context.name`, because a renamed context's display name may normalize to a different id.
    async fn ensure_remote_context(&self, host: &HostId, context: &Context) -> DaemonResult<()> {
        let normalized_id = normalize_context_id(context.id.as_str());
        if normalized_id != context.id.as_str() {
            return Err(DaemonError::Validation(format!(
                "context id `{}` is not canonical; expected `{normalized_id}`",
                context.id
            )));
        }

        let snapshot = self.remote_snapshot(host).await?;
        let remote = match snapshot
            .contexts
            .into_iter()
            .find(|remote| remote.id == context.id)
        {
            Some(remote) => remote,
            None => {
                let create = self
                    .forward(
                        host,
                        RequestBody::CreateContext {
                            name: context.id.to_string(),
                            owners: context.owners.clone(),
                        },
                    )
                    .await;
                match create {
                    Ok(ResponseBody::Context(remote)) if remote.id == context.id => remote,
                    Ok(other) => return Err(unexpected_response("context ensure", &other)),
                    Err(error @ DaemonError::Conflict(_)) => {
                        let snapshot = self.remote_snapshot(host).await?;
                        snapshot
                            .contexts
                            .into_iter()
                            .find(|remote| remote.id == context.id)
                            .ok_or(error)?
                    }
                    Err(error) => return Err(error),
                }
            }
        };

        if remote.name == context.name && remote.owners == context.owners {
            return Ok(());
        }

        let response = self
            .forward(
                host,
                RequestBody::UpdateContext {
                    id: context.id.clone(),
                    name: Some(context.name.clone()),
                    owners: Some(context.owners.clone()),
                },
            )
            .await?;
        if !matches!(response, ResponseBody::Context(_)) {
            return Err(unexpected_response("context ensure", &response));
        }
        Ok(())
    }

    async fn ensure_remote_repo(&self, host: &HostId, repo: &Repo) -> DaemonResult<()> {
        let clone = self
            .forward(
                host,
                RequestBody::CloneRepo {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    url: repo.url.clone(),
                    context: repo.context_id.clone(),
                    default_branch: Some(repo.default_branch.clone()),
                },
            )
            .await;

        match clone {
            Ok(ResponseBody::CloneStarted(_)) => self.wait_for_remote_repo(host, repo).await,
            Ok(ResponseBody::Repo(remote)) if remote.id == repo.id => Ok(()),
            Ok(other) => Err(unexpected_response("repository ensure", &other)),
            Err(error @ DaemonError::Conflict(_)) => {
                let snapshot = self.remote_snapshot(host).await?;
                if snapshot.repos.iter().any(|remote| remote.id == repo.id) {
                    Ok(())
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn wait_for_remote_repo(&self, host: &HostId, repo: &Repo) -> DaemonResult<()> {
        tokio::time::timeout(CLONE_TIMEOUT, async {
            loop {
                let snapshot = self.remote_snapshot(host).await?;
                if snapshot.repos.iter().any(|remote| remote.id == repo.id) {
                    return Ok(());
                }
                if let Some(clone) = snapshot.clones.iter().find(|clone| clone.id == repo.id)
                    && clone.status == CloneStatus::Failed
                {
                    return Err(DaemonError::Git(clone.error.clone().unwrap_or_else(|| {
                        format!("repository clone failed for {}", repo.id)
                    })));
                }
                tokio::time::sleep(CLONE_POLL_INTERVAL).await;
            }
        })
        .await
        .map_err(|_| DaemonError::Timeout(format!("repository clone for {} on {host}", repo.id)))?
    }

    async fn remote_snapshot(&self, host: &HostId) -> DaemonResult<Snapshot> {
        match self.forward(host, RequestBody::GetSnapshot).await? {
            ResponseBody::Snapshot(snapshot) => Ok(snapshot),
            other => Err(unexpected_response("repository snapshot", &other)),
        }
    }
}

pub(crate) async fn ensure_repo_then_create(
    router: &Router,
    host: &HostId,
    context: &Context,
    repo: &Repo,
    local_worktrees: &[Worktree],
    create: RequestBody,
) -> DaemonResult<ResponseBody> {
    router
        .ensure_repo_then_create(host, context, repo, local_worktrees, create)
        .await
}

fn requested_worktree_id(create: &RequestBody) -> DaemonResult<Option<WorktreeId>> {
    let RequestBody::CreateWorktree { repo, slug, .. } = create else {
        return Ok(None);
    };
    WorktreeId::try_from(format!("{repo}#{slug}"))
        .map(Some)
        .map_err(|error| DaemonError::Validation(error.to_string()))
}

fn ensure_create_matches_repo(repo: &Repo, create: &RequestBody) -> DaemonResult<()> {
    let request_repo = match create {
        RequestBody::CreateWorktree { repo, .. }
        | RequestBody::CreateWorktreeFromPr { repo, .. } => repo,
        _ => {
            return Err(DaemonError::Protocol(
                "repo ensure requires a worktree create request".to_owned(),
            ));
        }
    };
    if request_repo != &repo.id {
        return Err(DaemonError::Validation(format!(
            "create repository {request_repo} does not match local repository {}",
            repo.id
        )));
    }
    Ok(())
}

fn unexpected_response(operation: &str, response: &ResponseBody) -> DaemonError {
    DaemonError::Protocol(format!(
        "remote {operation} returned unexpected response {response:?}"
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fleet_core::{
        config::default_config,
        ids::{ContextId, JobId, RepoId, SessionId},
        model::{Context, HostConfigEntry, RepoHooks},
    };
    use fleet_proto::{
        event::Event,
        job::{JobKind, JobRecord, JobStatus},
        snapshot::DaemonInfo,
    };

    use crate::{
        machines::Machines, server::BroadcastBus, services::mirror::Mirror, testing::FakeRemote,
    };

    use super::*;

    #[tokio::test]
    async fn ensures_repo_sets_hooks_then_creates_and_maps_progress() {
        let target = host("dev-box");
        let (router, remote) = router_with_remote(target.clone());
        let context = context();
        let repo = repo();
        let remote_job = job("job-clone");
        let mut made = worktree("feature", Some(target.clone()));
        made.session = format!("{target}/{}", made.session);
        remote.push_response(Ok(ResponseBody::Snapshot(snapshot(Vec::new(), Vec::new()))));
        remote.push_response(Ok(ResponseBody::CloneStarted(remote_job.clone())));
        remote.push_response(Ok(ResponseBody::Snapshot(snapshot(
            vec![repo.clone()],
            Vec::new(),
        ))));
        remote.push_response(Ok(ResponseBody::Repo(repo.clone())));
        remote.push_response(Ok(ResponseBody::Worktree {
            created: true,
            worktree: worktree("feature", None),
            post_create_job: None,
        }));

        let bus = BroadcastBus::default();
        let mut events = bus.subscribe();
        router.start_event_pumps(bus);
        let response = router
            .ensure_repo_then_create(
                &target,
                &context,
                &repo,
                &[],
                RequestBody::CreateWorktree {
                    repo: repo.id.clone(),
                    slug: "feature".to_owned(),
                    branch: Some("feature/remote".to_owned()),
                    base: Some("origin/main".to_owned()),
                    host: Some(target.clone()),
                    hooks: repo.hooks.clone(),
                },
            )
            .await
            .expect("remote create");

        assert!(matches!(
            response,
            ResponseBody::Worktree {
                created: true,
                worktree,
                ..
            } if worktree == made
        ));
        let requests = remote.requests();
        assert_eq!(requests.len(), 5, "{requests:#?}");
        assert!(matches!(requests[0], RequestBody::GetSnapshot));
        assert_eq!(
            requests[1],
            RequestBody::CloneRepo {
                owner: "acme".to_owned(),
                name: "api".to_owned(),
                url: "ssh://git.example/acme/api.git".to_owned(),
                context: context_id(),
                default_branch: Some("main".to_owned()),
            }
        );
        assert!(matches!(requests[2], RequestBody::GetSnapshot));
        assert_eq!(
            requests[3],
            RequestBody::SetRepoHooks {
                repo: repo.id.clone(),
                hooks: repo.hooks.clone(),
            }
        );
        assert!(matches!(
            &requests[4],
            RequestBody::CreateWorktree {
                host: None,
                hooks,
                ..
            } if hooks == &repo.hooks
        ));

        remote.emit(Event::JobUpdated(JobRecord {
            status: JobStatus::Running,
            progress: Some("receiving objects".to_owned()),
            ..remote_job.clone()
        }));
        let mapped = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Event::JobUpdated(job) = events.recv().await.expect("router event") {
                    break job;
                }
            }
        })
        .await
        .expect("mapped job progress");
        assert_ne!(mapped.id, remote_job.id);
        assert_eq!(
            router.ids.remote_job(&mapped.id),
            Some((target.clone(), remote_job.id))
        );
        assert_eq!(mapped.progress.as_deref(), Some("receiving objects"));
        assert_eq!(router.ids.host_of_worktree(&made.id), Some(target));
    }

    #[tokio::test]
    async fn duplicate_on_another_host_is_refused_before_remote_requests() {
        let target = host("dev-box");
        let other = host("build-box");
        let (router, remote) = router_with_remote(target.clone());
        router.mirror.apply(
            &other,
            snapshot(vec![repo()], vec![worktree("feature", None)]),
        );

        let error = router
            .ensure_repo_then_create(
                &target,
                &context(),
                &repo(),
                &[],
                RequestBody::CreateWorktree {
                    repo: repo_id(),
                    slug: "feature".to_owned(),
                    branch: None,
                    base: None,
                    host: Some(target.clone()),
                    hooks: RepoHooks::default(),
                },
            )
            .await
            .expect_err("duplicate must fail");

        assert!(
            matches!(error, DaemonError::Conflict(message) if message.contains("another host"))
        );
        assert!(remote.requests().is_empty());
    }

    #[test]
    fn omitted_host_uses_daemon_default_host() {
        let target = host("dev-box");
        let (router, _remote) = router_with_remote(target.clone());
        let mut config = default_config("/tmp/fleet-remote-create");
        config.default_host = target.to_string();
        config.hosts.insert(
            target.clone(),
            HostConfigEntry::Command {
                run: vec!["ssh".to_owned(), "dev-box".to_owned()],
                fleetd: "fleetd".to_owned(),
                fleet_home: None,
                display: None,
            },
        );

        let placed = router.place_create_on_default(
            RequestBody::CreateWorktree {
                repo: repo_id(),
                slug: "feature".to_owned(),
                branch: None,
                base: None,
                host: None,
                hooks: RepoHooks::default(),
            },
            &config,
        );
        assert!(matches!(
            placed,
            RequestBody::CreateWorktree { host: Some(host), .. } if host == target
        ));
    }

    fn router_with_remote(host: HostId) -> (Router, Arc<FakeRemote>) {
        let machines = Arc::new(Machines::from_config(&default_config(
            "/tmp/fleet-remote-create",
        )));
        let remote = Arc::new(FakeRemote::new(host.clone()));
        machines.install_endpoint(host, remote.clone());
        (Router::new(machines, Arc::new(Mirror::new())), remote)
    }

    fn snapshot(repos: Vec<Repo>, worktrees: Vec<Worktree>) -> Snapshot {
        Snapshot {
            boards: Vec::new(),
            generated_at: "2026-09-08T12:00:00Z".to_owned(),
            contexts: vec![context()],
            repos,
            clones: Vec::new(),
            worktrees,
            active_context: Some(context_id()),
            sessions: Vec::new(),
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: DaemonInfo {
                version: "fleetd test".to_owned(),
                pid: 1,
                started_at: "2026-09-08T12:00:00Z".to_owned(),
                home: "/tmp/remote".to_owned(),
            },
        }
    }

    fn context() -> Context {
        Context {
            id: context_id(),
            name: "Acme".to_owned(),
            owners: vec!["acme".to_owned()],
            created_at: "2026-09-08T12:00:00Z".to_owned(),
        }
    }

    fn repo() -> Repo {
        Repo {
            id: repo_id(),
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            url: "ssh://git.example/acme/api.git".to_owned(),
            context_id: context_id(),
            default_branch: "main".to_owned(),
            path: "/tmp/remote/repos/acme/api".to_owned(),
            cloned_at: "2026-09-08T12:00:00Z".to_owned(),
            hooks: RepoHooks {
                prepare: vec!["mise install".to_owned()],
                post_create: vec!["bin/setup".to_owned()],
            },
        }
    }

    fn worktree(slug: &str, host: Option<HostId>) -> Worktree {
        Worktree {
            id: WorktreeId::try_from(format!("acme/api#{slug}")).expect("worktree id"),
            repo_id: repo_id(),
            slug: slug.to_owned(),
            branch: "feature/remote".to_owned(),
            base_ref: "origin/main".to_owned(),
            path: format!("/tmp/remote/worktrees/acme/api/{slug}"),
            session: SessionId::local("api", slug).expect("session").to_string(),
            host,
            created_at: "2026-09-08T12:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        }
    }

    fn job(id: &str) -> JobRecord {
        JobRecord {
            id: JobId::try_from(id).expect("job id"),
            kind: JobKind::Clone,
            target: "acme/api".to_owned(),
            title: "Clone acme/api".to_owned(),
            status: JobStatus::Queued,
            progress: None,
            log_path: "/tmp/remote/logs/clone.log".to_owned(),
            started_at: "2026-09-08T12:00:00Z".to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: true,
        }
    }

    fn host(value: &str) -> HostId {
        value.parse().expect("host")
    }

    fn repo_id() -> RepoId {
        "acme/api".parse().expect("repo")
    }

    fn context_id() -> ContextId {
        "acme".parse().expect("context")
    }
}
