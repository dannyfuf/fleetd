//! Configured machine-provider and endpoint registry.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use fleet_core::{config::Config, ids::HostId, model::HostConfigEntry};

use crate::adapters::shell::{RealShell, Shell};

use super::{
    CommandMachine, LegacyMachine, LinkOptions, MachineProvider, RemoteEndpoint, RemoteLink,
    TailscaleMachine,
};

/// Thread-safe registry of configured machine providers and lazy daemon endpoints.
pub struct Machines {
    configured: RwLock<BTreeMap<HostId, HostConfigEntry>>,
    providers: RwLock<BTreeMap<HostId, Arc<dyn MachineProvider>>>,
    endpoints: RwLock<BTreeMap<HostId, Arc<dyn RemoteEndpoint>>>,
    local_home: PathBuf,
    shell: Arc<dyn Shell>,
    local_daemon_id: HostId,
}

impl Machines {
    /// Builds providers for every configured host.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self::from_config_with_runtime(
            config,
            PathBuf::from(&config.worktrees_dir),
            Arc::new(RealShell),
        )
    }

    /// Builds providers using the active daemon home and shell adapter.
    #[must_use]
    pub fn from_config_with_runtime(
        config: &Config,
        home: impl Into<PathBuf>,
        shell: Arc<dyn Shell>,
    ) -> Self {
        let local_home = home.into();
        Self {
            configured: RwLock::new(config.hosts.clone()),
            providers: RwLock::new(build_providers(config, &local_home, &shell)),
            endpoints: RwLock::new(BTreeMap::new()),
            local_home,
            shell,
            local_daemon_id: HostId::try_from("local-daemon").expect("static host id is valid"),
        }
    }

    /// Supplies the local daemon identity used by proxy Hello handshakes.
    #[must_use]
    pub fn with_local_daemon_id(mut self, daemon_id: HostId) -> Self {
        self.local_daemon_id = daemon_id;
        self
    }

    /// Atomically replaces providers and closes endpoints for removed or changed hosts.
    pub fn rebuild(&self, config: &Config) {
        let previous = read(&self.configured).clone();
        if previous == config.hosts {
            return;
        }
        *write(&self.configured) = config.hosts.clone();
        *write(&self.providers) = build_providers(config, &self.local_home, &self.shell);
        let mut closing = Vec::new();
        write(&self.endpoints).retain(|host, endpoint| {
            let keep = config.hosts.get(host).is_some_and(|entry| {
                previous
                    .get(host)
                    .is_none_or(|previous_entry| previous_entry == entry)
            });
            if !keep {
                closing.push(Arc::clone(endpoint));
            }
            keep
        });
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            for endpoint in closing {
                runtime.spawn(async move { endpoint.close().await });
            }
        }
    }

    #[must_use]
    pub fn get(&self, host: &HostId) -> Option<Arc<dyn MachineProvider>> {
        read(&self.providers).get(host).cloned()
    }

    #[must_use]
    pub fn iter(&self) -> Vec<(HostId, Arc<dyn MachineProvider>)> {
        read(&self.providers)
            .iter()
            .map(|(host, provider)| (host.clone(), Arc::clone(provider)))
            .collect()
    }

    /// Returns or lazily creates a remote endpoint for a non-legacy provider.
    #[must_use]
    pub fn endpoint(&self, host: &HostId) -> Option<Arc<dyn RemoteEndpoint>> {
        if let Some(endpoint) = read(&self.endpoints).get(host).cloned() {
            return Some(endpoint);
        }
        let provider = self.get(host)?;
        if provider.provider_name() == "legacy" {
            return None;
        }
        // One endpoint per host is the invariant every `Arc::ptr_eq` check in the mirror and the
        // router relies on, so the lookup and the insert happen under one guard: two concurrent
        // callers would otherwise each build a link, and the loser's actor and ssh child would
        // keep running with nothing left holding them to close.
        let mut endpoints = write(&self.endpoints);
        let (endpoint, created) = match endpoints.entry(host.clone()) {
            Entry::Occupied(existing) => (Arc::clone(existing.get()), None),
            Entry::Vacant(slot) => {
                let link = RemoteLink::new_with_daemon_id(
                    provider,
                    self.local_daemon_id.clone(),
                    LinkOptions::default(),
                );
                let endpoint: Arc<dyn RemoteEndpoint> = link.clone();
                slot.insert(Arc::clone(&endpoint));
                (endpoint, Some(link))
            }
        };
        drop(endpoints);
        if let Some(link) = created
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                if let Err(error) = link.connect().await {
                    tracing::debug!(%error, "initial remote link connection failed");
                }
            });
        }
        Some(endpoint)
    }

    /// Installs a scripted endpoint for tests.
    pub fn install_endpoint(&self, host: HostId, endpoint: Arc<dyn RemoteEndpoint>) {
        write(&self.endpoints).insert(host, endpoint);
    }

    #[must_use]
    pub fn endpoints(&self) -> Vec<(HostId, Arc<dyn RemoteEndpoint>)> {
        read(&self.endpoints)
            .iter()
            .map(|(host, endpoint)| (host.clone(), Arc::clone(endpoint)))
            .collect()
    }
}

fn build_providers(
    config: &Config,
    local_home: &std::path::Path,
    shell: &Arc<dyn Shell>,
) -> BTreeMap<HostId, Arc<dyn MachineProvider>> {
    config
        .hosts
        .iter()
        .map(|(id, entry)| {
            let provider: Arc<dyn MachineProvider> = match entry {
                HostConfigEntry::Tailscale {
                    node,
                    user,
                    ssh_options,
                    fleetd,
                    fleet_home,
                } => Arc::new(
                    TailscaleMachine::new(
                        id.clone(),
                        node.clone(),
                        user.clone(),
                        ssh_options.clone(),
                        fleetd.clone(),
                        fleet_home.clone(),
                    )
                    .with_runtime(
                        local_home.to_path_buf(),
                        Duration::from_millis(
                            u64::try_from(config.ui.remote_status_refresh_ms).unwrap_or(500),
                        ),
                        Arc::clone(shell),
                    ),
                ),
                HostConfigEntry::Command {
                    run,
                    fleetd,
                    fleet_home,
                    display,
                } => Arc::new(CommandMachine::new(
                    id.clone(),
                    run.clone(),
                    fleetd.clone(),
                    fleet_home.clone(),
                    display.clone(),
                )),
                HostConfigEntry::Legacy { ssh, swarm_command } => Arc::new(LegacyMachine::new(
                    id.clone(),
                    ssh.clone(),
                    swarm_command.clone(),
                )),
            };
            (id.clone(), provider)
        })
        .collect()
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeMachine, FakeRemote};

    #[test]
    fn builds_all_provider_kinds_and_skips_legacy_endpoints() {
        let mut config = fleet_core::config::default_config("/tmp/fleet");
        let command = HostId::try_from("loopback").expect("host");
        let legacy = HostId::try_from("legacy").expect("host");
        config.hosts.insert(
            command.clone(),
            HostConfigEntry::Command {
                run: vec!["sh".into()],
                fleetd: "fleetd".into(),
                fleet_home: None,
                display: None,
            },
        );
        config.hosts.insert(
            legacy.clone(),
            HostConfigEntry::Legacy {
                ssh: "old".into(),
                swarm_command: "swarm".into(),
            },
        );
        let machines = Machines::from_config(&config);
        assert_eq!(machines.iter().len(), 2);
        assert!(machines.endpoint(&command).is_some());
        assert!(machines.endpoint(&legacy).is_none());
    }

    #[tokio::test]
    async fn rebuild_closes_replaced_endpoints() {
        let mut config = fleet_core::config::default_config("/tmp/fleet");
        let machines = Machines::from_config(&config);
        let host = HostId::try_from("loopback").expect("host");
        let remote = Arc::new(FakeRemote::new(host.clone()));
        let mut states = remote.state_changes();
        machines.install_endpoint(host, remote);

        config.hosts.insert(
            HostId::try_from("replacement").expect("host"),
            HostConfigEntry::Command {
                run: vec!["false".to_owned()],
                fleetd: "fleetd".to_owned(),
                fleet_home: None,
                display: None,
            },
        );

        machines.rebuild(&config);

        tokio::time::timeout(Duration::from_secs(1), async {
            while *states.borrow_and_update() != fleet_proto::snapshot::LinkState::Down {
                states.changed().await.expect("state sender");
            }
        })
        .await
        .expect("rebuild closes the endpoint");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_endpoint_lookups_create_one_link() {
        for _ in 0..8 {
            let host = HostId::try_from("loopback").expect("host");
            let machine = Arc::new(FakeMachine::new(host.clone()));
            let provider: Arc<dyn MachineProvider> = machine.clone();
            let machines = Arc::new(Machines {
                configured: RwLock::new(BTreeMap::new()),
                providers: RwLock::new(BTreeMap::from([(host.clone(), provider)])),
                endpoints: RwLock::new(BTreeMap::new()),
                local_home: PathBuf::from("/tmp/fleet"),
                shell: Arc::new(RealShell),
                local_daemon_id: HostId::try_from("local-daemon").expect("host"),
            });

            let barrier = Arc::new(tokio::sync::Barrier::new(16));
            let mut lookups = Vec::new();
            for _ in 0..16 {
                let machines = Arc::clone(&machines);
                let barrier = Arc::clone(&barrier);
                let host = host.clone();
                lookups.push(tokio::spawn(async move {
                    barrier.wait().await;
                    machines.endpoint(&host).expect("endpoint")
                }));
            }
            let mut endpoints = Vec::new();
            for lookup in lookups {
                endpoints.push(lookup.await.expect("endpoint lookup"));
            }

            let first = Arc::clone(&endpoints[0]);
            for endpoint in &endpoints {
                assert!(
                    Arc::ptr_eq(&first, endpoint),
                    "concurrent lookups must share one endpoint per host"
                );
            }
            assert_eq!(
                machines.endpoints().len(),
                1,
                "the registry must hold one endpoint per host"
            );
            tokio::time::timeout(Duration::from_secs(1), async {
                while machine.stream_opens() == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the single endpoint starts its connection actor");
            assert_eq!(
                machine.stream_opens(),
                1,
                "a second link would open a second stream and leak its actor"
            );
            first.close().await;
        }
    }

    #[tokio::test]
    async fn endpoint_creation_starts_the_remote_link_actor() {
        let host = HostId::try_from("loopback").expect("host");
        let machine = Arc::new(FakeMachine::new(host.clone()));
        let provider: Arc<dyn MachineProvider> = machine.clone();
        let machines = Machines {
            configured: RwLock::new(BTreeMap::new()),
            providers: RwLock::new(BTreeMap::from([(host.clone(), provider)])),
            endpoints: RwLock::new(BTreeMap::new()),
            local_home: PathBuf::from("/tmp/fleet"),
            shell: Arc::new(RealShell),
            local_daemon_id: HostId::try_from("local-daemon").expect("host"),
        };

        let endpoint = machines.endpoint(&host).expect("endpoint");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if machine.take_stream_peer().is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("endpoint starts its connection actor");
        endpoint.close().await;
    }
}
