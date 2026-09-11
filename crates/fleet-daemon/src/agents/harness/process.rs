//! Child-process plumbing shared by both harnesses.
//!
//! Both adapters spawn one child, keep stdin open across turns, drain stderr from the moment of
//! spawn (an undrained stderr pipe is a deadlock in both harnesses), treat stdout EOF as process
//! lifetime and never as a turn boundary, and tear down with the same SIGTERM/SIGKILL ladder.
//! That is all this module; the protocols on top of it are per harness.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::{OsStr, OsString},
    path::Path,
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::{Child, Command},
};

use super::{HarnessError, HarnessKind, HarnessResult};

/// How long a child gets to exit on its own before SIGTERM, and after it before SIGKILL.
pub const TERMINATE_GRACE: Duration = Duration::from_secs(2);

/// The streams and process handle of one peer.
///
/// Boxed trait objects rather than `ChildStdin`/`ChildStdout` so a test can drive a transport
/// over in-memory pipes with no interpreter, no binary and no timing luck. `process` is `None`
/// for such a peer, and termination is then whatever closing the streams does.
pub struct PeerStreams {
    /// Where Fleet writes frames.
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    /// Where Fleet reads frames.
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    /// Diagnostic stream, drained from the moment of spawn.
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    /// The child, when there is one.
    pub process: Option<Child>,
}

impl std::fmt::Debug for PeerStreams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PeerStreams")
            .field("has_stderr", &self.stderr.is_some())
            .field("pid", &self.process.as_ref().and_then(Child::id))
            .finish()
    }
}

/// The exit code of a finished child, including one a signal killed.
///
/// `ExitStatus::code()` is `None` for a signalled process, which is exactly the crash the tab
/// must identify (`exited 137` in red). `128 + signal` is the shell's own spelling of it.
#[must_use]
pub fn exit_code(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt as _;

    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
}

/// Splits a configured command line into its program and its leading arguments.
pub fn command_parts(command_line: &str) -> HarnessResult<(OsString, Vec<OsString>)> {
    let parts = shell_words::split(command_line).map_err(|error| HarnessError::Unavailable {
        reason: format!("The configured agent command could not be parsed: {error}"),
    })?;
    let mut parts = parts.into_iter();
    let program = parts.next().ok_or_else(|| HarnessError::Unavailable {
        reason: "The configured agent command is empty.".to_owned(),
    })?;
    Ok((OsString::from(program), parts.map(OsString::from).collect()))
}

/// Tokenizes user-supplied launch arguments with POSIX-ish quoting.
///
/// Appended **last** by both adapters so a user override wins (spec A.2.2). `shell_words` is the
/// same tokenizer the configured command line uses, so the two agree on quoting.
pub fn tokenize_user_args(args: &str) -> HarnessResult<Vec<String>> {
    if args.trim().is_empty() {
        return Ok(Vec::new());
    }
    shell_words::split(args).map_err(|error| HarnessError::Unavailable {
        reason: format!("The configured agent launch arguments could not be parsed: {error}"),
    })
}

/// Resolves a bare program name against `PATH` from the child's own environment.
///
/// The daemon may be launched from a GUI with a minimal `PATH`, so resolving against the
/// environment the child will actually get is what makes `claude` findable at all.
#[must_use]
pub fn resolve_program(program: OsString, environment: &HashMap<OsString, OsString>) -> OsString {
    let path = Path::new(&program);
    if path.components().count() > 1 {
        return program;
    }
    environment
        .get(&OsString::from("PATH"))
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join(path))
        .find(|candidate| candidate.is_file())
        .map_or(program, std::path::PathBuf::into_os_string)
}

/// The environment of a login shell in `cwd`, falling back to the daemon's own.
///
/// `fleetd` is frequently started by launchd or by a GUI, where `PATH` holds neither Homebrew nor
/// a version manager's shims. Asking the user's own login shell is how the harness binary becomes
/// findable in the first place.
pub async fn login_environment(cwd: &Path) -> HashMap<OsString, OsString> {
    let shell = std::env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("/bin/zsh"));
    let output = Command::new(shell)
        .args(["-l", "-c", "/usr/bin/env -0"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = output else {
        return std::env::vars_os().collect();
    };
    if !output.status.success() {
        return std::env::vars_os().collect();
    }
    let mut environment = HashMap::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
        let Some(separator) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = OsString::from(String::from_utf8_lossy(&entry[..separator]).into_owned());
        let value = OsString::from(String::from_utf8_lossy(&entry[separator + 1..]).into_owned());
        environment.insert(key, value);
    }
    if environment.is_empty() {
        std::env::vars_os().collect()
    } else {
        environment
    }
}

/// Removes the named variables from an inherited environment and applies explicit overrides.
///
/// The removal list is **data**, not code, in both adapters: inherited harness variables silently
/// re-point every thread Fleet starts (spec A.2.2), and a list is testable where a sequence of
/// `remove` calls is not.
pub fn filter_environment(
    mut environment: HashMap<OsString, OsString>,
    strip: &[&str],
    overrides: &BTreeMap<String, String>,
) -> HashMap<OsString, OsString> {
    for key in strip {
        environment.remove(OsStr::new(key));
    }
    for (key, value) in overrides {
        environment.insert(OsString::from(key.clone()), OsString::from(value.clone()));
    }
    environment
}

/// Expands a leading `~` manually.
///
/// `Command::env` performs no shell expansion, so `CODEX_HOME=~/.codex_work` reaches Codex
/// verbatim and trips *"CODEX_HOME points to '~/.codex_work', but that path does not exist"*.
#[must_use]
pub fn expand_home(path: &Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Path::new(&home).join(rest);
    }
    path.to_path_buf()
}

/// Spawns a harness child with piped stdio.
pub async fn spawn_child(
    harness: HarnessKind,
    command_line: &str,
    args: &[String],
    cwd: &Path,
    environment: HashMap<OsString, OsString>,
) -> HarnessResult<PeerStreams> {
    let (program, base_args) = command_parts(command_line)?;
    let mut command = Command::new(resolve_program(program, &environment));
    command
        .args(base_args)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(&environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        let name = harness.display_name();
        let binary = harness.executable();
        if error.kind() == std::io::ErrorKind::NotFound {
            HarnessError::Unavailable {
                reason: format!("{name} (`{binary}`) was not found on PATH."),
            }
        } else {
            HarnessError::Unavailable {
                reason: format!("{name} is installed but failed to run: {error}."),
            }
        }
    })?;
    let missing = |what: &str| HarnessError::Unavailable {
        reason: format!("{} did not provide piped {what}.", harness.display_name()),
    };
    let stdin = child.stdin.take().ok_or_else(|| missing("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    Ok(PeerStreams {
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
        stderr: Some(Box::new(stderr)),
        process: Some(child),
    })
}

/// Runs `<command> --version` and returns its stdout.
pub async fn read_version(
    harness: HarnessKind,
    command_line: &str,
    cwd: &Path,
    environment: &HashMap<OsString, OsString>,
    deadline: Duration,
) -> HarnessResult<String> {
    let (program, base_args) = command_parts(command_line)?;
    let mut command = Command::new(resolve_program(program, environment));
    command
        .args(base_args)
        .arg("--version")
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let name = harness.display_name();
    let binary = harness.executable();
    let output = tokio::time::timeout(deadline, command.output())
        .await
        .map_err(|_| HarnessError::Timeout {
            what: "the agent version probe",
            after: deadline,
        })?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                HarnessError::Unavailable {
                    reason: format!("{name} (`{binary}`) was not found on PATH."),
                }
            } else {
                HarnessError::Unavailable {
                    reason: format!("{name} is installed but failed to run: {error}."),
                }
            }
        })?;
    if !output.status.success() {
        return Err(HarnessError::Unavailable {
            reason: format!("{name} is installed but failed to run."),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Closes the child down: wait, SIGTERM, then SIGKILL.
///
/// Called only after the adapter has written whatever the protocol owes the harness (a gate
/// response, an interrupt), because a response interrupted mid-flight leaves the peer waiting on
/// a socket that is already dead.
pub async fn terminate(child: &mut Child, grace: Duration) -> Option<i32> {
    if let Ok(Ok(status)) = tokio::time::timeout(grace, child.wait()).await {
        return exit_code(&status);
    }
    if let Some(pid) = child.id() {
        let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if sent != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                tracing::warn!(
                    target: "fleet::agents",
                    %error,
                    "could not signal the harness child; escalating to a kill"
                );
            }
        }
    }
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(Ok(status)) => exit_code(&status),
        Ok(Err(error)) => {
            tracing::warn!(target: "fleet::agents", %error, "could not reap the harness child");
            None
        }
        Err(_) => {
            if let Err(error) = child.start_kill() {
                tracing::warn!(target: "fleet::agents", %error, "could not kill the harness child");
                return None;
            }
            match child.wait().await {
                Ok(status) => exit_code(&status),
                Err(error) => {
                    tracing::warn!(
                        target: "fleet::agents",
                        %error,
                        "could not reap the killed harness child"
                    );
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signalled_child_reports_the_shells_own_spelling_of_its_death() {
        use std::os::unix::process::ExitStatusExt as _;

        let killed = std::process::ExitStatus::from_raw(9);
        assert_eq!(killed.code(), None);
        assert_eq!(exit_code(&killed), Some(137));
        assert_eq!(
            exit_code(&std::process::ExitStatus::from_raw(0x0100)),
            Some(1)
        );
    }

    #[test]
    fn the_strip_list_is_data_and_overrides_win() {
        let inherited = HashMap::from([
            (OsString::from("CLAUDE_EFFORT"), OsString::from("high")),
            (OsString::from("PATH"), OsString::from("/usr/bin")),
        ]);
        let overrides = BTreeMap::from([("PATH".to_owned(), "/opt/bin".to_owned())]);
        let filtered = filter_environment(inherited, &["CLAUDE_EFFORT"], &overrides);
        assert!(!filtered.contains_key(OsStr::new("CLAUDE_EFFORT")));
        assert_eq!(
            filtered.get(OsStr::new("PATH")),
            Some(&OsString::from("/opt/bin"))
        );
    }

    #[test]
    fn user_launch_args_tokenize_with_shell_quoting() {
        assert_eq!(
            tokenize_user_args(r#"--model 'claude opus' --flag="a b""#)
                .unwrap_or_else(|error| panic!("{error}")),
            ["--model", "claude opus", "--flag=a b"]
        );
        assert!(tokenize_user_args("   ").is_ok_and(|args| args.is_empty()));
        assert!(tokenize_user_args("'unterminated").is_err());
    }

    #[tokio::test]
    async fn a_wedged_child_that_ignores_sigterm_is_still_killed() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|error| panic!("spawn the wedged child: {error}"));
        // 137 is SIGKILL's own spelling; a child that ignored the term would otherwise hang here.
        let code = terminate(&mut child, Duration::from_millis(150)).await;
        assert_eq!(code, Some(137));
    }
}
