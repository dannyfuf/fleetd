//! Tailscale machine provider skeleton.

use std::time::Duration;

use async_trait::async_trait;
use fleet_core::ids::HostId;

use super::{
    AsyncDuplex, ExecOutput, MachineAddress, MachineError, MachineProvider, ProbeReport,
    provider::not_implemented,
};

/// A configured Tailscale peer. Transport behavior is implemented by the transport stage.
pub struct TailscaleMachine {
    id: HostId,
    node: String,
    user: Option<String>,
    ssh_options: Vec<String>,
    fleetd: String,
    fleet_home: Option<String>,
}

impl TailscaleMachine {
    /// Creates a provider from the public host configuration fields.
    #[must_use]
    pub fn new(
        id: HostId,
        node: String,
        user: Option<String>,
        ssh_options: Vec<String>,
        fleetd: String,
        fleet_home: Option<String>,
    ) -> Self {
        Self {
            id,
            node,
            user,
            ssh_options,
            fleetd,
            fleet_home,
        }
    }

    /// Returns additional OpenSSH options in configuration order.
    #[must_use]
    pub fn ssh_options(&self) -> &[String] {
        &self.ssh_options
    }
}

#[async_trait]
impl MachineProvider for TailscaleMachine {
    fn id(&self) -> &HostId {
        &self.id
    }
    fn provider_name(&self) -> &'static str {
        "tailscale"
    }

    async fn resolve(&self) -> Result<MachineAddress, MachineError> {
        let _ = (&self.node, &self.user);
        Err(not_implemented("TailscaleMachine::resolve"))
    }

    async fn probe(&self, _timeout: Duration) -> ProbeReport {
        ProbeReport {
            reachable: false,
            latency_ms: None,
            version: None,
            error: Some(not_implemented("TailscaleMachine::probe").to_string()),
            stderr: None,
        }
    }

    async fn exec(&self, _argv: &[String], _timeout: Duration) -> Result<ExecOutput, MachineError> {
        Err(not_implemented("TailscaleMachine::exec"))
    }

    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError> {
        Err(not_implemented("TailscaleMachine::open_stream"))
    }

    fn fleetd_binary(&self) -> &str {
        &self.fleetd
    }
    fn fleet_home(&self) -> Option<&str> {
        self.fleet_home.as_deref()
    }
}
