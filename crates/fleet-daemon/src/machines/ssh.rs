//! Deterministic OpenSSH argv construction skeleton.

use std::path::PathBuf;

/// Builder for the OpenSSH command used by machine providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshArgv {
    destination: String,
    options: Vec<String>,
    control_path: PathBuf,
}

impl SshArgv {
    /// Creates a builder with Fleet's non-interactive connection defaults.
    #[must_use]
    pub fn new(destination: impl Into<String>, control_path: impl Into<PathBuf>) -> Self {
        Self {
            destination: destination.into(),
            options: Vec::new(),
            control_path: control_path.into(),
        }
    }

    /// Appends configured OpenSSH arguments.
    #[must_use]
    pub fn options(mut self, options: impl IntoIterator<Item = String>) -> Self {
        self.options.extend(options);
        self
    }

    /// Builds the local process argv for a remote argv.
    #[must_use]
    pub fn build(&self, remote: &[String]) -> Vec<String> {
        let _ = remote;
        vec![
            "ssh".to_owned(),
            "-o".to_owned(),
            "BatchMode=yes".to_owned(),
            "-o".to_owned(),
            format!("ControlPath={}", self.control_path.display()),
            self.destination.clone(),
        ]
    }
}
