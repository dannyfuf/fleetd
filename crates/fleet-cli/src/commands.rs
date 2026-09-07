//! Command dispatch and daemon client orchestration.

mod jobs;
mod sessions;
mod watches;
mod worktrees;

use crate::{
    args::{Cli, Command, DaemonCommand, VERSION_DISPLAY, WatchArgs, WatchCommand},
    envelope::{error_json, single_line},
};
use clap::{Parser, error::ErrorKind as ClapErrorKind};
use fleet_client::{Client, SpawnError, ensure_daemon, restart_daemon};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
};
use jobs::{doctor, import_from_swarm, update};
use sessions::{agent, agent_status, sleep};
use std::{path::PathBuf, str::FromStr, time::Duration};
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
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let json_requested = arguments.iter().any(|argument| argument == "--json");
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return 0;
        }
        Err(error) => {
            let error = clap_error(&error);
            print_error(&error, json_requested);
            return FAILURE;
        }
    };

    if cli.version {
        println!("{VERSION_DISPLAY}");
        return 0;
    }

    let Some(command) = cli.command else {
        let error = validation("a command is required");
        print_error(&error, json_requested);
        return FAILURE;
    };
    let command = match command {
        Command::Exec(args) => return crate::exec::run(args),
        Command::WatchChild(args) => return crate::exec::child(args),
        other => other,
    };
    if matches!(command, Command::Version) {
        println!("{VERSION_DISPLAY}");
        return 0;
    }
    let json_requested = command_requests_json(&command);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let error = unknown(format!("could not initialize CLI runtime: {error}"));
            print_error(&error, json_requested);
            return FAILURE;
        }
    };

    match runtime.block_on(run_command(command)) {
        Ok(output) => {
            if !output.text.is_empty() {
                println!("{}", output.text);
            }
            output.exit_code
        }
        Err(error) => {
            print_error(&error, json_requested);
            FAILURE
        }
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
        Command::Exec(_) | Command::WatchChild(_) => {
            Err(validation("exec must run before daemon autostart"))
        }
        Command::Watch(arguments) => match arguments.command {
            WatchCommand::List(arguments) => watch_list(client, arguments).await,
            WatchCommand::Tail(arguments) => watch_tail(client, arguments).await,
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
        Command::Agent(arguments) => agent(client, arguments.agent).await,
        Command::AgentStatus(arguments) => agent_status(client, arguments).await,
        Command::Doctor => doctor(client).await,
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
        Command::Exec(_)
        | Command::WatchChild(_)
        | Command::Open(_)
        | Command::Path(_)
        | Command::Agent(_)
        | Command::Doctor
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

fn print_error(error: &ProtoError, json: bool) {
    if json {
        println!("{}", error_json(error));
    } else {
        eprintln!("fleet: {}", error.message);
    }
}

#[cfg(test)]
mod tests;
