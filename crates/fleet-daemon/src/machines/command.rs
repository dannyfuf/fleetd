//! Advanced argv-prefix machine provider and loopback test transport.

use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use fleet_core::ids::HostId;
use tokio::process::Command;

use super::{
    AsyncDuplex, ChildStream, ExecOutput, MachineAddress, MachineError, MachineProvider,
    ProbeReport,
};

/// Executes every machine argv locally after a configured prefix.
pub struct CommandMachine {
    id: HostId,
    run: Vec<String>,
    fleetd: String,
    fleet_home: Option<String>,
    display: String,
}

impl CommandMachine {
    /// Creates a command-backed provider.
    #[must_use]
    pub fn new(
        id: HostId,
        run: Vec<String>,
        fleetd: String,
        fleet_home: Option<String>,
        display: Option<String>,
    ) -> Self {
        let display = display.unwrap_or_else(|| id.to_string());
        Self {
            id,
            run,
            fleetd,
            fleet_home,
            display,
        }
    }

    fn argv(&self, remote: impl IntoIterator<Item = String>) -> Vec<String> {
        self.run.iter().cloned().chain(remote).collect()
    }
}

#[async_trait]
impl MachineProvider for CommandMachine {
    fn id(&self) -> &HostId {
        &self.id
    }
    fn provider_name(&self) -> &'static str {
        "command"
    }

    async fn resolve(&self) -> Result<MachineAddress, MachineError> {
        Ok(MachineAddress {
            host: self.display.clone(),
            user: None,
            display: self.display.clone(),
            online: None,
        })
    }

    async fn probe(&self, timeout: Duration) -> ProbeReport {
        let started = Instant::now();
        let truth = self.exec(&["true".to_owned()], timeout).await;
        let result = match truth {
            Ok(output) if output.status == 0 => {
                self.exec(&[self.fleetd.clone(), "--version".to_owned()], timeout)
                    .await
            }
            Ok(output) => Err(MachineError::Unreachable(output.stderr)),
            Err(error) => Err(error),
        };
        match result {
            Ok(output) if output.status == 0 => ProbeReport {
                reachable: true,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                version: Some(output.stdout.trim().to_owned()),
                error: None,
                stderr: (!output.stderr.is_empty()).then_some(output.stderr),
            },
            Ok(output) => ProbeReport {
                reachable: false,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                version: None,
                error: Some(format!("exited {}", output.status)),
                stderr: Some(output.stderr),
            },
            Err(error) => ProbeReport {
                reachable: false,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                version: None,
                error: Some(error.to_string()),
                stderr: None,
            },
        }
    }

    async fn exec(&self, argv: &[String], timeout: Duration) -> Result<ExecOutput, MachineError> {
        let argv = self.argv(argv.iter().cloned());
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| MachineError::NotFound("empty command run prefix".to_owned()))?;
        let output = tokio::time::timeout(
            timeout,
            Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| MachineError::Timeout(format!("{} command", self.id)))??;
        let status = output.status.code().unwrap_or(-1);
        ChildStream::check_exit(status, &String::from_utf8_lossy(&output.stderr))?;
        Ok(ExecOutput {
            status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError> {
        let mut remote = vec![self.fleetd.clone(), "connect".to_owned()];
        if let Some(home) = &self.fleet_home {
            remote.extend(["--home".to_owned(), home.clone()]);
        }
        Ok(Box::new(ChildStream::spawn(&self.argv(remote))?))
    }

    fn fleetd_binary(&self) -> &str {
        &self.fleetd
    }
    fn fleet_home(&self) -> Option<&str> {
        self.fleet_home.as_deref()
    }
}
