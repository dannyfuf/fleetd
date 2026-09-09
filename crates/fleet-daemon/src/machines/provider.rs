//! Provider-neutral machine contracts.

use std::{io, time::Duration};

use async_trait::async_trait;
use fleet_core::ids::HostId;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::DaemonError;

/// Resolved address and display metadata for a configured machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineAddress {
    pub host: String,
    pub user: Option<String>,
    pub display: String,
    pub online: Option<bool>,
}

/// Result of a bounded machine health probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub reachable: bool,
    pub latency_ms: Option<u64>,
    pub version: Option<String>,
    pub error: Option<String>,
    pub stderr: Option<String>,
}

/// Captured result of a non-interactive machine command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Tokio byte stream used to speak the Fleet protocol to another daemon.
pub trait AsyncDuplex: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> AsyncDuplex for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

/// A configured way to resolve and communicate with one machine.
#[async_trait]
pub trait MachineProvider: Send + Sync {
    fn id(&self) -> &HostId;
    fn provider_name(&self) -> &'static str;
    async fn resolve(&self) -> Result<MachineAddress, MachineError>;
    async fn probe(&self, timeout: Duration) -> ProbeReport;
    async fn exec(&self, argv: &[String], timeout: Duration) -> Result<ExecOutput, MachineError>;
    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError>;
    fn fleetd_binary(&self) -> &str;
    fn fleet_home(&self) -> Option<&str>;
    /// Non-fatal provider warning from the most recent resolution attempt.
    fn warning(&self) -> Option<String> {
        None
    }
}

/// Optional provisioning lifecycle implemented by future machine providers.
#[async_trait]
pub trait MachineLifecycle: Send + Sync {
    async fn ensure_up(&self) -> Result<(), MachineError>;
    async fn shutdown(&self) -> Result<(), MachineError>;
}

/// Stable machine-provider failure categories.
#[derive(Debug, thiserror::Error)]
pub enum MachineError {
    #[error("unreachable: {0}")]
    Unreachable(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("timed out: {0}")]
    Timeout(String),
}

impl From<MachineError> for DaemonError {
    fn from(error: MachineError) -> Self {
        match error {
            MachineError::Unreachable(message) | MachineError::Auth(message) => {
                Self::Remote(message)
            }
            MachineError::NotFound(message) => Self::NotFound(message),
            MachineError::Protocol(message) => Self::Protocol(message),
            MachineError::Unsupported(message) => Self::Unsupported(message),
            MachineError::Io(error) => Self::Remote(error.to_string()),
            MachineError::Timeout(message) => Self::Timeout(message),
        }
    }
}
