//! Deterministic OpenSSH argv construction.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::MachineError;

/// Seconds between OpenSSH keepalive probes on an otherwise idle link.
///
/// Fleet dials a resolved address rather than a `Host` alias, so the user's `ssh_config`
/// never applies and `ServerAliveInterval` would default to 0 — a link the peer dropped
/// stays open locally until a write fails, which for `fleetd connect` can be minutes.
const SERVER_ALIVE_INTERVAL_SECS: u32 = 15;

/// Unanswered keepalive probes tolerated before OpenSSH tears the link down.
///
/// Four probes at [`SERVER_ALIVE_INTERVAL_SECS`] detect a dead peer inside a minute, which
/// is short enough for the link to reconnect and long enough to survive a roaming Wi-Fi hop.
const SERVER_ALIVE_COUNT_MAX: u32 = 4;

/// Seconds OpenSSH waits for the TCP connect before giving up.
///
/// A short connect timeout matters because sshd penalises a source address that exceeds
/// `LoginGraceTime`; a hung dial that is never abandoned is what earns that penalty.
const CONNECT_TIMEOUT_SECS: u32 = 10;

/// The non-interactive connection options every Fleet `ssh` invocation carries.
///
/// Callers that do not build a full [`SshArgv`] — the legacy probe paths — use this so one
/// list governs keepalive and timeout behaviour across the daemon. It is the concatenation
/// of [`required_options`] and [`tunable_options`]; `SshArgv` emits the two halves on either
/// side of the host's `sshOptions` because only one half may be overridden.
#[must_use]
pub(crate) fn connection_defaults() -> Vec<String> {
    let mut options = required_options();
    options.extend(tunable_options());
    options
}

/// The options a host must not be able to override.
///
/// `BatchMode=yes` is what keeps a daemon-spawned `ssh` from blocking forever on a password
/// or passphrase prompt it has no terminal to show. The `Control*` triple must also stay
/// Fleet's: [`SshArgv::ensure_control_dir`] creates and locks down exactly the directory
/// named here, so a host-supplied `ControlPath` would point the multiplexer at a directory
/// nobody prepared — silently degrading to one connection per command, or worse, a socket in
/// a world-writable place.
#[must_use]
fn required_options() -> Vec<String> {
    dashed(["BatchMode=yes".to_owned()])
}

/// The connection timings a host may override through `sshOptions`.
#[must_use]
fn tunable_options() -> Vec<String> {
    dashed([
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
        format!("ServerAliveInterval={SERVER_ALIVE_INTERVAL_SECS}"),
        format!("ServerAliveCountMax={SERVER_ALIVE_COUNT_MAX}"),
    ])
}

/// Prefixes every option with the `-o` that introduces it.
fn dashed(options: impl IntoIterator<Item = String>) -> Vec<String> {
    options
        .into_iter()
        .flat_map(|option| ["-o".to_owned(), option])
        .collect()
}

/// Builder for the OpenSSH command used by machine providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshArgv {
    destination: String,
    identity_file: Option<PathBuf>,
    options: Vec<String>,
    control_path: PathBuf,
}

impl SshArgv {
    /// Creates a builder with Fleet's non-interactive connection defaults.
    #[must_use]
    pub fn new(destination: impl Into<String>, control_path: impl Into<PathBuf>) -> Self {
        Self {
            destination: destination.into(),
            identity_file: None,
            options: Vec::new(),
            control_path: control_path.into(),
        }
    }

    /// Pins authentication to one private key, expanding a leading `~`.
    ///
    /// `None` changes nothing: without a configured key Fleet must keep offering every
    /// default identity and the agent, which is the only thing that works for agent-only
    /// users. With a key, `IdentitiesOnly=yes` stops OpenSSH walking the other identities,
    /// which is what exhausts sshd's `LoginGraceTime` and gets the source address penalised.
    #[must_use]
    pub fn identity_file(mut self, identity_file: Option<impl Into<PathBuf>>) -> Self {
        self.identity_file = identity_file.map(fleet_core::paths::expand_tilde);
        self
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
    ///
    /// Order is precedence, not cosmetics: OpenSSH keeps the *first* value it sees for an
    /// option, so position decides who wins. Fleet's non-negotiable options lead —
    /// `BatchMode` and the `Control*` triple, which the daemon's own correctness depends on.
    /// The host's `sshOptions` come next and beat everything after them, which is the
    /// tunables: the configured identity and the connection timings.
    #[must_use]
    pub fn build(&self, remote: &[String]) -> Vec<String> {
        let mut argv = vec!["ssh".to_owned()];
        argv.extend(required_options());
        argv.extend([
            "-o".to_owned(),
            "ControlMaster=auto".to_owned(),
            "-o".to_owned(),
            "ControlPersist=300".to_owned(),
            "-o".to_owned(),
            format!("ControlPath={}", self.control_path.display()),
        ]);
        argv.extend(self.options.iter().cloned());
        if let Some(identity_file) = &self.identity_file {
            argv.extend([
                "-o".to_owned(),
                "IdentitiesOnly=yes".to_owned(),
                "-i".to_owned(),
                identity_file.display().to_string(),
            ]);
        }
        argv.extend(tunable_options());
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
                "ControlMaster=auto",
                "-o",
                "ControlPersist=300",
                "-o",
                "ControlPath=/fleet/cache/ssh/%C",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=4",
                "--",
                "df@dev-box",
                "'fleetd' 'connect' '--home' '~/.fleet'",
            ]
        );
    }

    /// A host must not be able to turn off the two things the daemon's correctness rests on:
    /// a non-interactive `ssh`, and the control socket `ensure_control_dir` actually prepared.
    #[test]
    fn configured_options_cannot_override_batch_mode_or_the_control_socket() {
        let argv = SshArgv::new("host", "/fleet/cache/ssh/%C")
            .options(dashed([
                "BatchMode=no".to_owned(),
                "ControlPath=/tmp/attacker/%C".to_owned(),
                "ControlMaster=no".to_owned(),
                "ControlPersist=no".to_owned(),
            ]))
            .build(&["true".to_owned()]);

        // OpenSSH keeps the first value it sees, so Fleet's must come first for each of them.
        for (option, fleet_value) in [
            ("BatchMode=", "BatchMode=yes"),
            ("ControlPath=", "ControlPath=/fleet/cache/ssh/%C"),
            ("ControlMaster=", "ControlMaster=auto"),
            ("ControlPersist=", "ControlPersist=300"),
        ] {
            let first = argv
                .iter()
                .find(|argument| argument.starts_with(option))
                .map(String::as_str);
            assert_eq!(first, Some(fleet_value), "{argv:?}");
        }
    }

    #[test]
    fn keepalive_defaults_exist_on_every_invocation_and_yield_to_configured_options() {
        // A link that is never probed is only discovered dead on the next write, which for a
        // `fleetd connect` stream can be minutes after the peer went away.
        let argv = SshArgv::new("host", "/fleet/cache/ssh/%C").build(&["true".to_owned()]);
        let pairs = argv
            .windows(2)
            .filter(|pair| pair[0] == "-o")
            .map(|pair| pair[1].clone())
            .collect::<Vec<_>>();
        assert!(
            pairs.contains(&"ServerAliveInterval=15".to_owned()),
            "{argv:?}"
        );
        assert!(
            pairs.contains(&"ServerAliveCountMax=4".to_owned()),
            "{argv:?}"
        );
        assert!(pairs.contains(&"ConnectTimeout=10".to_owned()), "{argv:?}");

        // The timings are tunable: they sit behind `sshOptions`, so a host that sets its own
        // interval wins, which is the whole point of the per-host escape hatch.
        let overridden = SshArgv::new("host", "/fleet/cache/ssh/%C")
            .options(dashed([
                "ServerAliveInterval=60".to_owned(),
                "ConnectTimeout=30".to_owned(),
            ]))
            .build(&["true".to_owned()]);
        for (option, host_value) in [
            ("ServerAliveInterval=", "ServerAliveInterval=60"),
            ("ConnectTimeout=", "ConnectTimeout=30"),
        ] {
            let first = overridden
                .iter()
                .find(|argument| argument.starts_with(option))
                .map(String::as_str);
            assert_eq!(first, Some(host_value), "{overridden:?}");
        }
    }

    #[test]
    fn a_configured_identity_pins_authentication_and_no_identity_changes_nothing() {
        let argv = SshArgv::new("df@dev-box", "/fleet/cache/ssh/%C")
            .identity_file(Some("/keys/id_ed25519"))
            .build(&["true".to_owned()]);
        let identity = argv
            .windows(2)
            .position(|pair| pair[0] == "-i" && pair[1] == "/keys/id_ed25519");
        assert!(identity.is_some(), "{argv:?}");
        assert!(argv.contains(&"IdentitiesOnly=yes".to_owned()), "{argv:?}");

        // Agent-only users must keep offering every default identity.
        let without = SshArgv::new("df@dev-box", "/fleet/cache/ssh/%C")
            .identity_file(None::<String>)
            .build(&["true".to_owned()]);
        assert!(
            !without.contains(&"IdentitiesOnly=yes".to_owned()),
            "{without:?}"
        );
        assert!(!without.contains(&"-i".to_owned()), "{without:?}");
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
