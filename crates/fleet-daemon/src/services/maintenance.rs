use super::*;
use crate::adapters::files::FileKind;
use futures_util::{StreamExt, stream};
use std::sync::{Mutex as StdMutex, OnceLock};
use tokio::sync::watch;

type RuntimeConfig = Option<Arc<Config>>;

fn runtime_configs() -> &'static StdMutex<BTreeMap<PathBuf, watch::Sender<RuntimeConfig>>> {
    static CONFIGS: OnceLock<StdMutex<BTreeMap<PathBuf, watch::Sender<RuntimeConfig>>>> =
        OnceLock::new();
    CONFIGS.get_or_init(|| StdMutex::new(BTreeMap::new()))
}

pub(super) fn runtime_config_receiver(config: &ConfigStore) -> watch::Receiver<RuntimeConfig> {
    let mut configs = runtime_configs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    configs
        .entry(config.path().to_path_buf())
        .or_insert_with(|| watch::channel(None).0)
        .subscribe()
}

pub(super) fn publish_runtime_config(config: &ConfigStore, value: &Config) {
    let mut configs = runtime_configs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    configs
        .entry(config.path().to_path_buf())
        .or_insert_with(|| watch::channel(None).0)
        .send_replace(Some(Arc::new(value.clone())));
}

async fn initial_runtime_config(
    store: &ConfigStore,
    receiver: &watch::Receiver<RuntimeConfig>,
) -> DaemonResult<Arc<Config>> {
    if let Some(config) = receiver.borrow().clone() {
        return Ok(config);
    }
    let config = Arc::new(store.load().await?);
    publish_runtime_config(store, &config);
    Ok(config)
}

/// Owned periodic daemon tasks, joined during graceful shutdown.
pub struct PeriodicTasks {
    handles: Vec<JoinHandle<()>>,
}

impl PeriodicTasks {
    /// Waits briefly for cancellation-aware periodic loops to finish.
    pub async fn join(mut self) {
        for mut handle in self.handles.drain(..) {
            match tokio::time::timeout(Duration::from_secs(2), &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "periodic task failed"),
                Err(_) => {
                    handle.abort();
                    let _ = handle.await;
                }
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
    /// Starts status, host, prepared-pool, and PR-cache maintenance loops.
    pub async fn start_periodic_tasks(
        self: &Arc<Self>,
        events: BroadcastBus,
        shutdown: CancellationToken,
    ) -> DaemonResult<PeriodicTasks> {
        self.import.recover().await?;
        self.repos.reconcile_startup().await?;
        let config = self.config.load().await?;
        self.reconcile_runtime_config(&config);
        let handles = vec![
            tokio::spawn(self.watches.clone().run(shutdown.clone())),
            tokio::spawn(self.watch_discovery.clone().run(shutdown.clone())),
            tokio::spawn(run_status_refresh(
                Arc::clone(self),
                events.clone(),
                shutdown.clone(),
            )),
            tokio::spawn(run_agent_activity_refresh(
                Arc::clone(self),
                shutdown.clone(),
            )),
            tokio::spawn(run_host_refresh(
                Arc::clone(self),
                events.clone(),
                shutdown.clone(),
            )),
            tokio::spawn(run_pool_refresh(Arc::clone(self), events, shutdown.clone())),
            tokio::spawn(checkpoints::run_sweep(Arc::clone(self), shutdown.clone())),
            tokio::spawn(run_pr_cache_expiry(Arc::clone(self), shutdown)),
        ];
        Ok(PeriodicTasks { handles })
    }

    pub(super) fn reconcile_runtime_config(&self, config: &Config) {
        self.machines.rebuild(config);
        self.jobs
            .set_retention(Duration::from_millis(config.jobs.keep_finished_for));
        self.adapters.files.set_removable_roots(vec![
            PathBuf::from(&config.repos_dir),
            PathBuf::from(&config.worktrees_dir),
        ]);
        publish_runtime_config(&self.config, config);
    }

    /// Best-effort termination of all daemon-owned terminal sessions.
    ///
    /// Killing a session tells its holders to end their children; the wait afterwards is what
    /// makes `DaemonShutdown { stop_sessions: true }` mean the terminals are actually gone rather
    /// than merely asked to go. A plain SIGTERM, and `stop_sessions: false`, never come here: the
    /// holders survive those, which is how `fleet daemon restart` keeps the user's terminals.
    pub async fn stop_all_sessions(&self) {
        for session in self.sessions.snapshot() {
            if let Err(error) = self.sessions.kill(session.id).await {
                tracing::warn!(%error, "failed to stop session during daemon shutdown");
            }
        }
        self.sessions.wait_for_holders_to_stop().await;
    }

    pub(super) async fn apply_agent_activity_transitions(
        &self,
        transitions: Vec<sessions::AgentActivityTransition>,
    ) -> DaemonResult<()> {
        if transitions.is_empty() {
            return Ok(());
        }
        for transition in transitions {
            self.events.publish(Event::AgentActivityChanged {
                session: transition.session,
                terminal_id: transition.terminal,
                agent: transition.agent,
                activity: transition.activity,
                attention: transition.attention,
                changed_at: transition.changed_at,
            });
        }
        self.events.request_snapshot_current();
        Ok(())
    }
}

fn duration_from_millis(value: i64, minimum: u64) -> Duration {
    Duration::from_millis(u64::try_from(value).unwrap_or(0).max(minimum))
}

fn duration_from_seconds(value: i64) -> Duration {
    Duration::from_secs(u64::try_from(value).unwrap_or(0).max(1))
}

const PR_CACHE_RETENTION_MULTIPLIER: u32 = 10;

fn pr_cache_retention(ttl: Duration) -> Duration {
    ttl.saturating_mul(PR_CACHE_RETENTION_MULTIPLIER)
}

/// Shortest gap between two warnings about the same unchanged failure on a periodic task.
pub(super) const REPEATED_FAILURE_INTERVAL: Duration = Duration::from_secs(3600);

/// Warns about a failure that a periodic task keeps hitting, without repeating itself.
///
/// The status tick runs every `ui.statusRefreshMs` — 2 s by default, 500 ms at the floor —
/// so one permanently broken dependency, a missing `lsof` or an unreadable `ps`, becomes
/// tens of thousands of identical lines a day. That is how a daemon log reaches tens of
/// megabytes while saying one thing.
///
/// Suppression is keyed on the message, never on time alone: a *different* failure arriving
/// inside the window is news and is warned about immediately.
pub(super) struct RepeatedFailure {
    message: &'static str,
    last: Option<(String, tokio::time::Instant)>,
}

impl RepeatedFailure {
    pub(super) const fn new(message: &'static str) -> Self {
        Self {
            message,
            last: None,
        }
    }

    pub(super) fn report(&mut self, error: &crate::DaemonError) {
        let error = error.to_string();
        let now = tokio::time::Instant::now();
        let repeated = self.last.as_ref().is_some_and(|(previous, at)| {
            previous == &error && now.duration_since(*at) < REPEATED_FAILURE_INTERVAL
        });
        if repeated {
            tracing::debug!(%error, "{}", self.message);
            return;
        }
        self.last = Some((error.clone(), now));
        tracing::warn!(%error, "{}", self.message);
    }
}

async fn run_status_refresh(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
) {
    let mut policy = runtime_config_receiver(&services.config);
    let mut config = match initial_runtime_config(&services.config, &policy).await {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "status refresh could not load runtime policy");
            return;
        }
    };
    let mut next = tokio::time::Instant::now();
    let mut observation_failures =
        RepeatedFailure::new("failed to refresh terminal process observations");
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            changed = policy.changed() => {
                if changed.is_err() { break; }
                if let Some(updated) = policy.borrow().clone() {
                    config = updated;
                    next = tokio::time::Instant::now();
                }
            }
            _ = tokio::time::sleep_until(next) => {
                let every = duration_from_millis(config.ui.status_refresh_ms, 500);
                next = tokio::time::Instant::now() + every;
                if let Err(error) = services.sleep.refresh_observations().await {
                    observation_failures.report(&error);
                }
                events.request_snapshot(Arc::clone(&services));
            }
        }
    }
}

async fn run_agent_activity_refresh(services: Arc<Services>, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(agent_activity::TRACK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = interval.tick() => {
                let transitions = services
                    .sessions
                    .observe_agent_activities(std::time::Instant::now());
                if let Err(error) = services.apply_agent_activity_transitions(transitions).await {
                    tracing::warn!(%error, "agent activity refresh failed");
                }
            }
        }
    }
}

async fn run_host_refresh(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
) {
    let mut policy = runtime_config_receiver(&services.config);
    let mut config = match initial_runtime_config(&services.config, &policy).await {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "host refresh could not load runtime policy");
            return;
        }
    };
    let mut previous = BTreeMap::new();
    let mut next = tokio::time::Instant::now();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            changed = policy.changed() => {
                if changed.is_err() { break; }
                if let Some(updated) = policy.borrow().clone() {
                    config = updated;
                    next = tokio::time::Instant::now();
                }
                continue;
            }
            _ = tokio::time::sleep_until(next) => {}
        }
        next = tokio::time::Instant::now() + host_refresh_interval(&config);
        let refresh = async {
            let results = services
                .hosts
                .refresh_all(config.as_ref(), &services.machines)
                .await;
            let changed = results
                .iter()
                .filter(|(id, status)| host_status_changed(previous.get(*id), status))
                .map(|(id, status)| (id.clone(), status.clone()))
                .collect::<Vec<_>>();
            for (id, status) in &changed {
                events.publish(Event::HostLinkChanged {
                    host: id.clone(),
                    link: status.link,
                    version: status.version.clone(),
                    error: status.error.clone(),
                });
            }
            for id in previous.keys().filter(|id| !results.contains_key(*id)) {
                events.publish(Event::HostLinkChanged {
                    host: id.clone(),
                    link: fleet_proto::snapshot::LinkState::Down,
                    version: None,
                    error: Some("host removed from configuration".to_owned()),
                });
            }
            if !changed.is_empty() || previous.keys().any(|id| !results.contains_key(id)) {
                events.request_snapshot(Arc::clone(&services));
            }
            previous = results;
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

fn host_refresh_interval(config: &Config) -> Duration {
    duration_from_millis(config.ui.remote_status_refresh_ms, 500)
}

fn host_status_changed(
    previous: Option<&fleet_proto::snapshot::HostStatus>,
    current: &fleet_proto::snapshot::HostStatus,
) -> bool {
    previous.is_none_or(|previous| {
        previous.provider != current.provider
            || previous.version != current.version
            || previous.link != current.link
            || previous.address != current.address
            || previous.agent_binaries != current.agent_binaries
            || previous.reachable != current.reachable
            || previous.error != current.error
    })
}

async fn run_pool_refresh(
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
) {
    let mut policy = runtime_config_receiver(&services.config);
    let mut config = match initial_runtime_config(&services.config, &policy).await {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "pool refresh could not load runtime policy");
            return;
        }
    };
    let mut next = tokio::time::Instant::now();
    loop {
        let enabled = config.hot_pool_size != 0 && config.hot_refresh_interval_ms != 0;
        let every = Duration::from_millis(config.hot_refresh_interval_ms.max(1));
        tokio::select! {
            () = shutdown.cancelled() => break,
            changed = policy.changed() => {
                if changed.is_err() { break; }
                if let Some(updated) = policy.borrow().clone() {
                    config = updated;
                    next = tokio::time::Instant::now();
                }
            }
            _ = tokio::time::sleep_until(next), if enabled => {
                next = tokio::time::Instant::now() + every;
                let state = match services.state.load().await {
                    Ok(state) => state,
                    Err(error) => {
                        tracing::warn!(%error, "periodic pool refresh could not load state");
                        continue;
                    }
                };
                let mut changed = false;
                let mut pending = stream::iter(state.repos.into_iter().map(|repo| {
                    let services = &services;
                    let shutdown = &shutdown;
                    async move {
                        if shutdown.is_cancelled() { return (repo.id, Err(DaemonError::Cancelled)); }
                        let result = services.pool.prepare(repo.id.clone(), false).await;
                        (repo.id, result)
                    }
                })).buffer_unordered(2);
                while let Some((repo, result)) = pending.next().await {
                    match result {
                        Ok(()) => {
                            services.pool_refreshed_at.write().await.insert(repo, chrono::Utc::now().to_rfc3339());
                            changed = true;
                        }
                        Err(DaemonError::Cancelled) if shutdown.is_cancelled() => {}
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

async fn run_pr_cache_expiry(services: Arc<Services>, shutdown: CancellationToken) {
    let mut policy = runtime_config_receiver(&services.config);
    let mut config = match initial_runtime_config(&services.config, &policy).await {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "PR cache expiry could not load runtime policy");
            return;
        }
    };
    let mut next = tokio::time::Instant::now();
    loop {
        let ttl = duration_from_seconds(config.github.pr_ttl_seconds);
        let every = ttl.min(Duration::from_secs(60));
        tokio::select! {
            () = shutdown.cancelled() => break,
            changed = policy.changed() => {
                if changed.is_err() { break; }
                if let Some(updated) = policy.borrow().clone() {
                    config = updated;
                    next = tokio::time::Instant::now();
                }
            }
            _ = tokio::time::sleep_until(next) => {
                next = tokio::time::Instant::now() + every;
                let root = fleet_core::paths::FleetHome::new(services.home.clone())
                    .github_cache_dir()
                    .join("prs");
                if let Err(error) = expire_cache_files(
                    services.adapters.files.as_ref(),
                    &root,
                    chrono::Utc::now(),
                    pr_cache_retention(ttl),
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
    retention: Duration,
) -> DaemonResult<()> {
    if !files.exists(root) {
        return Ok(());
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match files.list(&directory) {
            Ok(entries) => entries,
            Err(error) if directory != root => {
                tracing::warn!(
                    %error,
                    path = %directory.display(),
                    "skipping unreadable pull-request cache directory"
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        for path in entries {
            let metadata = match files.metadata(&path) {
                Ok(metadata) => metadata,
                Err(DaemonError::Filesystem { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        path = %path.display(),
                        "skipping unreadable pull-request cache entry"
                    );
                    continue;
                }
            };
            match metadata.kind {
                FileKind::Directory => {
                    pending.push(path);
                    continue;
                }
                FileKind::Other => continue,
                FileKind::File if path.extension().is_none_or(|extension| extension != "json") => {
                    continue;
                }
                FileKind::File => {}
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
                    >= chrono::Duration::from_std(retention).unwrap_or(chrono::Duration::MAX)
            });
            if stale && let Err(error) = files.remove_file_if_unchanged(&path, metadata) {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "failed to expire pull-request cache entry"
                );
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
        testing::fakes::{FakeFiles, FakeFilesCall, FakeShell, FixedClock},
    };
    use fleet_core::ids::{ContextId, RepoId};
    use fleet_proto::{event::Event, job::JobKind, request::RequestBody};
    use std::{
        io,
        sync::atomic::{AtomicBool, Ordering},
    };

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
                "hosts": {"dev-box": {"ssh": "arch-dev", "swarmCommand": "swarm"}},
                "ui": {"remoteStatusRefreshMs": 500}
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
        let task = tokio::spawn(run_host_refresh(services, events, shutdown.clone()));
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
        let Event::HostLinkChanged { host, link, .. } = event else {
            panic!("expected host link change");
        };
        assert_eq!(host.as_str(), "dev-box");
        assert_eq!(link, fleet_proto::snapshot::LinkState::Legacy);
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
        let Event::SnapshotChanged(snapshot) = event else {
            panic!("expected snapshot");
        };
        assert!(snapshot.hosts[0].reachable);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), receiver.recv())
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(450)).await;
        assert!(
            shell.calls().len() > 1,
            "must have refreshed unchanged results"
        );
        online.store(false, Ordering::SeqCst);
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
        let Event::HostLinkChanged { error, .. } = event else {
            panic!("expected host link change");
        };
        assert_eq!(error.as_deref(), Some("Connection refused"));
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

    /// Regression: a devbox with no `lsof` logged the same refresh failure every two
    /// seconds until `~/.fleet/logs` reached 39 MB.
    #[tokio::test(start_paused = true)]
    async fn an_unchanged_periodic_failure_warns_once_an_hour() {
        let mut failures = RepeatedFailure::new("failed to refresh terminal process observations");
        let missing_lsof =
            crate::DaemonError::Process("lsof: No such file or directory".to_owned());

        failures.report(&missing_lsof);
        let first = failures.last.clone();
        failures.report(&missing_lsof);
        tokio::time::advance(Duration::from_secs(1_800)).await;
        failures.report(&missing_lsof);
        // Ticks inside the window neither warn again nor move the window.
        assert_eq!(failures.last, first);

        tokio::time::advance(REPEATED_FAILURE_INTERVAL).await;
        failures.report(&missing_lsof);
        assert_ne!(failures.last, first);

        // A different failure is news and is reported immediately.
        let stale = failures.last.clone();
        failures.report(&crate::DaemonError::Process("ps exited 1".to_owned()));
        assert_ne!(failures.last, stale);
    }

    #[test]
    fn host_refresh_interval_clamps_to_five_hundred_milliseconds() {
        let mut config = fleet_core::config::default_config("/tmp/fleet");
        config.ui.remote_status_refresh_ms = 1;
        assert_eq!(host_refresh_interval(&config), Duration::from_millis(500));
        config.ui.remote_status_refresh_ms = 4_321;
        assert_eq!(host_refresh_interval(&config), Duration::from_millis(4_321));
    }

    #[tokio::test]
    async fn set_config_rearms_runtime_policy() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let files = Arc::new(FakeFiles::new(
            home.join("trash"),
            vec![home.join("repos"), home.join("worktrees")],
        ));
        let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
        let jobs = Arc::new(JobManager::with_clock(home, clock.clone()));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        let services = Services::new(
            home,
            config,
            Arc::new(StateStore::new(home, files.clone(), clock.clone())),
            jobs.clone(),
            Adapters::system(files),
        );
        let id = jobs.submit(
            JobKind::Custom("completed".into()),
            "runtime-policy",
            "Completed job",
            false,
            false,
            |_context| async { Ok(()) },
        );
        jobs.wait(&id).await.unwrap();
        clock.set(chrono::Utc::now() + chrono::Duration::seconds(1));

        services
            .dispatch(RequestBody::SetConfig {
                patch: serde_json::json!({"jobs": {"keepFinishedFor": 0}}),
            })
            .await
            .unwrap();

        assert!(jobs.record(&id).is_none());
    }

    #[tokio::test]
    async fn set_config_pushes_runtime_policy_without_polling_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let files = Arc::new(FakeFiles::new(
            home.join("trash"),
            vec![home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        let services = Services::new(
            home,
            config.clone(),
            Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock))),
            Arc::new(JobManager::new(home)),
            Adapters::system(files.clone()),
        );
        let initial = config.load().await.unwrap();
        services.reconcile_runtime_config(&initial);
        let mut first = runtime_config_receiver(&config);
        let mut second = runtime_config_receiver(&config);

        services
            .dispatch(RequestBody::SetConfig {
                patch: serde_json::json!({"ui": {"statusRefreshMs": 4321}}),
            })
            .await
            .unwrap();
        first.changed().await.unwrap();
        second.changed().await.unwrap();
        let reads_after_update = files
            .calls()
            .iter()
            .filter(|call| matches!(call, crate::testing::fakes::FakeFilesCall::Read(path) if path == config.path()))
            .count();

        assert_eq!(first.borrow().as_ref().unwrap().ui.status_refresh_ms, 4321);
        assert_eq!(second.borrow().as_ref().unwrap().ui.status_refresh_ms, 4321);
        for _ in 0..10 {
            let _current = first.borrow().clone();
            let _current = second.borrow().clone();
        }
        assert_eq!(
            files
                .calls()
                .iter()
                .filter(|call| matches!(call, crate::testing::fakes::FakeFilesCall::Read(path) if path == config.path()))
                .count(),
            reads_after_update
        );
    }

    #[tokio::test]
    async fn set_config_refreshes_deletion_roots() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let old_repos = home.join("repos");
        let new_repos = home.join("new-repos");
        let files = Arc::new(FakeFiles::new(
            home.join("trash"),
            vec![old_repos.clone(), home.join("worktrees")],
        ));
        let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        let services = Services::new(
            home,
            config,
            Arc::new(StateStore::new(home, files.clone(), clock)),
            Arc::new(JobManager::new(home)),
            Adapters::system(files.clone()),
        );

        services
            .dispatch(RequestBody::SetConfig {
                patch: serde_json::json!({"reposDir": new_repos}),
            })
            .await
            .unwrap();

        assert!(
            files
                .guard_strict_descendant(&old_repos.join("owner/repo"))
                .is_err()
        );
        assert!(
            files
                .guard_strict_descendant(&new_repos.join("owner/repo"))
                .is_err(),
            "a configured root must not bypass missing-parent validation"
        );
        files
            .create_dir_all(&new_repos.join("owner"))
            .unwrap_or_else(|error| panic!("create configured root: {error}"));
        assert!(
            files
                .guard_strict_descendant(&new_repos.join("owner/repo"))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn context_membership_mutations_share_lifecycle_gate() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let files = Arc::new(FakeFiles::new(
            home.join("trash"),
            vec![home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        let services = Arc::new(Services::new(
            home,
            config,
            Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock))),
            Arc::new(JobManager::new(home)),
            Adapters::system(files),
        ));
        let lifecycle = services.repos.context_lifecycle();
        let guard = lifecycle.lock().await;
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(2);

        let moving = {
            let services = Arc::clone(&services);
            let ready = ready_tx.clone();
            tokio::spawn(async move {
                ready.send(()).await.unwrap();
                services
                    .repos
                    .move_to_context(
                        RepoId::try_from("acme/api").unwrap(),
                        ContextId::try_from("next").unwrap(),
                    )
                    .await
            })
        };
        let deleting = {
            let services = Arc::clone(&services);
            tokio::spawn(async move {
                ready_tx.send(()).await.unwrap();
                services
                    .dispatch(RequestBody::DeleteContext {
                        id: ContextId::try_from("old").unwrap(),
                    })
                    .await
            })
        };
        ready_rx.recv().await.unwrap();
        ready_rx.recv().await.unwrap();
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(!moving.is_finished());
        assert!(!deleting.is_finished());

        drop(guard);
        assert!(moving.await.unwrap().is_err());
        assert!(deleting.await.unwrap().is_err());
    }

    #[test]
    fn expiry_cannot_unlink_refreshed_cache() {
        let root = Path::new("/cache/prs");
        let path = root.join("acme/fleet.json");
        let files = FakeFiles::new("/trash".into(), vec![]);
        let stale = r#"{"fetchedAt":"2024-01-01T00:00:00Z"}"#;
        let refreshed = r#"{"fetchedAt":"2024-01-02T00:00:00Z"}"#;
        files.insert_text(&path, stale);
        files.replace_before_conditional_remove(&path, refreshed);

        expire_cache_files(
            &files,
            root,
            "2024-01-02T00:00:00Z".parse().unwrap(),
            Duration::from_secs(60),
        )
        .unwrap();

        assert_eq!(files.text(&path).as_deref(), Some(refreshed));
    }

    #[test]
    fn expiry_retains_stale_cache_for_offline_fallback() {
        let root = Path::new("/cache/prs");
        let path = root.join("acme/fleet.json");
        let files = FakeFiles::new("/trash".into(), vec![]);
        files.insert_text(&path, r#"{"fetchedAt":"2024-01-01T00:00:00Z"}"#);
        let retention = pr_cache_retention(Duration::from_secs(60));

        expire_cache_files(
            &files,
            root,
            "2024-01-01T00:01:00Z".parse().unwrap(),
            retention,
        )
        .unwrap();
        assert!(files.exists(&path));

        expire_cache_files(
            &files,
            root,
            "2024-01-01T00:10:00Z".parse().unwrap(),
            retention,
        )
        .unwrap();
        assert!(!files.exists(&path));
    }

    #[test]
    fn expiry_continues_after_entry_failures() {
        let root = Path::new("/cache/prs");
        let files = FakeFiles::new("/trash".into(), vec![]);
        let vanished = root.join("a-vanished.json");
        let unreadable = root.join("b-unreadable.json");
        let removal_failed = root.join("c-removal-failed.json");
        let unreadable_directory = root.join("d-unreadable-directory");
        let nested = unreadable_directory.join("nested.json");
        let healthy = root.join("e-healthy.json");
        let stale = r#"{"fetchedAt":"2024-01-01T00:00:00Z"}"#;
        for path in [&vanished, &unreadable, &removal_failed, &nested, &healthy] {
            files.insert_text(path, stale);
        }
        files.fail_next(
            FakeFilesCall::Metadata(vanished.clone()),
            io::ErrorKind::NotFound,
        );
        files.fail_next(
            FakeFilesCall::Metadata(unreadable.clone()),
            io::ErrorKind::PermissionDenied,
        );
        files.fail_next(
            FakeFilesCall::Remove(removal_failed.clone()),
            io::ErrorKind::PermissionDenied,
        );
        files.fail_next(
            FakeFilesCall::List(unreadable_directory),
            io::ErrorKind::PermissionDenied,
        );

        expire_cache_files(
            &files,
            root,
            "2024-01-02T00:00:00Z".parse().unwrap(),
            Duration::from_secs(60),
        )
        .unwrap();

        assert!(files.exists(&vanished));
        assert!(files.exists(&unreadable));
        assert!(files.exists(&removal_failed));
        assert!(files.exists(&nested));
        assert!(!files.exists(&healthy));
    }

    #[test]
    fn expiry_ignores_temp_and_non_directory() {
        let root = Path::new("/cache/prs");
        let files = FakeFiles::new("/trash".into(), vec![]);
        let temporary = root.join(".fleet.json.tmp-1-token");
        let note = root.join("README");
        let json_directory = root.join("owner.json");
        let stale = json_directory.join("fleet.json");
        files.insert_text(&temporary, "pending");
        files.insert_text(&note, "cache notes");
        files
            .create_dir_all(&json_directory)
            .unwrap_or_else(|error| panic!("{error}"));
        files.insert_text(&stale, r#"{"fetchedAt":"2024-01-01T00:00:00Z"}"#);

        expire_cache_files(
            &files,
            root,
            "2024-01-02T00:00:00Z".parse().unwrap(),
            Duration::from_secs(60),
        )
        .unwrap();

        assert!(files.exists(&temporary));
        assert!(files.exists(&note));
        assert!(files.exists(&json_directory));
        assert!(!files.exists(&stale));
    }
}
