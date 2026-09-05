//! Command dispatch and daemon client orchestration.

use std::{ffi::OsString, path::PathBuf, str::FromStr, time::Duration};

use clap::{Parser, error::ErrorKind as ClapErrorKind};
use fleet_client::{Client, SpawnError, ensure_daemon, restart_daemon};
use fleet_core::{
    config::Agent,
    ids::{HostId, JobId, RepoId, SessionId, WorktreeId},
    model::{CloneStatus, Repo, RepoHooks, Worktree},
    validate::{validate_branch, validate_slug},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    job::{JobRecord, JobStatus},
    response::SleepResult,
};

use crate::{
    args::{
        AgentChoice, Cli, Command, CreateArgs, DaemonCommand, DeleteArgs, InspectArgs, JsonArgs,
        KillArgs, OpenArgs, PathArgs, PruneArgs, SleepArgs, VERSION_DISPLAY,
    },
    envelope::{
        CreateEnvelope, DeleteEnvelope, InspectEnvelope, ListEnvelope, OkEnvelope, PROTOCOL,
        PruneEnvelope, SleepEnvelope, StatusEnvelope, error_json, single_line, to_json,
    },
    human,
};

const FAILURE: i32 = 1;
const UPDATE_RESTART: i32 = 75;

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
    run_from(std::env::args_os().collect())
}

fn run_from(arguments: Vec<OsString>) -> i32 {
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

async fn run_command(command: Command) -> Result<CommandOutput, ProtoError> {
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
        Command::Doctor => doctor(client).await,
        Command::Import(_) => import_from_swarm(client).await,
        Command::Update => update(client).await,
        Command::Daemon(_) => Err(validation("daemon commands must run before connecting")),
        Command::Version => Ok(CommandOutput::success(VERSION_DISPLAY.to_owned())),
    }
}

async fn create(client: &Client, arguments: CreateArgs) -> Result<CommandOutput, ProtoError> {
    let repo_id = parse_id::<RepoId>(&arguments.repo)?;
    validate_slug(&arguments.slug).map_err(|error| validation(error.to_string()))?;
    if let Some(branch) = &arguments.branch {
        validate_branch(branch).map_err(|error| validation(error.to_string()))?;
    }
    if let Some(default_branch) = &arguments.default_branch {
        validate_branch(default_branch).map_err(|error| validation(error.to_string()))?;
    }
    let host = parse_host(arguments.host.as_deref())?;
    let supplied_hooks = arguments.hooks.as_deref().map(parse_hooks).transpose()?;
    let (mut repo, _registered_now) = ensure_repo(client, &repo_id, &arguments).await?;

    if let Some(hooks) = supplied_hooks {
        repo = client.set_repo_hooks(repo.id.clone(), hooks).await?;
    }
    let hooks = repo.hooks.clone();
    let base = arguments
        .base
        .or_else(|| Some(format!("origin/{}", repo.default_branch)));
    let result = client
        .create_worktree(repo.id, arguments.slug, arguments.branch, base, host, hooks)
        .await?;
    if let Some(job) = &result.post_create_job {
        wait_for_job(client, job, "post-create hooks").await?;
    }
    let text = if arguments.json {
        to_json(&CreateEnvelope {
            protocol: PROTOCOL,
            created: result.created,
            worktree: &result.worktree,
        })?
    } else if result.created {
        format!("Created {}", result.worktree.id)
    } else {
        format!("Existing {}", result.worktree.id)
    };
    Ok(CommandOutput::success(text))
}

async fn ensure_repo(
    client: &Client,
    repo_id: &RepoId,
    arguments: &CreateArgs,
) -> Result<(Repo, bool), ProtoError> {
    let snapshot = client.get_snapshot().await?;
    if let Some(repo) = snapshot.repos.into_iter().find(|repo| &repo.id == repo_id) {
        return Ok((repo, false));
    }
    let url = arguments.url.clone().ok_or_else(|| {
        validation(format!(
            "repository `{repo_id}` is not registered; --url is required"
        ))
    })?;
    let context = if let Some(context) = snapshot
        .contexts
        .iter()
        .find(|context| context.owners.iter().any(|owner| owner == repo_id.owner()))
    {
        context.id.clone()
    } else if let Some(active) = snapshot.active_context {
        active
    } else if let Some(context) = snapshot.contexts.first() {
        context.id.clone()
    } else {
        client
            .create_context(repo_id.owner(), vec![repo_id.owner().to_owned()])
            .await?
            .id
    };

    let _job = client
        .clone_repo(
            repo_id.owner(),
            repo_id.name(),
            url,
            context,
            arguments.default_branch.clone(),
        )
        .await?;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let snapshot = client.get_snapshot().await?;
        if let Some(repo) = snapshot.repos.into_iter().find(|repo| &repo.id == repo_id) {
            return Ok((repo, true));
        }
        if let Some(clone) = snapshot.clones.iter().find(|clone| &clone.id == repo_id)
            && clone.status == CloneStatus::Failed
        {
            return Err(ProtoError {
                kind: ErrorKind::Git,
                message: single_line(clone.error.as_deref().unwrap_or("repository clone failed")),
            });
        }
    }
}

async fn open(client: &Client, arguments: OpenArgs) -> Result<CommandOutput, ProtoError> {
    let snapshot = client.get_snapshot().await?;
    let worktree = resolve_open_target(&snapshot.worktrees, &arguments.target)?;
    let session = client
        .ensure_session(Some(worktree.id.clone()), None, true)
        .await?;
    client.touch_worktree_opened(worktree.id).await?;
    Ok(CommandOutput::success(format!(
        "{}\nOpen it in the Fleet app: fleet",
        session.id
    )))
}

async fn list(client: &Client, arguments: JsonArgs) -> Result<CommandOutput, ProtoError> {
    let snapshot = client.get_snapshot().await?;
    let text = if arguments.json {
        to_json(&ListEnvelope {
            protocol: PROTOCOL,
            version: VERSION_DISPLAY,
            repos: &snapshot.repos,
            worktrees: &snapshot.worktrees,
        })?
    } else {
        human::list(snapshot.repos.len(), snapshot.worktrees.len())
    };
    Ok(CommandOutput::success(text))
}

async fn inspect(client: &Client, arguments: InspectArgs) -> Result<CommandOutput, ProtoError> {
    let ids = parse_ids::<WorktreeId>(&arguments.ids)?;
    let repo = arguments
        .repo
        .as_deref()
        .map(parse_id::<RepoId>)
        .transpose()?;
    let inspections = client.inspect_worktrees(ids, repo, arguments.fetch).await?;
    let text = if arguments.json {
        to_json(&InspectEnvelope {
            protocol: PROTOCOL,
            worktrees: &inspections,
        })?
    } else {
        human::inspect(&inspections)
    };
    Ok(CommandOutput::success(text))
}

async fn delete(client: &Client, arguments: DeleteArgs) -> Result<CommandOutput, ProtoError> {
    let ids = parse_ids::<WorktreeId>(&arguments.ids)?;
    let results = client.delete_worktrees(ids).await?;
    let ok = results.iter().all(|result| result.ok);
    let text = if arguments.json {
        to_json(&DeleteEnvelope {
            protocol: PROTOCOL,
            ok,
            results: &results,
        })?
    } else {
        human::delete(&results)
    };
    Ok(CommandOutput::with_exit_code(
        text,
        if ok { 0 } else { FAILURE },
    ))
}

async fn prune(client: &Client, arguments: PruneArgs) -> Result<CommandOutput, ProtoError> {
    let repo = arguments
        .repo
        .as_deref()
        .map(parse_id::<RepoId>)
        .transpose()?;
    let result = client
        .prune_worktrees(
            arguments.dry_run,
            !arguments.no_fetch,
            arguments.kill_sessions,
            repo,
        )
        .await?;
    let text = if arguments.json {
        to_json(&PruneEnvelope {
            protocol: PROTOCOL,
            dry_run: result.dry_run,
            deleted: &result.deleted,
            skipped: &result.skipped,
        })?
    } else {
        human::prune(&result)
    };
    Ok(CommandOutput::success(text))
}

async fn kill(client: &Client, arguments: KillArgs) -> Result<CommandOutput, ProtoError> {
    let id = parse_id::<WorktreeId>(&arguments.id)?;
    client.kill_worktree(id.clone()).await?;
    let text = if arguments.json {
        to_json(&OkEnvelope {
            protocol: PROTOCOL,
            ok: true,
        })?
    } else {
        format!("Killed {id}")
    };
    Ok(CommandOutput::success(text))
}

async fn status(client: &Client, arguments: JsonArgs) -> Result<CommandOutput, ProtoError> {
    let statuses = client.refresh_statuses(None).await?;
    let text = if arguments.json {
        to_json(&StatusEnvelope {
            protocol: PROTOCOL,
            statuses: &statuses,
        })?
    } else {
        human::status(&statuses)
    };
    Ok(CommandOutput::success(text))
}

async fn path(client: &Client, arguments: PathArgs) -> Result<CommandOutput, ProtoError> {
    let id = parse_id::<WorktreeId>(&arguments.id)?;
    Ok(CommandOutput::success(client.worktree_path(id).await?))
}

async fn sleep(client: &Client, arguments: SleepArgs) -> Result<CommandOutput, ProtoError> {
    let result = resolve_and_sleep(client, arguments.session.as_deref()).await?;
    let text = if arguments.json {
        to_json(&SleepEnvelope {
            protocol: PROTOCOL,
            kept: &result.kept,
            closed: &result.closed,
            session_killed: result.session_killed,
        })?
    } else {
        human::sleep(&result)
    };
    Ok(CommandOutput::success(text))
}

async fn resolve_and_sleep(
    client: &Client,
    target: Option<&str>,
) -> Result<SleepResult, ProtoError> {
    if let Some(target) = target {
        if target.contains('#') {
            return client.sleep_worktree(parse_id::<WorktreeId>(target)?).await;
        }
        return client.sleep_session(parse_id::<SessionId>(target)?).await;
    }
    if let Some(session) = std::env::var_os("FLEET_SESSION") {
        let session = session
            .into_string()
            .map_err(|_| validation("FLEET_SESSION is not valid UTF-8"))?;
        return client.sleep_session(parse_id::<SessionId>(&session)?).await;
    }
    if let Some(session) = client.current_session().await? {
        return client.sleep_session(session).await;
    }
    let sessions = client.list_sessions().await?;
    match sessions.as_slice() {
        [session] => client.sleep_session(session.id.clone()).await,
        [] => Err(ProtoError {
            kind: ErrorKind::NotFound,
            message: "no current or running session".to_owned(),
        }),
        _ => Err(validation(
            "session is required when more than one Fleet session is running",
        )),
    }
}

async fn agent(
    client: &Client,
    requested: Option<AgentChoice>,
) -> Result<CommandOutput, ProtoError> {
    let selected = match requested {
        Some(AgentChoice::Claude) => Agent::Claude,
        Some(AgentChoice::Opencode) => Agent::Opencode,
        None => client.get_config().await?.agent,
    };
    let session = client.ensure_session(None, Some(selected), false).await?;
    Ok(CommandOutput::success(format!(
        "{}\nOpen it in the Fleet app: fleet",
        session.id
    )))
}

async fn doctor(client: &Client) -> Result<CommandOutput, ProtoError> {
    let checks = client.doctor().await?;
    let ok = checks
        .iter()
        .all(|check| check.status != fleet_proto::response::DoctorStatus::Fail);
    Ok(CommandOutput::with_exit_code(
        human::doctor(&checks),
        if ok { 0 } else { FAILURE },
    ))
}

async fn import_from_swarm(client: &Client) -> Result<CommandOutput, ProtoError> {
    let job = client.import_from_swarm().await?;
    Ok(CommandOutput::success(format!("Import started {}", job.id)))
}

async fn update(client: &Client) -> Result<CommandOutput, ProtoError> {
    let job = client.update().await?;
    wait_for_job(client, &job, "update").await?;
    Ok(CommandOutput::with_exit_code(
        format!("Updated {}", job.id),
        UPDATE_RESTART,
    ))
}

async fn wait_for_job(
    client: &Client,
    initial: &JobRecord,
    operation: &str,
) -> Result<(), ProtoError> {
    let mut status = initial.status.clone();
    loop {
        match status {
            JobStatus::Succeeded => return Ok(()),
            JobStatus::Failed { error } => {
                return Err(unknown(format!("{operation} failed: {error}")));
            }
            JobStatus::Cancelled => {
                return Err(ProtoError {
                    kind: ErrorKind::Cancelled,
                    message: format!("{operation} was cancelled"),
                });
            }
            JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling => {}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        status = client
            .list_jobs()
            .await?
            .into_iter()
            .find(|job| job.id == initial.id)
            .ok_or_else(|| missing_job(&initial.id))?
            .status;
    }
}

fn missing_job(id: &JobId) -> ProtoError {
    ProtoError {
        kind: ErrorKind::NotFound,
        message: format!("update job `{id}` is no longer available"),
    }
}

fn resolve_open_target(worktrees: &[Worktree], target: &str) -> Result<Worktree, ProtoError> {
    let matches = worktrees
        .iter()
        .filter(|worktree| {
            worktree.id.as_str() == target
                || worktree.session == target
                || format!("{}/{}", worktree.repo_id.name(), worktree.slug) == target
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [worktree] => Ok((*worktree).clone()),
        [] => Err(ProtoError {
            kind: ErrorKind::NotFound,
            message: format!("worktree or session `{target}` was not found"),
        }),
        _ => Err(ProtoError {
            kind: ErrorKind::Conflict,
            message: format!("target `{target}` matches more than one worktree"),
        }),
    }
}

fn parse_hooks(source: &str) -> Result<RepoHooks, ProtoError> {
    let value: serde_json::Value = serde_json::from_str(source)
        .map_err(|error| validation(format!("invalid hooks: {error}")))?;
    let Some(object) = value.as_object() else {
        return Err(validation("invalid hooks: expected a JSON object"));
    };
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "prepare" | "postCreate"))
    {
        return Err(validation(format!("invalid hooks: unknown field `{key}`")));
    }
    serde_json::from_value(value).map_err(|error| validation(format!("invalid hooks: {error}")))
}

fn parse_host(host: Option<&str>) -> Result<Option<HostId>, ProtoError> {
    match host {
        None | Some("local") => Ok(None),
        Some(host) => parse_id(host).map(Some),
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

fn fleet_home() -> Result<PathBuf, ProtoError> {
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
        Command::Create(arguments) => arguments.json,
        Command::List(arguments) | Command::Status(arguments) => arguments.json,
        Command::Inspect(arguments) => arguments.json,
        Command::Delete(arguments) => arguments.json,
        Command::Prune(arguments) => arguments.json,
        Command::Kill(arguments) => arguments.json,
        Command::Sleep(arguments) => arguments.json,
        Command::Open(_)
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

fn print_error(error: &ProtoError, json: bool) {
    if json {
        println!("{}", error_json(error));
    } else {
        eprintln!("fleet: {}", error.message);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use fleet_proto::{
        codec::FleetCodec,
        error::{ErrorKind, ProtoError},
        job::{JobKind, JobRecord, JobStatus},
        request::{Request, RequestBody},
        response::{Response, ResponseBody},
    };
    use futures_util::{SinkExt, StreamExt};
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio_util::codec::Framed;

    use super::*;

    type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

    #[tokio::test]
    async fn formats_success_envelope_against_an_in_process_daemon() {
        let home = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let listener = bind(home.path()).await;
        let server = tokio::spawn(async move {
            let (socket, _) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let mut transport = Framed::new(socket, FleetCodec::new());
            authenticate(&mut transport).await;
            let request = next_request(&mut transport).await;
            let RequestBody::KillWorktree { id } = request.body else {
                panic!("expected kill request");
            };
            assert_eq!(id.as_str(), "acme/api#feature");
            send_result(&mut transport, request.id, Ok(ResponseBody::Ack)).await;
        });

        let client = Client::connect(home.path())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let output = execute(
            &client,
            Command::Kill(KillArgs {
                id: "acme/api#feature".to_owned(),
                json: true,
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(output.text, r#"{"protocol":1,"ok":true}"#);
        assert_eq!(output.exit_code, 0);
        server.await.unwrap_or_else(|error| panic!("{error}"));
    }

    #[tokio::test]
    async fn preserves_proto_error_kind_from_an_in_process_daemon() {
        let home = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let listener = bind(home.path()).await;
        let server = tokio::spawn(async move {
            let (socket, _) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let mut transport = Framed::new(socket, FleetCodec::new());
            authenticate(&mut transport).await;
            let request = next_request(&mut transport).await;
            send_result(
                &mut transport,
                request.id,
                Err(ProtoError {
                    kind: ErrorKind::NotFound,
                    message: "missing worktree".to_owned(),
                }),
            )
            .await;
        });

        let client = Client::connect(home.path())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let error = execute(
            &client,
            Command::Kill(KillArgs {
                id: "acme/api#feature".to_owned(),
                json: true,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error_json(&error),
            r#"{"protocol":1,"error":{"kind":"not-found","message":"missing worktree"}}"#
        );
        server.await.unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn maps_parser_and_domain_validation_failures_to_validation_errors() {
        let duplicate = Cli::try_parse_from(["fleet", "list", "--json", "--json"]).unwrap_err();
        assert_eq!(clap_error(&duplicate).kind, ErrorKind::Validation);

        let id_error = parse_id::<WorktreeId>("not-a-worktree").unwrap_err();
        assert_eq!(id_error.kind, ErrorKind::Validation);

        let slug_error = validate_slug("Not Canonical")
            .map_err(|error| validation(error.to_string()))
            .unwrap_err();
        assert_eq!(slug_error.kind, ErrorKind::Validation);

        let hooks_error = parse_hooks(r#"{"prepare":[],"unexpected":true}"#).unwrap_err();
        assert_eq!(hooks_error.kind, ErrorKind::Validation);
    }

    #[tokio::test]
    async fn successful_update_reserves_exit_code_75_for_restart() {
        let home = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let listener = bind(home.path()).await;
        let server = tokio::spawn(async move {
            let (socket, _) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let mut transport = Framed::new(socket, FleetCodec::new());
            authenticate(&mut transport).await;
            let update = next_request(&mut transport).await;
            assert!(matches!(update.body, RequestBody::Update));
            send_result(
                &mut transport,
                update.id,
                Ok(ResponseBody::Job(update_job(JobStatus::Queued))),
            )
            .await;
            let list = next_request(&mut transport).await;
            assert!(matches!(list.body, RequestBody::ListJobs));
            send_result(
                &mut transport,
                list.id,
                Ok(ResponseBody::Jobs(vec![update_job(JobStatus::Succeeded)])),
            )
            .await;
        });

        let client = Client::connect(home.path())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let output = execute(&client, Command::Update)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(output.exit_code, 75);
        assert_eq!(output.text, "Updated update-1");
        server.await.unwrap_or_else(|error| panic!("{error}"));
    }

    fn update_job(status: JobStatus) -> JobRecord {
        JobRecord {
            id: JobId::try_from("update-1").unwrap_or_else(|error| panic!("{error}")),
            kind: JobKind::Update,
            target: "fleet".to_owned(),
            title: "Update Fleet".to_owned(),
            status,
            progress: None,
            log_path: "/tmp/update.log".to_owned(),
            started_at: "2026-09-04T12:00:00Z".to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: false,
        }
    }

    async fn bind(home: &Path) -> UnixListener {
        UnixListener::bind(home.join("fleetd.sock")).unwrap_or_else(|error| panic!("{error}"))
    }

    async fn authenticate(transport: &mut ServerTransport) {
        let hello = next_request(transport).await;
        assert!(matches!(hello.body, RequestBody::Hello { protocol: 1, .. }));
        send_result(
            transport,
            hello.id,
            Ok(ResponseBody::Hello {
                protocol: 1,
                server: "test-daemon".to_owned(),
            }),
        )
        .await;
        let subscribe = next_request(transport).await;
        assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
        send_result(transport, subscribe.id, Ok(ResponseBody::Ack)).await;
    }

    async fn next_request(transport: &mut ServerTransport) -> Request {
        transport
            .next()
            .await
            .unwrap_or_else(|| panic!("connection closed"))
            .unwrap_or_else(|error| panic!("{error}"))
    }

    async fn send_result(
        transport: &mut ServerTransport,
        id: u64,
        result: Result<ResponseBody, ProtoError>,
    ) {
        let response =
            serde_json::to_value(Response { id, result }).unwrap_or_else(|error| panic!("{error}"));
        transport
            .send(response)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
    }
}
