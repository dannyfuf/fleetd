//! Legacy swarm-over-SSH probe-only provider.

use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use fleet_core::ids::HostId;
use tokio::process::Command;

use super::{AsyncDuplex, ExecOutput, MachineAddress, MachineError, MachineProvider, ProbeReport};

/// Compatibility provider for the former `{ssh, swarmCommand}` schema.
pub struct LegacyMachine {
    id: HostId,
    ssh: String,
    swarm_command: String,
}

impl LegacyMachine {
    #[must_use]
    pub fn new(id: HostId, ssh: String, swarm_command: String) -> Self {
        Self {
            id,
            ssh,
            swarm_command,
        }
    }
}

#[async_trait]
impl MachineProvider for LegacyMachine {
    fn id(&self) -> &HostId {
        &self.id
    }
    fn provider_name(&self) -> &'static str {
        "legacy"
    }

    async fn resolve(&self) -> Result<MachineAddress, MachineError> {
        Ok(MachineAddress {
            host: self.ssh.clone(),
            user: None,
            display: self.ssh.clone(),
            online: None,
        })
    }

    async fn probe(&self, timeout: Duration) -> ProbeReport {
        let started = Instant::now();
        let result = tokio::time::timeout(
            timeout,
            Command::new("ssh")
                .args(super::ssh::connection_defaults())
                .arg("--")
                .arg(&self.ssh)
                .arg(&self.swarm_command)
                .args(["list", "--json"])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match result {
            Ok(Ok(output)) if output.status.success() => {
                let value: serde_json::Value =
                    serde_json::from_slice(&output.stdout).unwrap_or_default();
                ProbeReport {
                    reachable: value.get("protocol").and_then(serde_json::Value::as_u64) == Some(1),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                    version: value
                        .get("version")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    error: None,
                    stderr: Some(String::from_utf8_lossy(&output.stderr).into_owned())
                        .filter(|value| !value.is_empty()),
                }
            }
            Ok(Ok(output)) => ProbeReport {
                reachable: false,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                version: None,
                error: Some(format!("ssh exited {}", output.status)),
                stderr: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
            },
            Ok(Err(error)) => ProbeReport {
                reachable: false,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                version: None,
                error: Some(error.to_string()),
                stderr: None,
            },
            Err(_) => ProbeReport {
                reachable: false,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                version: None,
                error: Some("legacy probe timed out".to_owned()),
                stderr: None,
            },
        }
    }

    async fn exec(&self, _argv: &[String], _timeout: Duration) -> Result<ExecOutput, MachineError> {
        Err(MachineError::Unsupported(
            "legacy machine is probe-only".to_owned(),
        ))
    }

    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError> {
        Err(MachineError::Unsupported(
            "legacy machine is probe-only".to_owned(),
        ))
    }

    fn fleetd_binary(&self) -> &str {
        &self.swarm_command
    }
    fn fleet_home(&self) -> Option<&str> {
        None
    }
}
