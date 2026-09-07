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
        let boards = Arc::new(boards::Boards::new(
            Arc::new(crate::stores::board::BoardStore::new(
                fleet_core::paths::FleetHome::new(home.clone()),
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
            inventory: Arc::new(tokio::sync::Mutex::new(snapshots::InventoryCache::default())),
            pool_refreshed_at: Arc::new(RwLock::new(BTreeMap::new())),
            events,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{adapters::clock::SystemClock, adapters::files::RealFiles};

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
}
