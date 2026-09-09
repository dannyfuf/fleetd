//! Deterministic OpenSSH argv construction.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::MachineError;

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

    /// Creates the control socket directory and restricts it to the current user.
    pub async fn ensure_control_dir(&self) -> Result<(), MachineError> {
        let directory = self.control_dir()?;
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(directory).await?;
        tokio::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).await?;
        Ok(())
    }

    /// Builds the local process argv for a remote argv.
    #[must_use]
    pub fn build(&self, remote: &[String]) -> Vec<String> {
        let mut argv = vec![
            "ssh".to_owned(),
            "-o".to_owned(),
            "BatchMode=yes".to_owned(),
            "-o".to_owned(),
            "ConnectTimeout=10".to_owned(),
            "-o".to_owned(),
            "ControlMaster=auto".to_owned(),
            "-o".to_owned(),
            "ControlPersist=300".to_owned(),
            "-o".to_owned(),
            format!("ControlPath={}", self.control_path.display()),
        ];
        argv.extend(self.options.iter().cloned());
        argv.extend([
            "--".to_owned(),
            self.destination.clone(),
            quote_remote_argv(remote),
        ]);
        argv
    }

    fn control_dir(&self) -> Result<&Path, MachineError> {
        self.control_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| {
                MachineError::NotFound(format!(
                    "SSH control path has no directory: {}",
                    self.control_path.display()
                ))
            })
    }
}

fn quote_remote_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|argument| format!("'{}'", argument.replace('\'', "'\"'\"'")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_documented_dev_box_argv_exactly() {
        let argv = SshArgv::new("df@dev-box", "/fleet/cache/ssh/%C")
            .options([
                "-o".to_owned(),
                "StrictHostKeyChecking=accept-new".to_owned(),
            ])
            .build(&[
                "fleetd".to_owned(),
                "connect".to_owned(),
                "--home".to_owned(),
                "~/.fleet".to_owned(),
            ]);

        assert_eq!(
            argv,
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPersist=300",
                "-o",
                "ControlPath=/fleet/cache/ssh/%C",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "--",
                "df@dev-box",
                "'fleetd' 'connect' '--home' '~/.fleet'",
            ]
        );
    }

    #[test]
    fn quotes_empty_spaces_and_single_quotes_for_the_remote_shell() {
        let argv = SshArgv::new("host", "/fleet/cache/ssh/%C").build(&[
            String::new(),
            "two words".to_owned(),
            "it's".to_owned(),
        ]);

        assert_eq!(
            argv.last().map(String::as_str),
            Some("'' 'two words' 'it'\"'\"'s'")
        );
    }

    #[tokio::test]
    async fn creates_private_control_directory() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let directory = temp.path().join("cache/ssh");
        let argv = SshArgv::new("host", directory.join("%C"));

        argv.ensure_control_dir()
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let mode = std::fs::metadata(directory)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
