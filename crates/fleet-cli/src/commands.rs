//! Command dispatch and daemon client orchestration.

mod agents;
mod board;
mod hosts;
mod jobs;
mod sessions;
mod watches;
mod worktrees;

use crate::{
    args::{AgentCommand, Cli, Command, DaemonCommand, VERSION_DISPLAY, WatchArgs, WatchCommand},
    envelope::{error_json, single_line},
};
use board::board;
use clap::{Parser, error::ErrorKind as ClapErrorKind};
use fleet_client::{Client, SpawnError, ensure_daemon, restart_daemon};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
};
use jobs::{doctor, import_from_swarm, update};
use sessions::{agent_status, sleep};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};
use tokio::sync::broadcast::{self, error::RecvError};
use watches::{watch_list, watch_session, watch_tail};
use worktrees::{create, delete, inspect, kill, list, open, path, prune, status};

const FAILURE: i32 = 1;
const UPDATE_RESTART: i32 = 75;
/// How long a wait stays event-driven before reconciling through a query API.
const QUIET_RECONCILE: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
struct CommandOutput {
    text: String,
    exit_code: i32,
}

impl CommandOutput {
    fn success(text: String) -> Self {
        Self { text, exit_code: 0 }
    }

    fn with_exit_code(text: String, exit_code: i32) -> Self {
        Self { text, exit_code }
    }
}

/// Runs Fleet's command-line interface and returns the process exit code.
#[must_use]
pub fn run() -> i32 {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    run_from(
        std::env::args_os().collect::<Vec<_>>(),
        &mut stdout,
        &mut stderr,
    )
}

fn run_from(arguments: Vec<OsString>, stdout: &mut impl Write, stderr: &mut impl Write) -> i32 {
    let json_requested = arguments.iter().any(|argument| argument == "--json");
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
            ) =>
        {
            return stdout_result(write!(stdout, "{error}"), 0, stderr);
        }
        Err(error) => {
            let error = clap_error(&error);
            return error_result(
                print_error(&error, json_requested, stdout, stderr),
                json_requested,
            );
        }
    };

    if cli.version {
        return stdout_result(writeln!(stdout, "{VERSION_DISPLAY}"), 0, stderr);
    }

    let Some(command) = cli.command else {
        let error = validation("a command is required");
        return error_result(
            print_error(&error, json_requested, stdout, stderr),
            json_requested,
        );
    };
    let command = match command {
        Command::Exec(args) => return crate::exec::run(args),
        Command::WatchChild(args) => return crate::exec::child(args),
        other => other,
    };
    if matches!(command, Command::Version) {
        return stdout_result(writeln!(stdout, "{VERSION_DISPLAY}"), 0, stderr);
    }
    let json_requested = command_requests_json(&command);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let error = unknown(format!("could not initialize CLI runtime: {error}"));
            return error_result(
                print_error(&error, json_requested, stdout, stderr),
                json_requested,
            );
        }
    };

    match runtime.block_on(run_command(command)) {
        Ok(output) => {
            if output.text.is_empty() {
                output.exit_code
            } else {
                stdout_result(
                    writeln!(stdout, "{}", output.text),
                    output.exit_code,
                    stderr,
                )
            }
        }
        Err(error) => error_result(
            print_error(&error, json_requested, stdout, stderr),
            json_requested,
        ),
    }
}

async fn run_command(mut command: Command) -> Result<CommandOutput, ProtoError> {
    if let Command::Watch(WatchArgs {
        command: WatchCommand::List(arguments),
    }) = &mut command
    {
        arguments.session = Some(watch_session(
            arguments.session.take(),
            std::env::var("FLEET_SESSION").ok().as_deref(),
        )?);
    }
    let home = fleet_home()?;
    if matches!(
        command,
        Command::Daemon(crate::args::DaemonArgs {
            command: DaemonCommand::Restart
        })
    ) {
        let _client = restart_daemon(&home, None).await.map_err(spawn_error)?;
        return Ok(CommandOutput::success("Restarted fleetd".to_owned()));
    }
    let client = ensure_daemon(&home, None).await.map_err(spawn_error)?;
    execute(&client, command).await
}

async fn execute(client: &Client, command: Command) -> Result<CommandOutput, ProtoError> {
    match command {
        Command::Board(arguments) => board(client, arguments).await,
        Command::Host(arguments) => hosts::run(client, arguments).await,
        Command::Exec(_) | Command::WatchChild(_) => {
            Err(validation("exec must run before daemon autostart"))
        }
        Command::Watch(arguments) => match arguments.command {
            WatchCommand::List(arguments) => watch_list(client, arguments).await,
            WatchCommand::Tail(arguments) => watch_tail(client, arguments).await,
        },
        Command::Agent(arguments) => match arguments.command {
            AgentCommand::List => agents::list(client).await,
            AgentCommand::New(arguments) => agents::new(client, arguments).await,
            AgentCommand::Send(arguments) => agents::send(client, arguments).await,
            AgentCommand::Respond(arguments) => agents::respond(client, arguments).await,
            AgentCommand::Interrupt(arguments) => agents::interrupt(client, arguments).await,
            AgentCommand::Stop(arguments) => agents::stop(client, arguments).await,
            AgentCommand::Tail(arguments) => agents::tail(client, arguments).await,
            // The PTY popup keeps a CLI surface: it is the documented fallback whenever a
            // native provider reports itself unavailable (`NATIVE-AGENTS.md` §10).
            AgentCommand::Terminal(arguments) => sessions::agent(client, arguments.agent).await,
        },
        Command::Create(arguments) => create(client, arguments).await,
        Command::Open(arguments) => open(client, arguments).await,
        Command::List(arguments) => list(client, arguments).await,
        Command::Inspect(arguments) => inspect(client, arguments).await,
        Command::Delete(arguments) => delete(client, arguments).await,
        Command::Prune(arguments) => prune(client, arguments).await,
        Command::Kill(arguments) => kill(client, arguments).await,
        Command::Status(arguments) => status(client, arguments).await,
        Command::Path(arguments) => path(client, arguments).await,
        Command::Sleep(arguments) => sleep(client, arguments).await,
        Command::AgentStatus(arguments) => agent_status(client, arguments).await,
        Command::Doctor(arguments) => doctor(client, arguments).await,
        Command::Import(_) => import_from_swarm(client).await,
        Command::Update => update(client).await,
        Command::Daemon(_) => Err(validation("daemon commands must run before connecting")),
        Command::Version => Ok(CommandOutput::success(VERSION_DISPLAY.to_owned())),
    }
}

fn parse_ids<T>(values: &[String]) -> Result<Vec<T>, ProtoError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    values.iter().map(|value| parse_id(value)).collect()
}

fn parse_id<T>(value: &str) -> Result<T, ProtoError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|error| validation(error.to_string()))
}

pub(crate) fn fleet_home() -> Result<PathBuf, ProtoError> {
    if let Some(home) = std::env::var_os("FLEET_HOME") {
        return Ok(PathBuf::from(home));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".fleet"))
        .ok_or_else(|| validation("HOME is not set; set FLEET_HOME"))
}

fn command_requests_json(command: &Command) -> bool {
    match command {
        Command::Board(arguments) => arguments.json,
        Command::Host(arguments) => matches!(
            arguments.command,
            crate::args::HostCommand::List { json: true }
        ),
        Command::Watch(arguments) => {
            matches!(&arguments.command, WatchCommand::List(arguments) if arguments.json)
        }
        Command::Create(arguments) => arguments.json,
        Command::List(arguments) | Command::Status(arguments) => arguments.json,
        Command::Inspect(arguments) => arguments.json,
        Command::Delete(arguments) => arguments.json,
        Command::Prune(arguments) => arguments.json,
        Command::Kill(arguments) => arguments.json,
        Command::Sleep(arguments) => arguments.json,
        Command::AgentStatus(arguments) => arguments.json,
        Command::Doctor(arguments) => arguments.json,
        Command::Exec(_)
        | Command::WatchChild(_)
        | Command::Open(_)
        | Command::Path(_)
        | Command::Agent(_)
        | Command::Import(_)
        | Command::Update
        | Command::Daemon(_)
        | Command::Version => false,
    }
}

fn validation(message: impl AsRef<str>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Validation,
        message: single_line(message.as_ref()),
    }
}

fn clap_error(error: &clap::Error) -> ProtoError {
    validation(error.to_string())
}

fn unknown(message: impl AsRef<str>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: single_line(message.as_ref()),
    }
}

fn spawn_error(error: SpawnError) -> ProtoError {
    match error {
        SpawnError::Protocol(error) => error,
        SpawnError::Io(error) => ProtoError {
            kind: ErrorKind::Fs,
            message: single_line(&format!("could not start Fleet daemon: {error}")),
        },
        SpawnError::ProcessAlive { pid } => ProtoError {
            kind: ErrorKind::Conflict,
            message: format!("Fleet daemon process {pid} is alive but its socket is unavailable"),
        },
        other => unknown(other.to_string()),
    }
}

/// Waits for a matching event, yielding `None` on lag or after 30 quiet seconds so callers
/// reconcile through their query APIs: reconnects have no event of their own.
async fn wait_event(
    events: &mut broadcast::Receiver<Event>,
    matches: impl Fn(&Event) -> bool,
) -> Result<Option<Event>, ProtoError> {
    let receive = async {
        loop {
            match events.recv().await {
                Ok(Event::DaemonShuttingDown) | Err(RecvError::Closed) => {
                    return Err(unknown("Fleet daemon connection is closed"));
                }
                Ok(event) if matches(&event) => return Ok(Some(event)),
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => return Ok(None),
            }
        }
    };
    match tokio::time::timeout(QUIET_RECONCILE, receive).await {
        Ok(result) => result,
        Err(_) => Ok(None),
    }
}

fn print_error(
    error: &ProtoError,
    json: bool,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<()> {
    if json {
        writeln!(stdout, "{}", error_json(error))
    } else {
        writeln!(stderr, "fleet: {}", error.message)
    }
}

fn stdout_result(result: io::Result<()>, exit_code: i32, stderr: &mut impl Write) -> i32 {
    match result {
        Ok(()) => exit_code,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(error) => {
            let _ = writeln!(stderr, "fleet: could not write output: {error}");
            FAILURE
        }
    }
}

fn error_result(result: io::Result<()>, written_to_stdout: bool) -> i32 {
    match result {
        Ok(()) => FAILURE,
        Err(error) if written_to_stdout && error.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(_) => FAILURE,
    }
}

#[cfg(test)]
mod tests;
