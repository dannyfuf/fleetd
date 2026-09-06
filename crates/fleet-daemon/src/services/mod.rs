//! Fleet application services orchestrating domain rules, stores, adapters, jobs, and runtime snapshots.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use fleet_core::{ids::RepoId, paths::slot_path, sessions::WorktreeStatus};
use fleet_proto::{
    request::RequestBody,
    response::ResponseBody,
    snapshot::{DaemonInfo, PoolStatus, Snapshot},
};
use tokio::{sync::RwLock, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    adapters::Adapters,
    jobs::JobManager,
    server::BroadcastBus,
    stores::{config::ConfigStore, state::StateStore},
};

pub mod contexts;
pub mod doctor;
pub mod github;
pub mod hosts;
pub mod import;
pub mod inspect;
pub mod pool;
pub mod prune;
pub mod repos;
pub mod sessions;
pub mod sleep;
pub mod update;
mod watch_discovery;
pub mod watches;
pub mod worktrees;

use contexts::Contexts;
use doctor::Doctor;
use github::Github;
use hosts::Hosts;
use import::{Import, ImportNotifier};
use inspect::Inspect;
use pool::Pool;
use prune::Prune;
use repos::Repos;
use sessions::Sessions;
use sleep::Sleep;
use update::Update;
use worktrees::Worktrees;

/// Fully wired facade used by socket connection actors.
#[derive(Clone)]
pub struct Services {
    /// Effective configuration store.
    pub config: Arc<ConfigStore>,
    /// Validated state store.
    pub state: Arc<StateStore>,
    /// Detached background job manager.
    pub jobs: Arc<JobManager>,
    /// External-system dependency bundle shared by services.
    pub adapters: Adapters,
    /// Context domain service.
    pub contexts: Contexts,
    /// Repository domain service.
    pub repos: Repos,
    /// Worktree domain service.
    pub worktrees: Worktrees,
    /// Prepared-copy pool service.
    pub pool: Pool,
    /// GitHub and pull-request service.
    pub github: Github,
    /// Remote-host reachability cache.
    pub hosts: Hosts,
    /// Runtime PTY session service.
    pub sessions: Sessions,
    /// Cooperative and discovered child output and lifecycle registry.
    pub watches: watches::Watches,
    watch_discovery: watch_discovery::WatchDiscovery,
    /// Session sleep-policy service.
    pub sleep: Sleep,
    /// Worktree inspection service.
    pub inspect: Inspect,
    /// Safe prune service.
    pub prune: Prune,
    /// Environment diagnostics and updater service.
    pub doctor: Doctor,
    /// Compatible swarm-state import service.
    pub import: Import,
    /// Fleet source-checkout update service.
    pub update: Update,
    home: PathBuf,
    started_at: String,
    statuses: Arc<RwLock<Option<Vec<WorktreeStatus>>>>,
    pool_refreshed_at: Arc<RwLock<BTreeMap<RepoId, String>>>,
    /// Daemon-wide event bus shared by every service integration.
    pub events: BroadcastBus,
}

struct BusImportNotifier {
    events: BroadcastBus,
}

#[async_trait::async_trait]
impl ImportNotifier for BusImportNotifier {
    async fn snapshot_changed(&self) -> DaemonResult<()> {
        self.events.request_snapshot_current();
        Ok(())
    }
}

/// Owned periodic daemon tasks, joined during graceful shutdown.
pub struct PeriodicTasks {
    handles: Vec<JoinHandle<()>>,
}

impl PeriodicTasks {
    /// Waits briefly for cancellation-aware periodic loops to finish.
    pub async fn join(mut self) {
        for mut handle in self.handles.drain(..) {
            if tokio::time::timeout(Duration::from_secs(2), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
    }
}

impl Drop for PeriodicTasks {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

impl Services {
    /// Composes every service around shared stores and job resources.
    #[must_use]
    pub fn new(
        home: impl Into<PathBuf>,
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        adapters: Adapters,
    ) -> Self {
        Self::build(
            home.into(),
            config,
            state,
            jobs,
            adapters,
            BroadcastBus::default(),
        )
    }

    /// Composes services around the daemon-wide event bus and attaches snapshot assembly.
    #[must_use]
    pub fn new_with_events(
        home: impl Into<PathBuf>,
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        adapters: Adapters,
        events: BroadcastBus,
    ) -> Arc<Self> {
        let services = Arc::new(Self::build(
            home.into(),
            config,
            state,
            jobs,
            adapters,
            events.clone(),
        ));
        events.attach_services(Arc::downgrade(&services));
        services
    }

    fn build(
        home: PathBuf,
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        adapters: Adapters,
        events: BroadcastBus,
    ) -> Self {
        let repos = Repos::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.github),
            Arc::clone(&adapters.files),
        )
        .with_process(Arc::clone(&adapters.process));
        let sessions =
            Sessions::new(Arc::clone(&config), Arc::clone(&state)).with_events(events.clone());
        let worktrees = Worktrees::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.files),
            Arc::clone(&adapters.git),
        )
        .with_integrations(sessions.clone(), Arc::clone(&adapters.github))
        .with_shell(Arc::clone(&adapters.shell));
        let pool = Pool::without_background(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.files),
        )
        .with_shell(Arc::clone(&adapters.shell));
        let github = Github::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.github),
            Arc::clone(&adapters.files),
        );
        let sleep = Sleep::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&adapters.process),
        );
        let inspect = Inspect::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.github),
        )
        .with_sessions(sessions.clone());
        let prune = Prune::with_deleter(
            Arc::clone(&jobs),
            inspect.clone(),
            Arc::new(worktrees.clone()),
        );
        let doctor = Doctor::new(
            Arc::clone(&jobs),
            Arc::clone(&config),
            Arc::clone(&adapters.shell),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.github),
            Arc::clone(&adapters.files),
        );
        let swarm_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(".swarm");
        let import = Import::new(
            home.clone(),
            swarm_home,
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.files),
        )
        .with_notifier(Arc::new(BusImportNotifier {
            events: events.clone(),
        }));
        let update = Update::new(
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.shell),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR"))),
        );
        let hosts = Hosts::new(home.clone(), Arc::clone(&adapters.shell));
        let watches = sessions.watches();
        let watch_discovery = watch_discovery::WatchDiscovery::new(
            Arc::clone(&config),
            sessions.clone(),
            Arc::clone(&adapters.process),
            watches.clone(),
        );
        Self {
            hosts,
            home,
            started_at: chrono::Utc::now().to_rfc3339(),
            contexts: Contexts::new(Arc::clone(&state)),
            repos,
            worktrees,
            pool,
            github,
            watches,
            watch_discovery,
            sessions,
            sleep,
            inspect,
            prune,
            doctor,
            import,
            update,
            config,
            state,
            jobs,
            adapters,
            statuses: Arc::new(RwLock::new(None)),
            pool_refreshed_at: Arc::new(RwLock::new(BTreeMap::new())),
            events,
        }
    }

    /// Assembles an authoritative snapshot from real persisted state and jobs with runtime sessions.
    pub async fn snapshot(&self) -> DaemonResult<Snapshot> {
        let state = self.state.load().await?;
        let config = self.config.load().await?;
        let sessions = self.sessions.snapshot();
        let generated_at = chrono::Utc::now().to_rfc3339();
        let statuses = merge_statuses(&state.worktrees, self.statuses.read().await.as_deref());
        let refreshed = self.pool_refreshed_at.read().await;
        let pools = state
            .repos
            .iter()
            .map(|repo| {
                let root = Path::new(&config.worktrees_dir)
                    .join(&repo.owner)
                    .join(&repo.name);
                let ready = (0..config.hot_pool_size)
                    .filter(|slot| {
                        usize::try_from(*slot)
                            .ok()
                            .is_some_and(|slot| self.adapters.files.exists(&slot_path(&root, slot)))
                    })
                    .count();
                PoolStatus {
                    repo: repo.id.clone(),
                    ready: u32::try_from(ready).unwrap_or(u32::MAX),
                    size: u32::try_from(config.hot_pool_size).unwrap_or(u32::MAX),
                    refreshed_at: refreshed.get(&repo.id).cloned(),
                }
            })
            .collect();
        let hosts = self.hosts.snapshot(&config, &generated_at).await;
        Ok(Snapshot {
            generated_at,
            contexts: state.contexts,
            repos: state.repos,
            clones: state.clones,
            worktrees: state.worktrees,
            active_context: state.active_context_id,
            sessions,
            statuses,
            pools,
            hosts,
            jobs: self.jobs.list(),
            daemon: DaemonInfo {
                version: Self::version(),
                pid: std::process::id(),
                started_at: self.started_at.clone(),
                home: self.home.to_string_lossy().into_owned(),
            },
        })
    }

    /// Dispatches one post-handshake protocol operation to its owning service.
    pub async fn dispatch(&self, body: RequestBody) -> DaemonResult<ResponseBody> {
        self.dispatch_owned(body, 0).await
    }

    pub(crate) async fn dispatch_owned(
        &self,
        body: RequestBody,
        owner: u64,
    ) -> DaemonResult<ResponseBody> {
        self.reject_remote_request(&body).await?;
        match body {
            body @ RequestBody::StartWatch { .. } => Ok(ResponseBody::WatchStarted(
                self.sessions.start_watch(owner, body)?,
            )),
            RequestBody::AppendWatchOutput {
                watch,
                stream,
                text,
            } => {
                self.watches.append_owned(watch, owner, stream, text)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::FinishWatch {
                watch,
                code,
                signal,
            } => {
                self.watches.finish_owned(watch, owner, code, signal)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ListWatches { session } => {
                Ok(ResponseBody::Watches(self.watches.list(&session)))
            }
            RequestBody::TailWatch { watch, from_seq } => {
                Ok(ResponseBody::WatchTail(self.watches.tail(watch, from_seq)?))
            }
            RequestBody::DismissWatch { watch } => {
                self.watches.dismiss(watch)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::Hello { .. }
            | RequestBody::Subscribe { .. }
            | RequestBody::Unsubscribe => Err(DaemonError::Protocol(
                "connection-local request reached service facade".to_owned(),
            )),
            RequestBody::GetSnapshot => Ok(ResponseBody::Snapshot(self.snapshot().await?)),
            RequestBody::CreateContext { name, owners } => Ok(ResponseBody::Context(
                self.contexts.create(name, owners).await?,
            )),
            RequestBody::UpdateContext { id, name, owners } => Ok(ResponseBody::Context(
                self.contexts.update(id, name, owners).await?,
            )),
            RequestBody::DeleteContext { id } => {
                let repo_ids = self
                    .state
                    .load()
                    .await?
                    .repos
                    .into_iter()
                    .filter(|repo| repo.context_id == id)
                    .map(|repo| repo.id)
                    .collect::<Vec<_>>();
                for repo in repo_ids {
                    self.delete_repo_cascade(repo).await?;
                }
                self.contexts.delete(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::SetActiveContext { id } => {
                self.contexts.set_active(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::CloneRepo {
                owner,
                name,
                url,
                context,
                default_branch,
            } => Ok(ResponseBody::CloneStarted(
                self.repos
                    .clone_repo(owner, name, url, context, default_branch)
                    .await?,
            )),
            RequestBody::DeleteRepo { repo } => {
                self.delete_repo_cascade(repo).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::MoveRepoToContext { repo, context } => Ok(ResponseBody::Repo(
                self.repos.move_to_context(repo, context).await?,
            )),
            RequestBody::SearchRemoteRepos { owner, query } => Ok(ResponseBody::RemoteRepos(
                self.repos.search_remote(owner, query).await?,
            )),
            RequestBody::ListRemoteRepos { owner, force } => Ok(ResponseBody::RemoteRepos(
                self.repos.list_remote(owner, force).await?,
            )),
            RequestBody::ListBaseRefs { repo, force } => Ok(ResponseBody::BaseRefs(
                self.repos.list_base_refs(repo, force).await?,
            )),
            RequestBody::SetRepoHooks { repo, hooks } => {
                Ok(ResponseBody::Repo(self.repos.set_hooks(repo, hooks).await?))
            }
            RequestBody::DismissClone { repo } => {
                self.repos.dismiss_clone(repo).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::CreateWorktree {
                repo,
                slug,
                branch,
                base,
                host: Some(_),
                hooks,
            } => {
                let _ = (repo, slug, branch, base, hooks);
                Err(DaemonError::Unsupported(
                    "remote hosts are not supported yet".to_owned(),
                ))
            }
            RequestBody::CreateWorktree {
                repo,
                slug,
                branch,
                base,
                host: None,
                hooks,
            } => {
                let (created, worktree, post_create_job) = self
                    .worktrees
                    .create(repo, slug, branch, base, None, hooks)
                    .await?;
                Ok(ResponseBody::Worktree {
                    created,
                    worktree,
                    post_create_job: post_create_job.map(Box::new),
                })
            }
            RequestBody::DeleteWorktrees { ids } => Ok(ResponseBody::WorktreesDeleted(
                self.worktrees.delete(ids).await?,
            )),
            RequestBody::InspectWorktrees { ids, repo, fetch } => Ok(ResponseBody::Inspections(
                self.inspect.worktrees(ids, repo, fetch).await?,
            )),
            RequestBody::PruneWorktrees {
                dry_run,
                fetch,
                kill_sessions,
                repo,
            } => Ok(ResponseBody::Pruned(
                self.prune
                    .worktrees(dry_run, fetch, kill_sessions, repo)
                    .await?,
            )),
            RequestBody::KillWorktree { id } => {
                self.worktrees.kill(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::SleepWorktree { id } => {
                Ok(ResponseBody::Slept(self.sleep.worktree(id).await?))
            }
            RequestBody::TouchWorktreeOpened { id } => {
                self.worktrees.touch_opened(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::WorktreePath { id } => {
                Ok(ResponseBody::Path(self.worktrees.path(id).await?))
            }
            RequestBody::RestoreTrash { entry } => {
                self.worktrees.restore_trash(entry).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::RefreshStatuses { repo } => {
                let refreshes_all = repo.is_none();
                let statuses = self.sessions.refresh_statuses(repo).await?;
                if refreshes_all {
                    *self.statuses.write().await = Some(statuses.clone());
                } else {
                    let mut cached = self.statuses.write().await;
                    merge_observed_statuses(&mut cached, &statuses);
                }
                Ok(ResponseBody::Statuses(statuses))
            }
            RequestBody::ListPullRequests {
                repo,
                context,
                tab,
                force,
            } => Ok(ResponseBody::PullRequests(
                self.github
                    .list_pull_requests(repo, context, tab, force)
                    .await?,
            )),
            RequestBody::CreateWorktreeFromPr { repo, number } => {
                let (created, worktree, post_create_job) =
                    self.worktrees.create_from_pr(repo, number).await?;
                Ok(ResponseBody::Worktree {
                    created,
                    worktree,
                    post_create_job: post_create_job.map(Box::new),
                })
            }
            RequestBody::EnsureSession {
                worktree,
                agent,
                sleep_previous,
            } => Ok(ResponseBody::Session(
                self.sessions
                    .ensure(worktree, agent, sleep_previous)
                    .await?,
            )),
            RequestBody::ListSessions => Ok(ResponseBody::Sessions(self.sessions.list().await?)),
            RequestBody::CurrentSession => {
                Ok(ResponseBody::CurrentSession(self.sessions.current()))
            }
            RequestBody::KillSession { session } => {
                self.sessions.kill(session).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::SleepSession { session } => {
                Ok(ResponseBody::Slept(self.sleep.session(session).await?))
            }
            RequestBody::NewTerminal {
                session,
                name,
                command,
                cwd,
            } => Ok(ResponseBody::Terminal(
                self.sessions
                    .new_terminal(session, name, command, cwd)
                    .await?,
            )),
            RequestBody::CloseTerminal { terminal } => {
                self.sessions.close_terminal(terminal).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::RestartTerminal { terminal } => Ok(ResponseBody::Terminal(
                self.sessions.restart_terminal(terminal).await?,
            )),
            RequestBody::RenameTerminal { terminal, name } => Ok(ResponseBody::Terminal(
                self.sessions.rename_terminal(terminal, name).await?,
            )),
            RequestBody::SelectTerminal { session, terminal } => Ok(ResponseBody::Session(
                self.sessions.select_terminal(session, terminal).await?,
            )),
            RequestBody::AttachTerminal {
                terminal,
                cols,
                rows,
            } => {
                self.sessions.attach(terminal, cols, rows).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::DetachTerminal { terminal } => {
                self.sessions.detach(terminal).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::TerminalInput { terminal, bytes } => {
                self.sessions.input(terminal, bytes).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::TerminalKey { terminal, key } => {
                self.sessions.key(terminal, key).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::TerminalMouse { terminal, mouse } => {
                self.sessions.mouse(terminal, mouse).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ResizeTerminal {
                terminal,
                cols,
                rows,
            } => {
                self.sessions.resize(terminal, cols, rows).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ScrollTerminal { terminal, scroll } => {
                self.sessions.scroll(terminal, scroll).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::WheelTerminal { terminal, wheel } => {
                self.sessions.wheel(terminal, wheel).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ScrollOrKeyTerminal {
                terminal,
                scroll,
                key,
            } => {
                self.sessions.scroll_or_key(terminal, scroll, key).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::RequestFullFrame { terminal } => {
                self.sessions.request_full_frame(terminal).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::PasteTerminal { terminal, text } => {
                self.sessions.paste(terminal, text).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ListJobs => Ok(ResponseBody::Jobs(self.jobs.list())),
            RequestBody::CancelJob { job } => {
                self.jobs.cancel(&job)?;
                Ok(ResponseBody::JobCancelled(job))
            }
            RequestBody::RetryJob { job } => Ok(ResponseBody::Job(self.jobs.retry(&job)?)),
            RequestBody::TailJob { job, lines } => {
                Ok(ResponseBody::JobLog(self.jobs.tail(&job, lines).await?))
            }
            RequestBody::DismissJobs { jobs } => {
                self.jobs.dismiss(&jobs)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::GetConfig => Ok(ResponseBody::Config(self.config.load().await?)),
            RequestBody::SetConfig { patch } => {
                Ok(ResponseBody::Config(self.config.update(patch).await?))
            }
            RequestBody::MatchKeepAliveRules => Ok(ResponseBody::KeepAliveRuleMatches(
                self.sleep.match_keep_alive_rules().await?,
            )),
            RequestBody::ImportFromSwarm => Ok(ResponseBody::Job(self.import.start().await?)),
            RequestBody::Doctor => Ok(ResponseBody::Doctor(self.doctor.check().await?)),
            RequestBody::Update => Ok(ResponseBody::Job(self.update.start().await?)),
            RequestBody::DaemonPing => Ok(ResponseBody::Pong),
            RequestBody::DaemonVersion => Ok(ResponseBody::Version {
                version: Self::version(),
                protocol: fleet_proto::PROTOCOL_VERSION,
            }),
            RequestBody::DaemonShutdown { .. } => Ok(ResponseBody::ShuttingDown),
        }
    }

    /// Returns the daemon build version string.
    #[must_use]
    pub fn version() -> String {
        format!("fleetd {}", env!("CARGO_PKG_VERSION"))
    }

    async fn delete_repo_cascade(&self, repo: RepoId) -> DaemonResult<()> {
        let _deleting = self.jobs.begin_repo_deletion(&repo)?;
        self.jobs.quiesce_repo(&repo).await?;
        let ids = self
            .state
            .load()
            .await?
            .worktrees
            .into_iter()
            .filter(|worktree| worktree.repo_id == repo)
            .map(|worktree| worktree.id)
            .collect::<Vec<_>>();
        let failures = self
            .worktrees
            .delete(ids)
            .await?
            .into_iter()
            .filter(|result| !result.ok)
            .map(|result| {
                result
                    .reason
                    .unwrap_or_else(|| format!("could not delete {}", result.worktree_id))
            })
            .collect::<Vec<_>>();
        if !failures.is_empty() {
            return Err(DaemonError::Conflict(failures.join("; ")));
        }
        self.repos.delete_guarded(repo).await
    }

    /// Starts status, host, prepared-pool, and PR-cache maintenance loops.
    pub async fn start_periodic_tasks(
        self: &Arc<Self>,
        events: BroadcastBus,
        shutdown: CancellationToken,
    ) -> DaemonResult<PeriodicTasks> {
        self.repos.reconcile_startup().await?;
        let config = self.config.load().await?;
        self.jobs
            .set_retention(Duration::from_millis(config.jobs.keep_finished_for));
        let mut handles = Vec::new();
        handles.push(tokio::spawn(self.watches.clone().run(shutdown.clone())));
        handles.push(tokio::spawn(self.watch_discovery.clone().run(
            shutdown.clone(),
            Duration::from_millis(config.discovered_watches.interval_ms.max(500)),
        )));

        let status_every = duration_from_millis(config.ui.status_refresh_ms, 500);
        handles.push(tokio::spawn(run_status_refresh(
            Arc::clone(self),
            events.clone(),
            shutdown.clone(),
            status_every,
        )));

        if !config.hosts.is_empty() {
            handles.push(tokio::spawn(run_host_refresh(
                Arc::clone(self),
                events.clone(),
                shutdown.clone(),
                Duration::from_secs(60),
            )));
        }

        if config.hot_pool_size > 0 && config.hot_refresh_interval_ms > 0 {
            handles.push(tokio::spawn(run_pool_refresh(
                Arc::clone(self),
                events,
                shutdown.clone(),
                Duration::from_millis(config.hot_refresh_interval_ms),
            )));
        }

        handles.push(tokio::spawn(run_pr_cache_expiry(
            Arc::clone(self),
            shutdown,
            duration_from_seconds(config.github.pr_ttl_seconds),
        )));
        Ok(PeriodicTasks { handles })
    }

    /// Best-effort termination of all daemon-owned terminal sessions.
    pub async fn stop_all_sessions(&self) {
        for session in self.sessions.snapshot() {
            if let Err(error) = self.sessions.kill(session.id).await {
                tracing::warn!(%error, "failed to stop session during daemon shutdown");
            }
        }
    }

    async fn reject_remote_request(&self, body: &RequestBody) -> DaemonResult<()> {
        let needs_state = matches!(
            body,
            RequestBody::DeleteContext { .. }
                | RequestBody::DeleteRepo { .. }
                | RequestBody::DeleteWorktrees { .. }
                | RequestBody::InspectWorktrees { .. }
                | RequestBody::PruneWorktrees { .. }
                | RequestBody::KillWorktree { .. }
                | RequestBody::SleepWorktree { .. }
                | RequestBody::WorktreePath { .. }
                | RequestBody::RefreshStatuses { .. }
                | RequestBody::EnsureSession {
                    worktree: Some(_),
                    ..
                }
        );
        if !needs_state {
            return Ok(());
        }
        let state = self.state.load().await?;
        let proxies_remote = match body {
            RequestBody::DeleteContext { id } => state.worktrees.iter().any(|worktree| {
                worktree.host.is_some()
                    && state
                        .repos
                        .iter()
                        .any(|repo| repo.id == worktree.repo_id && &repo.context_id == id)
            }),
            RequestBody::DeleteRepo { repo } => state
                .worktrees
                .iter()
                .any(|worktree| worktree.host.is_some() && &worktree.repo_id == repo),
            RequestBody::DeleteWorktrees { ids } => state
                .worktrees
                .iter()
                .any(|worktree| worktree.host.is_some() && ids.contains(&worktree.id)),
            RequestBody::InspectWorktrees { ids, repo, .. } => {
                state.worktrees.iter().any(|worktree| {
                    worktree.host.is_some()
                        && (ids.is_empty() || ids.contains(&worktree.id))
                        && repo.as_ref().is_none_or(|repo| &worktree.repo_id == repo)
                })
            }
            RequestBody::PruneWorktrees { repo, .. } | RequestBody::RefreshStatuses { repo } => {
                state.worktrees.iter().any(|worktree| {
                    worktree.host.is_some()
                        && repo.as_ref().is_none_or(|repo| &worktree.repo_id == repo)
                })
            }
            RequestBody::KillWorktree { id }
            | RequestBody::SleepWorktree { id }
            | RequestBody::WorktreePath { id }
            | RequestBody::EnsureSession {
                worktree: Some(id), ..
            } => state
                .worktrees
                .iter()
                .any(|worktree| worktree.host.is_some() && &worktree.id == id),
            _ => false,
        };
        if proxies_remote {
            Err(DaemonError::Unsupported(
                "remote hosts are not supported yet".to_owned(),
            ))
        } else {
            Ok(())
        }
    }
}

fn merge_statuses(
    worktrees: &[fleet_core::model::Worktree],
    observed: Option<&[WorktreeStatus]>,
) -> Vec<WorktreeStatus> {
    let observed = observed
        .unwrap_or_default()
        .iter()
        .map(|status| (&status.worktree_id, status))
        .collect::<BTreeMap<_, _>>();
    worktrees
        .iter()
        .map(|worktree| {
            observed
                .get(&worktree.id)
                .map(|status| (*status).clone())
                .unwrap_or_else(|| {
                    Inspect::unknown_statuses(std::slice::from_ref(worktree)).remove(0)
                })
        })
        .collect()
}

fn merge_observed_statuses(cached: &mut Option<Vec<WorktreeStatus>>, observed: &[WorktreeStatus]) {
    let statuses = cached.get_or_insert_with(Vec::new);
    for status in observed {
        if let Some(existing) = statuses
            .iter_mut()
            .find(|existing| existing.worktree_id == status.worktree_id)
        {
            *existing = status.clone();
        } else {
            statuses.push(status.clone());
        }
    }
}

fn duration_from_millis(value: i64, minimum: u64) -> Duration {
    Duration::from_millis(u64::try_from(value).unwrap_or(0).max(minimum))
}

fn duration_from_seconds(value: i64) -> Duration {
    Duration::from_secs(u64::try_from(value).unwrap_or(0).max(1))
}

async fn run_status_refresh(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
    every: Duration,
) {
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = interval.tick() => {
                match services.sessions.refresh_statuses(None).await {
                    Ok(statuses) => {
                        *services.statuses.write().await = Some(statuses);
                        events.request_snapshot(Arc::clone(&services));
                    }
                    Err(DaemonError::Unimplemented(_)) => {}
                    Err(error) => tracing::warn!(%error, "periodic status refresh failed"),
                }
            }
        }
    }
}

async fn run_host_refresh(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
    every: Duration,
) {
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous = BTreeMap::new();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        let refresh = async {
            let config = services.config.load().await?;
            let results = services.hosts.probe_all(&config).await;
            let current = results
                .into_iter()
                .map(|(id, status)| (id, (status.reachable, status.error)))
                .collect::<BTreeMap<_, _>>();
            if current != previous {
                previous = current;
                events.request_snapshot(Arc::clone(&services));
            }
            Ok::<(), DaemonError>(())
        };
        tokio::select! {
            () = shutdown.cancelled() => break,
            result = refresh => {
                if let Err(error) = result {
                    tracing::warn!(%error, "periodic host refresh failed");
                }
            }
        }
    }
}

async fn run_pool_refresh(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
    every: Duration,
) {
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = interval.tick() => {
                let state = match services.state.load().await {
                    Ok(state) => state,
                    Err(error) => {
                        tracing::warn!(%error, "periodic pool refresh could not load state");
                        continue;
                    }
                };
                let mut changed = false;
                for repo in state.repos {
                    if shutdown.is_cancelled() {
                        break;
                    }
                    match services.pool.prepare(repo.id.clone(), false).await {
                        Ok(()) => {
                            services.pool_refreshed_at.write().await
                                .insert(repo.id, chrono::Utc::now().to_rfc3339());
                            changed = true;
                        }
                        Err(DaemonError::Unimplemented(_)) => {}
                        Err(error) => tracing::warn!(%error, "periodic pool refresh failed"),
                    }
                }
                if changed {
                    events.request_snapshot(Arc::clone(&services));
                }
            }
        }
    }
}

async fn run_pr_cache_expiry(services: Arc<Services>, shutdown: CancellationToken, ttl: Duration) {
    let every = ttl.min(Duration::from_secs(60));
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = interval.tick() => {
                let root = fleet_core::paths::FleetHome::new(services.home.clone())
                    .github_cache_dir()
                    .join("prs");
                if let Err(error) = expire_cache_files(
                    services.adapters.files.as_ref(),
                    &root,
                    chrono::Utc::now(),
                    ttl,
                ) {
                    tracing::warn!(%error, "failed to expire pull-request cache");
                }
            }
        }
    }
}

fn expire_cache_files(
    files: &dyn crate::adapters::files::Files,
    root: &Path,
    now: chrono::DateTime<chrono::Utc>,
    ttl: Duration,
) -> DaemonResult<()> {
    if !files.exists(root) {
        return Ok(());
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for path in files.list(&directory)? {
            if path.extension().is_none_or(|extension| extension != "json") {
                pending.push(path);
                continue;
            }
            let fetched_at = files
                .read_text(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| {
                    value
                        .get("fetchedAt")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                })
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
                .map(|value| value.with_timezone(&chrono::Utc));
            let stale = fetched_at.is_none_or(|fetched_at| {
                now.signed_duration_since(fetched_at)
                    >= chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX)
            });
            if stale {
                files.remove_file(&path)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod host_refresh_tests {
    use super::*;
    use crate::{
        adapters::{clock::SystemClock, shell::ShellResult},
        testing::fakes::{FakeFiles, FakeShell},
    };
    use fleet_proto::event::Event;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn host_refresh_publishes_changes_but_not_new_timestamps() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = temp.path();
        let files = Arc::new(FakeFiles::new(
            home.join("trash"),
            vec![home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        config
            .update(serde_json::json!({
                "hosts": {"dev-box": {"ssh": "arch-dev", "swarmCommand": "swarm"}}
            }))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        let online = Arc::new(AtomicBool::new(true));
        let health = Arc::clone(&online);
        shell.when(
            move |_| health.load(Ordering::SeqCst),
            ShellResult {
                status: 0,
                stdout: r#"{"protocol":1}"#.to_owned(),
                stderr: String::new(),
            },
        );
        shell.when(
            |_| true,
            ShellResult {
                status: 255,
                stdout: String::new(),
                stderr: "Connection refused".to_owned(),
            },
        );
        let mut adapters = Adapters::system(files.clone());
        adapters.shell = shell.clone();
        let services = Arc::new(Services::new(
            home,
            config,
            Arc::new(StateStore::new(home, files, Arc::new(SystemClock))),
            Arc::new(JobManager::new(home)),
            adapters,
        ));
        let pending = services
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(pending.hosts[0].error.as_deref(), Some("probe pending"));
        assert_eq!(pending.hosts[0].checked_at, pending.generated_at);
        let events = BroadcastBus::default();
        let mut receiver = events.subscribe();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run_host_refresh(
            services,
            events,
            shutdown.clone(),
            Duration::from_millis(10),
        ));
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
        let Event::SnapshotChanged(snapshot) = event else {
            panic!("expected snapshot");
        };
        assert!(snapshot.hosts[0].reachable);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), receiver.recv())
                .await
                .is_err()
        );
        assert!(
            shell.calls().len() > 1,
            "must have refreshed unchanged results"
        );
        online.store(false, Ordering::SeqCst);
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
        let Event::SnapshotChanged(snapshot) = event else {
            panic!("expected snapshot");
        };
        assert!(!snapshot.hosts[0].reachable);
        assert_eq!(
            snapshot.hosts[0].error.as_deref(),
            Some("Connection refused")
        );
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
    }
}
