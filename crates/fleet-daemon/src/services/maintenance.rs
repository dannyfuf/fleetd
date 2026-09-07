use super::*;
use futures_util::{StreamExt, stream};

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
        handles.push(tokio::spawn(run_agent_activity_refresh(
            Arc::clone(self),
            shutdown.clone(),
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

    pub(super) async fn apply_agent_activity_transitions(
        &self,
        transitions: Vec<sessions::AgentActivityTransition>,
    ) -> DaemonResult<()> {
        if transitions.is_empty() {
            return Ok(());
        }
        let statuses = self.sessions.refresh_statuses(None).await?;
        *self.statuses.write().await = Some(statuses);
        for transition in transitions {
            self.events.publish(Event::AgentActivityChanged {
                session: transition.session,
                terminal_id: transition.terminal,
                agent: transition.agent,
                activity: transition.activity,
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
                if let Err(error) = services.sleep.refresh_observations().await {
                    tracing::warn!(%error, "failed to refresh terminal process observations");
                }
                match services.sessions.refresh_statuses(None).await {
                    Ok(statuses) => {
                        *services.statuses.write().await = Some(statuses);
                        events.request_snapshot(Arc::clone(&services));
                    }
                    Err(error) => tracing::warn!(%error, "periodic status refresh failed"),
                }
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
