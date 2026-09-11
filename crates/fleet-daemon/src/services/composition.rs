use super::*;

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

    pub(super) fn build(
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
            Arc::clone(&adapters.process),
        );
        let sessions =
            Sessions::new(Arc::clone(&config), Arc::clone(&state)).with_events(events.clone());
        let worktrees = Worktrees::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            &adapters,
            sessions.clone(),
        );
        let fleet_home = fleet_core::paths::FleetHome::new(home.clone());
        let agents = agents::AgentSessionManager::new(
            fleet_home.agents_db_path(),
            events.clone(),
            worktrees.clone(),
            Arc::clone(&config),
        );
        // Checkpoints are Git-only: the ref namespace is their whole store, so the service needs
        // the process boundary and nothing else — no database handle, no event bus, no state.
        let checkpoints = checkpoints::Checkpoints::new(Arc::clone(&adapters.shell));
        // The manager takes the capture side: the turn identifier is minted inside its `send`,
        // and it already holds the `Worktrees` a capture needs to resolve a path
        // (`docs/NATIVE-AGENTS.md` §13 phase 8).
        agents.set_checkpoints(checkpoints.clone());
        let boards = Arc::new(boards::Boards::new(
            Arc::new(crate::stores::board::BoardStore::new(
                fleet_home,
                Arc::clone(&adapters.files),
            )),
            Arc::clone(&state),
            adapters.board_backends.clone(),
            Arc::clone(&adapters.clock),
            Arc::clone(&jobs),
            Arc::new(worktrees.clone()),
            events.clone(),
        ));
        let pool = Pool::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.files),
            Arc::clone(&adapters.shell),
        );
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
            &sessions,
        );
        let inspect = Inspect::new(
            Arc::clone(&state),
            Arc::clone(&jobs),
            Arc::clone(&adapters.git),
            Arc::clone(&adapters.github),
            sessions.clone(),
        );
        let prune = Prune::new(
            Arc::clone(&jobs),
            inspect.clone(),
            sessions.clone(),
            Arc::new(worktrees.clone()),
        );
        let doctor = Doctor::new(
            Arc::clone(&config),
            Arc::clone(&adapters.shell),
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
            update::runtime_checkout(),
        );
        let hosts = Hosts::new(home.clone(), Arc::clone(&adapters.shell));
        let local_daemon_id = load_or_create_daemon_id(&home);
        let daemon_id = local_daemon_id.to_string();
        let machines = Arc::new(
            Machines::from_config_with_runtime(
                &default_config(&home),
                home.clone(),
                Arc::clone(&adapters.shell),
            )
            .with_local_daemon_id(local_daemon_id),
        );
        let mirror = Arc::new(mirror::Mirror::new());
        let router = Arc::new(router::Router::with_ids(
            Arc::clone(&machines),
            Arc::clone(&mirror),
            router::RemoteIds::new(sessions.terminal_id_counter()),
        ));
        agents.set_remote_host_resolver(Arc::new({
            let router = Arc::clone(&router);
            move |id| router::Resolver::host_of_worktree(router.as_ref(), id)
        }));
        // The durable read-through mirror of the threads other hosts own
        // (`docs/NATIVE-AGENTS.md` §9.3). The ownership census goes in first, so a mirrored
        // thread routes upstream from this daemon's very first request rather than only after
        // the owner's first snapshot has arrived.
        router.adopt_mirrored_threads(agents.mirrored_threads());
        router.set_agent_mirror(Arc::new(agents.clone()));
        let bootstrap = Arc::new(bootstrap::Bootstrap::with_registry(
            Arc::clone(&jobs),
            Arc::clone(&machines),
            bootstrap_origin_url(),
            option_env!("FLEET_BUILD_COMMIT").map(str::to_owned),
        ));
        let watches = sessions.watches();
        let watch_discovery = watch_discovery::WatchDiscovery::new(
            Arc::clone(&config),
            sessions.clone(),
            Arc::clone(&adapters.process),
            watches.clone(),
        );
        Self {
            boards,
            hosts,
            machines,
            mirror,
            router,
            bootstrap,
            home,
            started_at: chrono::Utc::now().to_rfc3339(),
            daemon_id,
            contexts: Contexts::new(Arc::clone(&state)),
            repos,
            worktrees,
            agents,
            checkpoints,
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
            inventory: Arc::new(tokio::sync::Mutex::new(snapshots::InventoryCache::default())),
            pool_refreshed_at: Arc::new(RwLock::new(BTreeMap::new())),
            events,
        }
    }
}

fn bootstrap_origin_url() -> String {
    let checkout = update::runtime_checkout();
    std::process::Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{adapters::clock::SystemClock, adapters::files::RealFiles};

    /// Collects formatted `tracing` output so a log-only failure path can be asserted.
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl CapturedLogs {
        fn text(&self) -> String {
            String::from_utf8_lossy(
                &self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
            .into_owned()
        }
    }

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    /// Runs `work` with every event it logs captured instead of printed.
    fn capturing_logs<T>(work: impl FnOnce() -> T) -> (T, String) {
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish();
        let value = tracing::subscriber::with_default(subscriber, work);
        (value, logs.text())
    }

    #[test]
    fn services_build_uses_runtime_update_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let services = Services::new(
            home,
            Arc::new(ConfigStore::new(home, files.clone())),
            Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock))),
            Arc::new(JobManager::new(home)),
            Adapters::system(files),
        );

        assert_eq!(services.update.checkout(), update::runtime_checkout());
    }

    #[test]
    fn an_invalid_daemon_id_file_is_quarantined_instead_of_aborting_startup() {
        let temp = tempfile::tempdir().expect("temp home");
        let home = temp.path();
        // `local` is a reserved host id, so this file cannot become the daemon identity.
        std::fs::write(home.join("daemon-id"), "local\n").expect("write daemon id");
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));

        let services = Services::new(
            home,
            Arc::new(ConfigStore::new(home, files.clone())),
            Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock))),
            Arc::new(JobManager::new(home)),
            Adapters::system(files),
        );

        assert_ne!(services.daemon_id(), "local");
        assert!(HostId::try_from(services.daemon_id()).is_ok());
        assert_eq!(
            std::fs::read_to_string(home.join("daemon-id.invalid"))
                .expect("the rejected identity is kept for diagnosis")
                .trim(),
            "local"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("daemon-id"))
                .expect("daemon id file")
                .trim(),
            services.daemon_id()
        );
    }

    #[test]
    fn a_daemon_id_that_cannot_be_read_or_written_still_starts_the_daemon() {
        let temp = tempfile::tempdir().expect("temp home");
        let home = temp.path();
        // A directory where the identity file belongs: neither the read nor the write can
        // succeed, and startup must survive both rather than abort or hand out an invalid id.
        std::fs::create_dir(home.join("daemon-id")).expect("occupy the identity path");
        let build = || {
            let files = Arc::new(RealFiles::new(
                home.join("trash"),
                [home.join("repos"), home.join("worktrees")],
            ));
            Services::new(
                home,
                Arc::new(ConfigStore::new(home, files.clone())),
                Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock))),
                Arc::new(JobManager::new(home)),
                Adapters::system(files),
            )
        };
        let ((first, second), logs) = capturing_logs(|| (build(), build()));
        assert!(HostId::try_from(first.daemon_id()).is_ok());
        assert_ne!(
            first.daemon_id(),
            second.daemon_id(),
            "an identity that cannot be persisted cannot be stable, which is what the warning says"
        );
        // The failed write is the whole reason the identity is unstable, so it has to be
        // reported rather than discarded: this is the only place that failure is observable.
        assert!(
            logs.contains("could not persist the daemon identity"),
            "{logs}"
        );

        // Control: the same startup on a writable home says nothing, so the assertion above is
        // reading the failed write and not a warning the daemon logs unconditionally.
        let writable = tempfile::tempdir().expect("temp home");
        let home = writable.path();
        let (_services, logs) = capturing_logs(|| {
            let files = Arc::new(RealFiles::new(
                home.join("trash"),
                [home.join("repos"), home.join("worktrees")],
            ));
            Services::new(
                home,
                Arc::new(ConfigStore::new(home, files.clone())),
                Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock))),
                Arc::new(JobManager::new(home)),
                Adapters::system(files),
            )
        });
        assert!(
            !logs.contains("could not persist the daemon identity"),
            "{logs}"
        );
    }

    #[test]
    fn daemon_id_is_stable_for_one_fleet_home() {
        let temp = tempfile::tempdir().expect("temp home");
        let home = temp.path();
        let build = || {
            let files = Arc::new(RealFiles::new(
                home.join("trash"),
                [home.join("repos"), home.join("worktrees")],
            ));
            Services::new(
                home,
                Arc::new(ConfigStore::new(home, files.clone())),
                Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock))),
                Arc::new(JobManager::new(home)),
                Adapters::system(files),
            )
        };
        let first = build();
        let second = build();
        assert_eq!(first.daemon_id(), second.daemon_id());
        assert_eq!(
            std::fs::read_to_string(home.join("daemon-id"))
                .expect("daemon id file")
                .trim(),
            first.daemon_id()
        );
    }
}
