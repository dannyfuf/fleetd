//! Fleet application services orchestrating domain rules, stores, adapters, and jobs.

use std::{path::PathBuf, sync::Arc};

use fleet_proto::{
    request::RequestBody,
    response::ResponseBody,
    snapshot::{DaemonInfo, HostStatus, Snapshot},
};

use crate::{
    DaemonError, DaemonResult,
    adapters::Adapters,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

pub mod contexts;
pub mod doctor;
pub mod github;
pub mod import;
pub mod inspect;
pub mod pool;
pub mod prune;
pub mod repos;
pub mod sessions;
pub mod sleep;
pub mod update;
pub mod worktrees;

use contexts::Contexts;
use doctor::Doctor;
use github::Github;
use inspect::Inspect;
use pool::Pool;
use prune::Prune;
use repos::Repos;
use sessions::Sessions;
use sleep::Sleep;
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
    /// Runtime PTY session service.
    pub sessions: Sessions,
    /// Session sleep-policy service.
    pub sleep: Sleep,
    /// Worktree inspection service.
    pub inspect: Inspect,
    /// Safe prune service.
    pub prune: Prune,
    /// Environment diagnostics and updater service.
    pub doctor: Doctor,
    home: PathBuf,
    started_at: String,
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
        let repos = Repos::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.github),
            Arc::clone(&adapters.files),
        );
        let worktrees = Worktrees::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.files),
            Arc::clone(&adapters.git),
        );
        let pool = Pool::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.files),
        );
        let github = Github::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.github),
        );
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
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
        );
        let prune = Prune::new(Arc::clone(&state), Arc::clone(&jobs), inspect.clone());
        let doctor = Doctor::new(
            Arc::clone(&jobs),
            Arc::clone(&config),
            Arc::clone(&adapters.shell),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.github),
            Arc::clone(&adapters.files),
        );
        Self {
            home: home.into(),
            started_at: chrono::Utc::now().to_rfc3339(),
            contexts: Contexts::new(Arc::clone(&state)),
            repos,
            worktrees,
            pool,
            github,
            sessions,
            sleep,
            inspect,
            prune,
            doctor,
            config,
            state,
            jobs,
            adapters,
        }
    }

    /// Assembles an authoritative snapshot from real persisted state and jobs with runtime sessions.
    pub async fn snapshot(&self) -> DaemonResult<Snapshot> {
        let state = self.state.load().await?;
        let config = self.config.load().await?;
        let sessions = self.sessions.snapshot();
        let generated_at = chrono::Utc::now().to_rfc3339();
        let statuses = Inspect::unknown_statuses(&state.worktrees);
        let pools = Pool::snapshot_statuses(&state.repos, config.hot_pool_size);
        let hosts = config
            .hosts
            .keys()
            .cloned()
            .map(|id| HostStatus {
                id,
                reachable: false,
                checked_at: generated_at.clone(),
                error: Some("host status has not been observed yet".to_owned()),
            })
            .collect();
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
        match body {
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
            } => Ok(ResponseBody::CloneStarted(
                self.repos.clone_repo(owner, name, url, context).await?,
            )),
            RequestBody::DeleteRepo { repo } => {
                self.repos.delete(repo).await?;
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
                host,
                hooks,
            } => {
                let (created, worktree) = self
                    .worktrees
                    .create(repo, slug, branch, base, host, hooks)
                    .await?;
                Ok(ResponseBody::Worktree { created, worktree })
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
            RequestBody::RefreshStatuses { repo } => Ok(ResponseBody::Statuses(
                self.inspect.refresh_statuses(repo).await?,
            )),
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
                let (created, worktree) = self.worktrees.create_from_pr(repo, number).await?;
                Ok(ResponseBody::Worktree { created, worktree })
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
            RequestBody::GetConfig => Ok(ResponseBody::Config(self.config.load().await?)),
            RequestBody::SetConfig { patch } => {
                Ok(ResponseBody::Config(self.config.update(patch).await?))
            }
            RequestBody::MatchKeepAliveRules => Ok(ResponseBody::KeepAliveRuleMatches(
                self.sleep.match_keep_alive_rules().await?,
            )),
            RequestBody::ImportFromSwarm => {
                Ok(ResponseBody::Job(self.repos.import_from_swarm().await?))
            }
            RequestBody::Doctor => Ok(ResponseBody::Doctor(self.doctor.check().await?)),
            RequestBody::Update => Ok(ResponseBody::Job(self.doctor.update().await?)),
            RequestBody::DaemonPing => Ok(ResponseBody::Pong),
            RequestBody::DaemonVersion => Ok(ResponseBody::Version {
                version: Self::version(),
                protocol: 1,
            }),
            RequestBody::DaemonShutdown { .. } => Ok(ResponseBody::ShuttingDown),
        }
    }

    /// Returns the daemon build version string.
    #[must_use]
    pub fn version() -> String {
        format!("fleetd {}", env!("CARGO_PKG_VERSION"))
    }
}
