//! Configured machine-provider and endpoint registry.

use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use fleet_core::{config::Config, ids::HostId, model::HostConfigEntry};

use super::{
    CommandMachine, LegacyMachine, LinkOptions, MachineProvider, RemoteEndpoint, RemoteLink,
    TailscaleMachine,
};

/// Thread-safe registry of configured machine providers and lazy daemon endpoints.
pub struct Machines {
    providers: RwLock<BTreeMap<HostId, Arc<dyn MachineProvider>>>,
    endpoints: RwLock<BTreeMap<HostId, Arc<dyn RemoteEndpoint>>>,
}

impl Machines {
    /// Builds providers for every configured host.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self {
            providers: RwLock::new(build_providers(config)),
            endpoints: RwLock::new(BTreeMap::new()),
        }
    }

    /// Atomically replaces providers and drops endpoints for removed or changed hosts.
    pub fn rebuild(&self, config: &Config) {
        *write(&self.providers) = build_providers(config);
        write(&self.endpoints).clear();
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
        let endpoint: Arc<dyn RemoteEndpoint> = RemoteLink::new(provider, LinkOptions::default());
        write(&self.endpoints).insert(host.clone(), Arc::clone(&endpoint));
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

fn build_providers(config: &Config) -> BTreeMap<HostId, Arc<dyn MachineProvider>> {
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
                } => Arc::new(TailscaleMachine::new(
                    id.clone(),
                    node.clone(),
                    user.clone(),
                    ssh_options.clone(),
                    fleetd.clone(),
                    fleet_home.clone(),
                )),
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
}
