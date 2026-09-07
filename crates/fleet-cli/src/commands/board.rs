//! Board command orchestration using the typed daemon client.

use fleet_core::{
    board::{
        BackendDescriptor, BackendRef, Board, BoardPatch, BoardView, Card, CardDraft, CardPatch,
        ConflictPolicy, ConflictResolution, Label, Priority, merge_settings, summarize, valid_date,
    },
    ids::{BoardId, ContextId, JobId, LabelId, StatusId},
};

use super::{
    Client, CommandOutput, Duration, ErrorKind, JobRecord, JobStatus, ProtoError, human,
    parse_host, unknown, validation,
};
use crate::{
    args::{
        BoardArgs, BoardCardCommand, BoardCardFields, BoardCommand, BoardConflictPolicy,
        BoardPriority, BoardResolution, BoardSetArgs,
    },
    envelope::{
        BoardBackendSchemaEnvelope, BoardBackendsEnvelope, BoardCardEnvelope, BoardEnvelope,
        BoardListEnvelope, BoardSyncEnvelope, BoardWorktreeEnvelope, OkEnvelope, PROTOCOL, to_json,
    },
};

pub(super) async fn execute(
    client: &Client,
    arguments: BoardArgs,
) -> Result<CommandOutput, ProtoError> {
    let BoardArgs {
        board,
        context,
        json,
        command,
    } = arguments;
    // Clap checks conflicts within a command level, but global flags can be
    // supplied at different levels of the nested card command.
    if board.is_some() && context.is_some() {
        return Err(validation("--board and --context cannot be used together"));
    }
    match command {
        BoardCommand::List => {
            let mut boards = client.list_boards(context).await?;
            if let Some(id) = board {
                // An empty table would read as "that board has nothing", not "no such board".
                if !boards.iter().any(|board| board.id == id) {
                    return Err(ProtoError {
                        kind: ErrorKind::NotFound,
                        message: format!("board `{id}` was not found"),
                    });
                }
                boards.retain(|board| board.id == id);
            }
            let text = if json {
                to_json(&BoardListEnvelope {
                    protocol: PROTOCOL,
                    boards: &boards,
                })?
            } else {
                human::boards(&boards)
            };
            Ok(CommandOutput::success(text))
        }
        BoardCommand::Backends => {
            // Which kinds exist is a property of the daemon, not of any one board.
            let backends = client.list_board_backends().await?;
            let text = if json {
                to_json(&BoardBackendsEnvelope {
                    protocol: PROTOCOL,
                    backends: &backends,
                })?
            } else {
                human::board_backends(&backends)
            };
            Ok(CommandOutput::success(text))
        }
        BoardCommand::Create(arguments) => {
            if board.is_some() {
                return Err(validation("board create accepts --context, not --board"));
            }
            let context = resolve_context(client, context).await?;
            let settings = backend_settings(&serde_json::Value::Null, &arguments.settings)?;
            let view = client
                .create_board(
                    context,
                    arguments.name,
                    arguments.prefix,
                    arguments.backend.map(|kind| BackendRef { kind, settings }),
                )
                .await?;
            let label = if json {
                None
            } else {
                backend_descriptor(client, &view.board.backend.kind).await
            };
            board_show_output(&view, label.as_ref(), json)
        }
        command => {
            let view = resolve_board(client, board, context).await?;
            match command {
                BoardCommand::Show => {
                    // The label lives in the descriptor list, which only the human header
                    // reads: the JSON envelope carries the board verbatim and must not pay
                    // for a second round trip.
                    let label = if json {
                        None
                    } else {
                        backend_descriptor(client, &view.board.backend.kind).await
                    };
                    board_show_output(&view, label.as_ref(), json)
                }
                BoardCommand::Describe => {
                    let schema = client.describe_board_backend(view.board.id.clone()).await?;
                    let text = if json {
                        to_json(&BoardBackendSchemaEnvelope {
                            protocol: PROTOCOL,
                            schema: &schema,
                        })?
                    } else {
                        human::board_backend_schema(&schema)
                    };
                    Ok(CommandOutput::success(text))
                }
                BoardCommand::Set(arguments) => {
                    let patch = board_patch(&view.board, arguments)?;
                    // `card edit` refuses an empty patch for the same reason: a request that
                    // changes nothing still asks the daemon to rewrite the document.
                    if patch.is_empty() {
                        return Err(validation("board set requires at least one field"));
                    }
                    let view = client.update_board(view.board.id, patch).await?;
                    // The same board view `show` prints, so it carries the same header: a
                    // raw `jira` here and a `Jira (acli)` there read as two different things.
                    let label = if json {
                        None
                    } else {
                        backend_descriptor(client, &view.board.backend.kind).await
                    };
                    board_show_output(&view, label.as_ref(), json)
                }
                BoardCommand::Sync { wait, full } => {
                    sync(client, view.board.id, wait, full, json).await
                }
                BoardCommand::Card(arguments) => {
                    card_command(client, &view, arguments.command, json).await
                }
                BoardCommand::List | BoardCommand::Backends | BoardCommand::Create(_) => {
                    unreachable!("handled above")
                }
            }
        }
    }
}

async fn resolve_context(
    client: &Client,
    context: Option<ContextId>,
) -> Result<ContextId, ProtoError> {
    match context {
        Some(context) => Ok(context),
        None => client.get_snapshot().await?.active_context.ok_or_else(|| {
            validation("no active context; select one with --context or use --board")
        }),
    }
}

async fn resolve_board(
    client: &Client,
    board: Option<BoardId>,
    context: Option<ContextId>,
) -> Result<BoardView, ProtoError> {
    match board {
        Some(board) => client.get_board(board).await,
        None => {
            client
                .ensure_board(resolve_context(client, context).await?)
                .await
        }
    }
}

/// The descriptor for `kind`, or `None` when the daemon cannot name it.
///
/// The header takes both its label and the settings it names from here, so no printer has to
/// know one backend's vocabulary. A header is not worth failing a `board show` over: a daemon
/// that refuses the request, or that registers no such kind, leaves the raw kind in place.
async fn backend_descriptor(client: &Client, kind: &str) -> Option<BackendDescriptor> {
    client
        .list_board_backends()
        .await
        .ok()?
        .into_iter()
        .find(|descriptor| descriptor.kind == kind)
}

/// `base` with every `--setting key=value` merged in, or `base` untouched when none were given.
///
/// Values parse as JSON when they are valid JSON and stay strings otherwise, and `null` removes
/// a key — `fleet_core::board::merge_settings` owns that rule so the app agrees with the CLI.
fn backend_settings(
    base: &serde_json::Value,
    pairs: &[(String, String)],
) -> Result<serde_json::Value, ProtoError> {
    if pairs.is_empty() {
        return Ok(base.clone());
    }
    merge_settings(base, pairs).map_err(|error| validation(error.to_string()))
}

/// The backend the `--backend`/`--setting` flags ask for, or `None` when neither was given.
///
/// `--backend` starts from empty settings: the previous kind's keys are meaningless to the new
/// one, and carrying them over would push `project` into a backend that rejects unknown keys.
/// `--setting` alone keeps the kind and merges into what the board already stores.
fn backend_patch(
    board: &Board,
    kind: Option<String>,
    pairs: &[(String, String)],
) -> Result<Option<BackendRef>, ProtoError> {
    if kind.is_none() && pairs.is_empty() {
        return Ok(None);
    }
    let base = if kind.is_some() {
        serde_json::Value::Null
    } else {
        board.backend.settings.clone()
    };
    Ok(Some(BackendRef {
        kind: kind.unwrap_or_else(|| board.backend.kind.clone()),
        settings: backend_settings(&base, pairs)?,
    }))
}

fn board_patch(board: &Board, arguments: BoardSetArgs) -> Result<BoardPatch, ProtoError> {
    let settings = if arguments.start_on_worktree.is_some()
        || arguments.conflict_policy.is_some()
        || arguments.push_new_cards.is_some()
        || arguments.branch_template.is_some()
    {
        let mut settings = board.settings.clone();
        if let Some(start) = arguments.start_on_worktree {
            settings.start_on_worktree = start;
        }
        // The daemon validates the template and `worktree_slug` uses it, but until now no
        // surface could write one: a different branch shape meant editing the board document
        // by hand. The app dialog still omits it by design (UX-SPEC §3.8.6).
        if let Some(template) = arguments.branch_template {
            settings.branch_template = template;
        }
        if let Some(push) = arguments.push_new_cards {
            settings.push_new_cards = push;
        }
        if let Some(policy) = arguments.conflict_policy {
            settings.conflict_policy = match policy {
                BoardConflictPolicy::Manual => ConflictPolicy::Manual,
                BoardConflictPolicy::RemoteWins => ConflictPolicy::RemoteWins,
                BoardConflictPolicy::LocalWins => ConflictPolicy::LocalWins,
            };
        }
        Some(settings)
    } else {
        None
    };
    let labels = board_labels(board, &arguments.add_labels, &arguments.remove_labels)?;
    let backend = backend_patch(board, arguments.backend, &arguments.settings)?;
    Ok(BoardPatch {
        name: arguments.name,
        prefix: arguments.prefix,
        backend,
        default_repo_id: if arguments.clear_default_repo {
            Some(None)
        } else {
            arguments.default_repo.map(Some)
        },
        settings,
        labels,
        ..BoardPatch::default()
    })
}

/// The board's label set with `add` appended and `remove` taken out, or `None` when neither asks.
fn board_labels(
    board: &Board,
    add: &[String],
    remove: &[String],
) -> Result<Option<Vec<Label>>, ProtoError> {
    if add.is_empty() && remove.is_empty() {
        return Ok(None);
    }
    let mut labels = board.labels.clone();
    for value in remove {
        let id = resolve_labels(board, std::slice::from_ref(value))?
            .pop()
            .ok_or_else(|| validation(format!("label `{value}` was not found")))?;
        labels.retain(|label| label.id != id);
    }
    for name in add {
        let name = name.trim();
        if name.is_empty() {
            return Err(validation("a label name must not be empty"));
        }
        // Every other name lookup here is case-insensitive: adding `bug` to a board that has
        // `Bug` must not make a second label that no later `--label` could tell apart.
        if labels
            .iter()
            .any(|label| label.name.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let mut slug = fleet_core::slug::normalize_context_id(name);
        if slug.is_empty() {
            return Err(validation(format!(
                "label `{name}` must contain a letter or number"
            )));
        }
        let base = slug.clone();
        let mut suffix = 2;
        while labels.iter().any(|label| label.id.as_str() == slug) {
            slug = format!("{base}-{suffix}");
            suffix += 1;
        }
        labels.push(Label {
            id: LabelId::try_from(slug).map_err(|error| validation(error.to_string()))?,
            name: name.to_owned(),
            color: None,
        });
    }
    Ok(Some(labels))
}

fn board_show_output(
    view: &BoardView,
    backend: Option<&BackendDescriptor>,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let text = if json {
        to_json(&BoardEnvelope {
            protocol: PROTOCOL,
            board: &view.board,
            cards: &view.cards,
        })?
    } else {
        human::board(view, backend, now_epoch())
    };
    Ok(CommandOutput::success(text))
}

/// Seconds since the Unix epoch, or zero on a clock set before it: the header's `synced` stamp
/// is a courtesy, and a machine whose clock cannot be read still gets its board printed.
fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or_default()
}

fn card_output(
    board: &Board,
    cards: &[Card],
    card: &Card,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let text = if json {
        to_json(&BoardCardEnvelope {
            protocol: PROTOCOL,
            card,
        })?
    } else {
        human::board_card(board, cards, card)
    };
    Ok(CommandOutput::success(text))
}

fn resolve_card<'a>(view: &'a BoardView, key: &str) -> Result<&'a Card, ProtoError> {
    unique_match(
        view.cards.iter().filter(|card| {
            card.id.as_str().eq_ignore_ascii_case(key)
                || card.display_key(&view.board).eq_ignore_ascii_case(key)
                // Only a card with no remote answers to its local key. A board mirroring the
                // Jira project its own prefix names ("SP" over project SP) otherwise has two
                // namespaces of the same shape overlapping: `SP-4` matched both the fourth
                // card and issue SP-4, so most keys became ambiguous and the rest resolved to
                // whichever card happened to be numbered like somebody else's issue.
                || (card.remote.is_none()
                    && card.local_key(&view.board).eq_ignore_ascii_case(key))
        }),
        "card",
        key,
    )
}

fn unique_match<T>(
    mut matches: impl Iterator<Item = T>,
    kind: &str,
    key: &str,
) -> Result<T, ProtoError> {
    let first = matches.next().ok_or_else(|| ProtoError {
        kind: ErrorKind::NotFound,
        message: format!("{kind} `{key}` was not found"),
    })?;
    if matches.next().is_some() {
        return Err(ProtoError {
            kind: ErrorKind::Conflict,
            message: format!("{kind} `{key}` matches more than one entry; use its ID"),
        });
    }
    Ok(first)
}

fn resolve_status(board: &Board, value: &str) -> Result<StatusId, ProtoError> {
    if let Some(status) = board
        .statuses
        .iter()
        .find(|status| status.id.as_str() == value)
    {
        return Ok(status.id.clone());
    }
    unique_match(
        board.statuses.iter().filter(|status| {
            status.id.as_str().eq_ignore_ascii_case(value)
                || status.name.eq_ignore_ascii_case(value)
        }),
        "status",
        value,
    )
    .map(|status| status.id.clone())
}

fn resolve_labels(board: &Board, values: &[String]) -> Result<Vec<LabelId>, ProtoError> {
    let mut labels = Vec::new();
    for value in values {
        if let Some(label) = board.labels.iter().find(|label| label.id.as_str() == value) {
            if !labels.contains(&label.id) {
                labels.push(label.id.clone());
            }
            continue;
        }
        let label = unique_match(
            board.labels.iter().filter(|label| {
                label.id.as_str().eq_ignore_ascii_case(value)
                    || label.name.eq_ignore_ascii_case(value)
            }),
            "label",
            value,
        )?;
        if !labels.contains(&label.id) {
            labels.push(label.id.clone());
        }
    }
    Ok(labels)
}

fn priority(value: BoardPriority) -> Priority {
    match value {
        BoardPriority::Urgent => Priority::Urgent,
        BoardPriority::High => Priority::High,
        BoardPriority::Medium => Priority::Medium,
        BoardPriority::Low => Priority::Low,
        BoardPriority::None => Priority::None,
    }
}

fn card_patch(board: &Board, fields: BoardCardFields) -> Result<CardPatch, ProtoError> {
    // `ops::valid_date` is the one function every surface offering a due date validates with;
    // without it here, the CLI is the only one that learns `2026-02-31` is not a day from a
    // daemon round trip.
    if let Some(due) = fields.due.as_deref()
        && !valid_date(due)
    {
        return Err(validation(format!(
            "`{due}` is not a valid YYYY-MM-DD date"
        )));
    }
    Ok(CardPatch {
        description: fields.desc,
        status_id: fields
            .status
            .as_deref()
            .map(|status| resolve_status(board, status))
            .transpose()?,
        priority: fields.priority.map(priority),
        labels: if fields.clear_labels {
            Some(Vec::new())
        } else if fields.labels.is_empty() {
            None
        } else {
            Some(resolve_labels(board, &fields.labels)?)
        },
        assignee: if fields.clear_assignee {
            Some(None)
        } else {
            fields.assignee.map(Some)
        },
        estimate: if fields.clear_estimate {
            Some(None)
        } else {
            fields.estimate.map(Some)
        },
        due_date: if fields.clear_due {
            Some(None)
        } else {
            fields.due.map(Some)
        },
        repo_id: if fields.clear_repo {
            Some(None)
        } else {
            fields.repo.map(Some)
        },
        ..CardPatch::default()
    })
}

async fn card_command(
    client: &Client,
    view: &BoardView,
    command: BoardCardCommand,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let mut board = view.board.clone();
    let card = match command {
        BoardCardCommand::New { title, fields } => {
            // The contract gives `new` the value flags only: a clear flag would be a silent
            // no-op on a card that has nothing to clear yet.
            if let Some(flag) = fields.clear_flag() {
                return Err(validation(format!(
                    "{flag} applies to `card edit`, not `card new`"
                )));
            }
            let patch = card_patch(&view.board, fields)?;
            client
                .create_card(
                    view.board.id.clone(),
                    CardDraft {
                        title,
                        description: patch.description.unwrap_or_default(),
                        status_id: patch.status_id,
                        priority: patch.priority.unwrap_or_default(),
                        labels: patch.labels.unwrap_or_default(),
                        assignee: patch.assignee.flatten(),
                        estimate: patch.estimate.flatten(),
                        due_date: patch.due_date.flatten(),
                        repo_id: patch.repo_id.flatten(),
                        ..CardDraft::default()
                    },
                )
                .await?
        }
        BoardCardCommand::Show { key } => {
            return card_output(&view.board, &view.cards, resolve_card(view, &key)?, json);
        }
        BoardCardCommand::Edit {
            key,
            title,
            fields,
            archive,
        } => {
            let card = resolve_card(view, &key)?;
            let mut patch = card_patch(&view.board, fields)?;
            patch.title = title;
            patch.archived = archive;
            if patch.is_empty() {
                return Err(validation(
                    "card edit requires at least one field or --archive",
                ));
            }
            client.update_card(card.id.clone(), patch).await?
        }
        BoardCardCommand::Move { key, status, index } => {
            let card = resolve_card(view, &key)?;
            client
                .move_card(
                    card.id.clone(),
                    resolve_status(&view.board, &status)?,
                    index,
                )
                .await?
        }
        BoardCardCommand::Comment { key, body } => {
            client
                .add_card_comment(resolve_card(view, &key)?.id.clone(), body)
                .await?
        }
        BoardCardCommand::Delete { key } => {
            let card = resolve_card(view, &key)?;
            client.delete_card(card.id.clone()).await?;
            let text = if json {
                to_json(&OkEnvelope {
                    protocol: PROTOCOL,
                    ok: true,
                })?
            } else {
                format!("Deleted {}", card.display_key(&view.board))
            };
            return Ok(CommandOutput::success(text));
        }
        BoardCardCommand::Worktree {
            key,
            repo,
            base,
            host,
        } => {
            let card_id = resolve_card(view, &key)?.id.clone();
            let host = parse_host(host.as_deref())?;
            let (card, worktree, created) = client
                .create_worktree_from_card(card_id, repo, base, host)
                .await?;
            let text = if json {
                to_json(&BoardWorktreeEnvelope {
                    protocol: PROTOCOL,
                    created,
                    card: &card,
                    worktree: &worktree,
                })?
            } else if created {
                format!("Created {}", worktree.id)
            } else {
                format!("Existing {}", worktree.id)
            };
            return Ok(CommandOutput::success(text));
        }
        BoardCardCommand::Resolve { key, resolution } => {
            let resolution = match resolution {
                BoardResolution::KeepLocal => ConflictResolution::KeepLocal,
                BoardResolution::TakeRemote => ConflictResolution::TakeRemote,
            };
            let card = client
                .resolve_card_conflict(resolve_card(view, &key)?.id.clone(), resolution)
                .await?;
            // Taking the remote materializes its labels on the board itself. Rendering the
            // card against the snapshot this command opened with would print the new label's
            // slug instead of its name; a refresh that fails leaves that snapshot in place.
            if let Ok(refreshed) = client.get_board(view.board.id.clone()).await {
                board = refreshed.board;
            }
            card
        }
    };
    card_output(&board, &view.cards, &card, json)
}

async fn sync(
    client: &Client,
    board_id: BoardId,
    wait: bool,
    full: bool,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let job_id = client.sync_board(board_id.clone(), full).await?;
    if !wait {
        let text = if json {
            to_json(&BoardSyncEnvelope {
                protocol: PROTOCOL,
                job_id: &job_id,
                job: None,
                summary: None,
            })?
        } else {
            format!("Sync started {job_id}")
        };
        return Ok(CommandOutput::success(text));
    }
    let job = wait_for_sync(client, &job_id).await?;
    let view = client.get_board(board_id).await?;
    let summary = summarize(&view.board, &view.cards);
    match &job.status {
        JobStatus::Cancelled => {
            return Err(ProtoError {
                kind: ErrorKind::Cancelled,
                message: "board sync was cancelled".to_owned(),
            });
        }
        JobStatus::Failed { error } => {
            return Err(unknown(format!(
                "board sync failed: {}",
                summary.last_error.as_deref().unwrap_or(error)
            )));
        }
        _ => {}
    }
    // `last_error` on a job that *succeeded* is this sync's warning, not its failure: the
    // service parks "skipped N remote cards: …" there on a sync that reconciled and pushed
    // fine, and BOARD.md §8 says the job fails only when nothing reconciled. Failing here made
    // one unreadable Jira issue exit 1 out of every scripted `fleet board sync --wait`.
    // `human::board_sync` prints it as `Last error:` and the envelope carries it in `summary`.
    let text = if json {
        to_json(&BoardSyncEnvelope {
            protocol: PROTOCOL,
            job_id: &job_id,
            job: Some(&job),
            summary: Some(&summary),
        })?
    } else {
        human::board_sync(&job, &summary)
    };
    Ok(CommandOutput::success(text))
}

async fn wait_for_sync(client: &Client, id: &JobId) -> Result<JobRecord, ProtoError> {
    loop {
        let job = client
            .list_jobs()
            .await?
            .into_iter()
            .find(|job| &job.id == id)
            .ok_or_else(|| ProtoError {
                kind: ErrorKind::NotFound,
                message: format!("board sync job `{id}` is no longer available"),
            })?;
        match &job.status {
            JobStatus::Succeeded | JobStatus::Failed { .. } | JobStatus::Cancelled => {
                return Ok(job);
            }
            JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling => {}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, Command};
    use clap::Parser;
    use fleet_core::{
        board::{RemoteLink, new_board},
        model::Context,
    };
    use fleet_proto::{
        PROTOCOL_VERSION,
        codec::FleetCodec,
        job::JobKind,
        request::{Request, RequestBody},
        response::{Response, ResponseBody},
        snapshot::Snapshot,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::{net::UnixListener, time::timeout};
    use tokio_util::codec::Framed;

    fn view() -> BoardView {
        let context = Context {
            id: "work".parse().unwrap(),
            name: "Fleet".into(),
            owners: vec![],
            created_at: "now".into(),
        };
        let mut board = new_board(&context, "now");
        board.prefix = "FLT".into();
        let card = serde_json::from_value(json!({"id":"Card-12","boardId":"work","number":12,"title":"Fix login","statusId":"todo","createdAt":"now","updatedAt":"now"})).unwrap();
        BoardView {
            board,
            cards: vec![card],
        }
    }

    fn args(arguments: &[&str]) -> BoardArgs {
        let Some(Command::Board(args)) = Cli::try_parse_from(
            ["fleet", "board"]
                .into_iter()
                .chain(arguments.iter().copied()),
        )
        .unwrap()
        .command
        else {
            panic!("expected board")
        };
        args
    }

    fn empty_snapshot() -> Snapshot {
        serde_json::from_value(json!({
            "generatedAt": "now", "contexts": [], "repos": [], "clones": [],
            "worktrees": [], "sessions": [], "statuses": [], "jobs": [],
            "daemon": {"version": "test", "pid": 1, "startedAt": "now", "home": "/tmp"}
        }))
        .unwrap()
    }

    fn get_board() -> RequestBody {
        RequestBody::GetBoard {
            board_id: "work".parse().unwrap(),
        }
    }

    /// Exercises real request framing without requiring or spawning fleetd.
    async fn run(
        arguments: &[&str],
        steps: Vec<(RequestBody, Result<ResponseBody, ProtoError>)>,
    ) -> Result<CommandOutput, ProtoError> {
        timeout(Duration::from_secs(5), async {
            let home = TempDir::new().unwrap();
            let listener = UnixListener::bind(home.path().join("fleetd.sock")).unwrap();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut transport =
                    Framed::new(socket, FleetCodec::<serde_json::Value, Request>::new());
                let hello = transport.next().await.unwrap().unwrap();
                assert!(matches!(hello.body, RequestBody::Hello { .. }));
                transport
                    .send(
                        serde_json::to_value(Response {
                            id: hello.id,
                            result: Ok(ResponseBody::Hello {
                                protocol: PROTOCOL_VERSION,
                                server: "test".into(),
                            }),
                        })
                        .unwrap(),
                    )
                    .await
                    .unwrap();
                let subscribe = transport.next().await.unwrap().unwrap();
                assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
                transport
                    .send(
                        serde_json::to_value(Response {
                            id: subscribe.id,
                            result: Ok(ResponseBody::Ack),
                        })
                        .unwrap(),
                    )
                    .await
                    .unwrap();
                for (expected, result) in steps {
                    let request = transport.next().await.unwrap().unwrap();
                    assert_eq!(request.body, expected);
                    transport
                        .send(
                            serde_json::to_value(Response {
                                id: request.id,
                                result,
                            })
                            .unwrap(),
                        )
                        .await
                        .unwrap();
                }
            });
            let client = Client::connect(home.path()).await.unwrap();
            let arguments = args(arguments);
            let json = arguments.json;
            let command = Command::Board(arguments);
            assert_eq!(super::super::command_requests_json(&command), json);
            let output = super::super::execute(&client, command).await;
            drop(client);
            server.await.unwrap();
            output
        })
        .await
        .expect("board CLI socket test timed out")
    }

    fn link(key: &str) -> RemoteLink {
        RemoteLink {
            parent_key: None,
            backend: "jira".into(),
            key: key.into(),
            url: None,
            version: None,
            synced_at: "now".into(),
            remote_updated_at: None,
        }
    }

    #[test]
    fn resolves_display_local_and_opaque_keys_case_insensitively_and_rejects_ambiguity() {
        let mut view = view();
        // An unlinked card answers to its local key, its id, and — display_key falling back —
        // to the same local key again.
        for key in ["fLt-12", "cARD-12"] {
            assert_eq!(resolve_card(&view, key).unwrap().id, view.cards[0].id);
        }
        view.cards[0].remote = Some(link("PROJ-123"));
        for key in ["pRoJ-123", "cARD-12"] {
            assert_eq!(resolve_card(&view, key).unwrap().id, view.cards[0].id);
        }
        assert_eq!(
            resolve_card(&view, "missing").unwrap_err().kind,
            ErrorKind::NotFound
        );
        let mut collision = view.cards[0].clone();
        collision.id = "other-card".parse().unwrap();
        collision.number = 13;
        view.cards.push(collision);
        assert_eq!(
            resolve_card(&view, "proj-123").unwrap_err().kind,
            ErrorKind::Conflict
        );
    }

    /// A board whose prefix is the project key it mirrors puts two namespaces of the same
    /// shape on the same cards. Only the remote key addresses a linked card, or `SP-4` names
    /// both issue SP-4 and the fourth card — and would move whichever one it picked.
    #[test]
    fn a_linked_card_answers_to_its_remote_key_and_never_to_a_local_one() {
        let mut view = view();
        view.board.prefix = "SP".into();
        view.cards[0].number = 3;
        view.cards[0].remote = Some(link("SP-2"));
        let mut second = view.cards[0].clone();
        second.id = "card-99".parse().unwrap();
        second.number = 4;
        second.remote = Some(link("SP-3"));
        view.cards.push(second);
        // "SP-3" is the second card's issue, never the first card's local key.
        assert_eq!(resolve_card(&view, "SP-3").unwrap().id, view.cards[1].id);
        assert_eq!(resolve_card(&view, "SP-2").unwrap().id, view.cards[0].id);
        // A local number no issue carries is not a card at all once the card is linked.
        assert_eq!(
            resolve_card(&view, "SP-4").unwrap_err().kind,
            ErrorKind::NotFound
        );
    }

    #[tokio::test]
    async fn rejects_selectors_split_across_subcommand_levels_before_any_request() {
        let error = run(
            &[
                "--board",
                "work",
                "card",
                "show",
                "FLT-12",
                "--context",
                "personal",
            ],
            vec![],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(error.message.contains("--board and --context"));
    }

    #[tokio::test]
    async fn default_and_explicit_context_ensure_board_while_explicit_board_fetches() {
        let view = view();
        let snapshot = Snapshot {
            active_context: Some("work".parse().unwrap()),
            ..empty_snapshot()
        };
        let ensure = RequestBody::EnsureBoard {
            context_id: "work".parse().unwrap(),
        };
        for (arguments, steps) in [
            (
                vec!["show", "--json"],
                vec![
                    (
                        RequestBody::GetSnapshot,
                        Ok(ResponseBody::Snapshot(snapshot)),
                    ),
                    (ensure.clone(), Ok(ResponseBody::Board(view.clone()))),
                ],
            ),
            (
                vec!["card", "show", "flt-12", "--context", "work", "--json"],
                vec![(ensure, Ok(ResponseBody::Board(view.clone())))],
            ),
            (
                vec!["show", "--board", "work", "--json"],
                vec![(get_board(), Ok(ResponseBody::Board(view.clone())))],
            ),
        ] {
            let output = run(&arguments, steps).await.unwrap();
            assert_eq!(output.exit_code, 0);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&output.text).unwrap()["protocol"],
                1
            );
        }
        let error = run(
            &["show", "--json"],
            vec![(
                RequestBody::GetSnapshot,
                Ok(ResponseBody::Snapshot(empty_snapshot())),
            )],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(error.message.contains("no active context"));
    }

    #[tokio::test]
    async fn board_set_creates_and_removes_the_labels_cards_can_then_carry() {
        let mut view = view();
        view.board.labels.push(Label {
            id: "old".parse().unwrap(),
            name: "Old".into(),
            color: None,
        });
        let patch = BoardPatch {
            labels: Some(vec![Label {
                id: "needs-triage".parse().unwrap(),
                name: "Needs triage".into(),
                color: None,
            }]),
            ..BoardPatch::default()
        };
        run(
            &[
                "set",
                "--board",
                "work",
                "--add-label",
                "Needs triage",
                "--remove-label",
                "Old",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateBoard {
                        board_id: "work".parse().unwrap(),
                        patch,
                    },
                    Ok(ResponseBody::Board(view.clone())),
                ),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        // Without a label surface the documented `--label` flag can never resolve anything.
        let error = run(
            &["set", "--board", "work", "--remove-label", "ghost"],
            vec![(get_board(), Ok(ResponseBody::Board(view)))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
    }

    #[test]
    fn adding_a_label_that_differs_only_in_case_reuses_the_one_the_board_has() {
        let mut board = view().board;
        board.labels.push(Label {
            id: "bug".parse().unwrap(),
            name: "Bug".into(),
            color: None,
        });
        // Every other name lookup here is case-insensitive. A second `bug` would be
        // indistinguishable from `Bug` on a card, and `--label Bug` would then refuse both
        // as ambiguous, so the board could never use either again.
        assert_eq!(
            board_labels(&board, &["bug".into()], &[]).unwrap(),
            Some(board.labels.clone())
        );
        assert_eq!(
            board_labels(&board, &["Triage".into()], &[])
                .unwrap()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn listing_an_unknown_board_is_not_an_empty_table() {
        let view = view();
        // An empty table reads as "that board has nothing", which is not what happened.
        let error = run(
            &["list", "--board", "ghost"],
            vec![(
                RequestBody::ListBoards { context_id: None },
                Ok(ResponseBody::Boards(vec![summarize(
                    &view.board,
                    &view.cards,
                )])),
            )],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn resolving_renders_the_card_against_the_board_the_resolution_left_behind() {
        let view = view();
        let mut card = view.cards[0].clone();
        card.labels = vec!["regression".parse().unwrap()];
        let mut refreshed = view.clone();
        refreshed.board.labels.push(Label {
            id: "regression".parse().unwrap(),
            name: "Regression".into(),
            color: None,
        });
        refreshed.cards = vec![card.clone()];
        // Take-remote materializes the remote's labels on the board itself; the snapshot this
        // command opened with would render the new label as its slug.
        let output = run(
            &[
                "card",
                "resolve",
                "FLT-12",
                "take-remote",
                "--board",
                "work",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::ResolveCardConflict {
                        card_id: view.cards[0].id.clone(),
                        resolution: ConflictResolution::TakeRemote,
                    },
                    Ok(ResponseBody::Card(card)),
                ),
                (get_board(), Ok(ResponseBody::Board(refreshed))),
            ],
        )
        .await
        .unwrap();
        assert!(
            output.text.contains("Labels: Regression"),
            "{}",
            output.text
        );
    }

    #[tokio::test]
    async fn clear_flags_are_refused_on_create_instead_of_silently_ignored() {
        let error = run(
            &["card", "new", "Title", "--board", "work", "--clear-labels"],
            vec![(get_board(), Ok(ResponseBody::Board(view())))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(error.message.contains("--clear-labels"));
    }

    #[tokio::test]
    async fn an_impossible_due_date_is_refused_before_the_daemon_is_asked() {
        for arguments in [
            vec![
                "card",
                "new",
                "Title",
                "--board",
                "work",
                "--due",
                "2026-02-31",
            ],
            vec![
                "card",
                "edit",
                "FLT-12",
                "--board",
                "work",
                "--due",
                "not-a-date",
            ],
        ] {
            let error = run(
                &arguments,
                vec![(get_board(), Ok(ResponseBody::Board(view())))],
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Validation);
            assert!(error.message.contains("YYYY-MM-DD"), "{}", error.message);
        }
    }

    #[tokio::test]
    async fn a_flagless_board_set_is_refused_like_an_empty_card_edit() {
        let error = run(
            &["set", "--board", "work"],
            vec![(get_board(), Ok(ResponseBody::Board(view())))],
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(
            error.message.contains("at least one field"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn set_preserves_other_settings_and_edit_does_not_reset_omitted_fields() {
        let mut view = view();
        view.board.settings.push_new_cards = true;
        view.board.settings.branch_template = "task-{key}".into();
        let mut settings = view.board.settings.clone();
        settings.start_on_worktree = false;
        let patch = BoardPatch {
            settings: Some(settings),
            ..BoardPatch::default()
        };
        run(
            &["set", "--board", "work", "--start-on-worktree", "false"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateBoard {
                        board_id: view.board.id.clone(),
                        patch,
                    },
                    Ok(ResponseBody::Board(view.clone())),
                ),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        // The one board setting with no other surface: without it `push_create` is a
        // capability no client can ever turn on, and `card new` on a remote board makes a
        // card no sync will ever push.
        let mut pushing = view.board.settings.clone();
        pushing.push_new_cards = false;
        run(
            &["set", "--board", "work", "--push-new-cards", "false"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateBoard {
                        board_id: view.board.id.clone(),
                        patch: BoardPatch {
                            settings: Some(pushing),
                            ..BoardPatch::default()
                        },
                    },
                    Ok(ResponseBody::Board(view.clone())),
                ),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        let patch = CardPatch {
            title: Some("New title".into()),
            ..CardPatch::default()
        };
        run(
            &[
                "card",
                "edit",
                "FLT-12",
                "--board",
                "work",
                "--title",
                "New title",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateCard {
                        card_id: view.cards[0].id.clone(),
                        patch,
                    },
                    Ok(ResponseBody::Card(view.cards[0].clone())),
                ),
            ],
        )
        .await
        .unwrap();
        let patch = card_patch(
            &view.board,
            BoardCardFields {
                priority: Some(BoardPriority::None),
                estimate: Some(0),
                ..BoardCardFields::default()
            },
        )
        .unwrap();
        assert_eq!(patch.priority, Some(Priority::None));
        assert_eq!(patch.estimate, Some(Some(0)));
        assert_eq!(patch.labels, None);
        assert_eq!(patch.assignee, None);
    }

    #[tokio::test]
    async fn list_create_board_and_new_card_use_typed_requests() {
        let mut view = view();
        let summary = summarize(&view.board, &view.cards);
        let output = run(
            &["list", "--json"],
            vec![(
                RequestBody::ListBoards { context_id: None },
                Ok(ResponseBody::Boards(vec![summary.clone()])),
            )],
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
            json!({"protocol":1,"boards":[summary]})
        );
        run(
            &[
                "create",
                "--context",
                "work",
                "--name",
                "Fleet",
                "--prefix",
                "FLT",
                "--backend",
                "local",
                "--json",
            ],
            vec![(
                RequestBody::CreateBoard {
                    context_id: "work".parse().unwrap(),
                    name: Some("Fleet".into()),
                    prefix: Some("FLT".into()),
                    backend: Some(BackendRef::default()),
                },
                Ok(ResponseBody::Board(view.clone())),
            )],
        )
        .await
        .unwrap();
        view.board.labels.push(fleet_core::board::Label {
            id: "bug".parse().unwrap(),
            name: "Bug report".into(),
            color: None,
        });
        run(
            &[
                "card",
                "new",
                "Fix login",
                "--board",
                "work",
                "--desc",
                "Details",
                "--status",
                "In Progress",
                "--priority",
                "urgent",
                "--label",
                "Bug report",
                "--label",
                "BUG",
                "--assignee",
                "Danny",
                "--estimate",
                "3",
                "--due",
                "2026-09-30",
                "--repo",
                "acme/api",
                "--json",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::CreateCard {
                        board_id: view.board.id.clone(),
                        draft: CardDraft {
                            title: "Fix login".into(),
                            description: "Details".into(),
                            status_id: Some("in-progress".parse().unwrap()),
                            priority: Priority::Urgent,
                            labels: vec!["bug".parse().unwrap()],
                            assignee: Some("Danny".into()),
                            estimate: Some(3),
                            due_date: Some("2026-09-30".into()),
                            repo_id: Some("acme/api".parse().unwrap()),
                            ..CardDraft::default()
                        },
                    },
                    Ok(ResponseBody::Card(view.cards[0].clone())),
                ),
            ],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn card_move_comment_delete_and_resolve_send_resolved_ids() {
        let view = view();
        let id = view.cards[0].id.clone();
        for (arguments, request, response) in [
            (
                vec!["move", "fLt-12", "Done", "--index", "0"],
                RequestBody::MoveCard {
                    card_id: id.clone(),
                    status_id: "done".parse().unwrap(),
                    index: Some(0),
                },
                ResponseBody::Card(view.cards[0].clone()),
            ),
            (
                vec!["comment", "cARD-12", "Hello"],
                RequestBody::AddCardComment {
                    card_id: id.clone(),
                    body: "Hello".into(),
                },
                ResponseBody::Card(view.cards[0].clone()),
            ),
            (
                vec!["resolve", "FLT-12", "take-remote"],
                RequestBody::ResolveCardConflict {
                    card_id: id.clone(),
                    resolution: ConflictResolution::TakeRemote,
                },
                ResponseBody::Card(view.cards[0].clone()),
            ),
            (
                vec!["delete", "FLT-12"],
                RequestBody::DeleteCard { card_id: id },
                ResponseBody::Ack,
            ),
        ] {
            let arguments: Vec<_> = ["card"]
                .into_iter()
                .chain(arguments)
                .chain(["--board", "work", "--json"])
                .collect();
            // Resolving re-reads the board before rendering: take-remote materializes the
            // remote's labels on the board itself.
            let refreshes = matches!(request, RequestBody::ResolveCardConflict { .. });
            let mut steps = vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (request, Ok(response)),
            ];
            if refreshes {
                steps.push((get_board(), Ok(ResponseBody::Board(view.clone()))));
            }
            let output = run(&arguments, steps).await.unwrap();
            assert_eq!(output.exit_code, 0);
        }
    }

    #[tokio::test]
    async fn worktree_human_output_matches_create() {
        let view = view();
        let worktree: fleet_core::model::Worktree = serde_json::from_value(json!({"id":"acme/api#flt-12","repoId":"acme/api","slug":"flt-12","branch":"flt-12","baseRef":"main","path":"/tmp/flt-12","session":"api/flt-12","createdAt":"now"})).unwrap();
        // `created` is the daemon's answer, not an inference from the card's previous link:
        // adopting an existing worktree must not be reported as a creation.
        for existing in [false, true] {
            let output = run(
                &[
                    "card", "worktree", "FLT-12", "--board", "work", "--repo", "acme/api",
                    "--base", "main", "--host", "local",
                ],
                vec![
                    (get_board(), Ok(ResponseBody::Board(view.clone()))),
                    (
                        RequestBody::CreateWorktreeFromCard {
                            card_id: view.cards[0].id.clone(),
                            repo_id: Some(worktree.repo_id.clone()),
                            base: Some("main".into()),
                            host: None,
                        },
                        Ok(ResponseBody::CardWorktree {
                            card: view.cards[0].clone(),
                            worktree: worktree.clone(),
                            created: !existing,
                        }),
                    ),
                ],
            )
            .await
            .unwrap();
            assert_eq!(
                output.text,
                if existing {
                    "Existing acme/api#flt-12"
                } else {
                    "Created acme/api#flt-12"
                }
            );
        }
    }

    fn job(status: JobStatus) -> JobRecord {
        JobRecord {
            id: "sync-1".parse().unwrap(),
            kind: JobKind::Custom("board.sync".into()),
            target: "work".into(),
            title: "Sync".into(),
            status,
            progress: Some("pulled 3, pushed 1".into()),
            log_path: "/tmp/sync.log".into(),
            started_at: "now".into(),
            finished_at: None,
            cancellable: true,
            retryable: false,
        }
    }

    #[tokio::test]
    async fn sync_wait_polls_until_terminal_and_reports_summary_or_error() {
        for status in [
            JobStatus::Succeeded,
            JobStatus::Failed {
                error: "job failed".into(),
            },
            JobStatus::Cancelled,
        ] {
            let mut view = view();
            if matches!(status, JobStatus::Failed { .. }) {
                view.board.sync.last_error = Some("backend unavailable".into());
            }
            let output = run(
                &["sync", "--board", "work", "--wait", "--json"],
                vec![
                    (get_board(), Ok(ResponseBody::Board(view.clone()))),
                    (
                        RequestBody::SyncBoard {
                            board_id: view.board.id.clone(),
                            full: false,
                        },
                        Ok(ResponseBody::Job(job(JobStatus::Queued))),
                    ),
                    (
                        RequestBody::ListJobs,
                        Ok(ResponseBody::Jobs(vec![job(JobStatus::Running)])),
                    ),
                    (
                        RequestBody::ListJobs,
                        Ok(ResponseBody::Jobs(vec![job(status.clone())])),
                    ),
                    (get_board(), Ok(ResponseBody::Board(view))),
                ],
            )
            .await;
            match status {
                JobStatus::Succeeded => {
                    let output = output.unwrap();
                    let json: serde_json::Value = serde_json::from_str(&output.text).unwrap();
                    assert_eq!(json["job"]["progress"], "pulled 3, pushed 1");
                    assert_eq!(json["summary"]["cardCount"], 1);
                    assert_eq!(json["protocol"], 1);
                }
                JobStatus::Failed { .. } => {
                    assert!(output.unwrap_err().message.contains("backend unavailable"))
                }
                JobStatus::Cancelled => assert_eq!(output.unwrap_err().kind, ErrorKind::Cancelled),
                _ => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn sync_without_wait_returns_immediately_and_missing_jobs_report_not_found() {
        let view = view();
        let steps = vec![
            (get_board(), Ok(ResponseBody::Board(view.clone()))),
            (
                RequestBody::SyncBoard {
                    board_id: view.board.id.clone(),
                    full: false,
                },
                Ok(ResponseBody::Job(job(JobStatus::Queued))),
            ),
        ];
        let output = run(&["sync", "--board", "work", "--json"], steps.clone())
            .await
            .unwrap();
        assert_eq!(output.text, r#"{"protocol":1,"jobId":"sync-1"}"#);
        let mut steps = steps;
        steps.push((RequestBody::ListJobs, Ok(ResponseBody::Jobs(vec![]))));
        let error = run(&["sync", "--board", "work", "--wait"], steps)
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
        assert!(error.message.contains("board sync job"));
    }

    fn jira_view() -> BoardView {
        let mut view = view();
        view.board.backend = BackendRef {
            kind: "jira".into(),
            settings: json!({"project": "OLD", "jql": "assignee = currentUser()"}),
        };
        view
    }

    fn descriptors() -> Vec<fleet_core::board::BackendDescriptor> {
        vec![fleet_core::board::BackendDescriptor {
            kind: "jira".into(),
            label: "Jira (acli)".into(),
            capabilities: fleet_core::board::BackendCapabilities {
                pull: true,
                ..fleet_core::board::BackendCapabilities::default()
            },
            settings_schema: vec![
                serde_json::from_value(
                    json!({"key": "project", "name": "Project key (required)", "kind": "text"}),
                )
                .unwrap(),
            ],
        }]
    }

    #[tokio::test]
    async fn backends_needs_no_board_and_describe_asks_the_board_it_resolved() {
        let output = run(
            &["backends", "--json"],
            vec![(
                RequestBody::ListBoardBackends {},
                Ok(ResponseBody::BoardBackends(descriptors())),
            )],
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
            json!({"protocol": 1, "backends": descriptors()})
        );
        // Which kinds the daemon registers is not a property of any one board, so no board is
        // resolved first: a context without a board can still ask what it could point at.
        let output = run(
            &["backends"],
            vec![(
                RequestBody::ListBoardBackends {},
                Ok(ResponseBody::BoardBackends(descriptors())),
            )],
        )
        .await
        .unwrap();
        assert!(
            output.text.contains("jira  Jira (acli)  pull"),
            "{}",
            output.text
        );

        let schema = fleet_core::board::BackendSchema {
            key_prefix: Some("SP".into()),
            readonly_fields: vec!["priority".into()],
            ..fleet_core::board::BackendSchema::default()
        };
        let output = run(
            &["describe", "--board", "work"],
            vec![
                (get_board(), Ok(ResponseBody::Board(jira_view()))),
                (
                    RequestBody::DescribeBoardBackend {
                        board_id: "work".parse().unwrap(),
                    },
                    Ok(ResponseBody::BoardBackendSchema(schema.clone())),
                ),
            ],
        )
        .await
        .unwrap();
        assert!(
            output.text.contains("Read-only fields: priority"),
            "{}",
            output.text
        );
        let output = run(
            &["describe", "--board", "work", "--json"],
            vec![
                (get_board(), Ok(ResponseBody::Board(jira_view()))),
                (
                    RequestBody::DescribeBoardBackend {
                        board_id: "work".parse().unwrap(),
                    },
                    Ok(ResponseBody::BoardBackendSchema(schema.clone())),
                ),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
            json!({"protocol": 1, "schema": schema})
        );
    }

    #[tokio::test]
    async fn a_kind_change_starts_from_empty_settings_and_a_setting_alone_merges() {
        for (arguments, expected) in [
            (
                vec![
                    "set",
                    "--backend",
                    "jira",
                    "--setting",
                    "project=SP",
                    "--setting",
                    "maxConcurrency=8",
                ],
                // The previous kind's keys mean nothing to the new one, so `jql` is gone and
                // `8` arrives as a number, not as the string the shell handed us.
                BackendRef {
                    kind: "jira".into(),
                    settings: json!({"project": "SP", "maxConcurrency": 8}),
                },
            ),
            (
                vec![
                    "set",
                    "--setting",
                    r#"statuses=["To Do","Done"]"#,
                    "--setting",
                    "jql=null",
                ],
                // No `--backend`: the kind stays and the pairs merge into what is stored,
                // where `null` is how a key is removed.
                BackendRef {
                    kind: "jira".into(),
                    settings: json!({"project": "OLD", "statuses": ["To Do", "Done"]}),
                },
            ),
            (
                vec!["set", "--backend", "local"],
                // `--backend` alone leaves the new kind with nothing configured.
                BackendRef::default(),
            ),
        ] {
            let view = jira_view();
            let arguments: Vec<_> = arguments.into_iter().chain(["--board", "work"]).collect();
            run(
                &arguments,
                vec![
                    (get_board(), Ok(ResponseBody::Board(view.clone()))),
                    (
                        RequestBody::UpdateBoard {
                            board_id: view.board.id.clone(),
                            patch: BoardPatch {
                                backend: Some(expected),
                                ..BoardPatch::default()
                            },
                        },
                        Ok(ResponseBody::Board(view.clone())),
                    ),
                    // `set` prints the same header `show` does, label included.
                    (
                        RequestBody::ListBoardBackends {},
                        Ok(ResponseBody::BoardBackends(descriptors())),
                    ),
                ],
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn create_carries_its_settings_and_a_backendless_create_stays_null() {
        for (arguments, backend) in [
            (
                vec!["--backend", "jira", "--setting", "project=SP"],
                Some(BackendRef {
                    kind: "jira".into(),
                    settings: json!({"project": "SP"}),
                }),
            ),
            (vec!["--backend", "local"], Some(BackendRef::default())),
            (vec![], None),
        ] {
            let arguments: Vec<_> = ["create", "--context", "work"]
                .into_iter()
                .chain(arguments)
                .collect();
            run(
                &arguments,
                vec![
                    (
                        RequestBody::CreateBoard {
                            context_id: "work".parse().unwrap(),
                            name: None,
                            prefix: None,
                            backend,
                        },
                        Ok(ResponseBody::Board(view())),
                    ),
                    (
                        RequestBody::ListBoardBackends {},
                        Ok(ResponseBody::BoardBackends(descriptors())),
                    ),
                ],
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn a_full_sync_tells_the_daemon_to_ignore_the_cursor() {
        let view = jira_view();
        let output = run(
            &["sync", "--board", "work", "--full", "--json"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::SyncBoard {
                        board_id: view.board.id.clone(),
                        full: true,
                    },
                    Ok(ResponseBody::Job(job(JobStatus::Queued))),
                ),
            ],
        )
        .await
        .unwrap();
        assert_eq!(output.text, r#"{"protocol":1,"jobId":"sync-1"}"#);
    }

    #[tokio::test]
    async fn a_read_only_field_fails_with_the_daemons_message_word_for_word() {
        let view = jira_view();
        let message = "priority is read-only on this board's backend";
        let error = run(
            &[
                "card",
                "edit",
                "FLT-12",
                "--board",
                "work",
                "--priority",
                "high",
            ],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::UpdateCard {
                        card_id: view.cards[0].id.clone(),
                        patch: CardPatch {
                            priority: Some(Priority::High),
                            ..CardPatch::default()
                        },
                    },
                    Err(ProtoError {
                        kind: ErrorKind::Validation,
                        message: message.to_owned(),
                    }),
                ),
            ],
        )
        .await
        .unwrap_err();
        // The CLI is the surface that shows this; a reworded copy would send the user looking
        // for a setting that does not exist.
        assert_eq!(error.message, message);
        assert_eq!(error.kind, ErrorKind::Validation);
    }

    #[tokio::test]
    async fn the_show_header_names_the_backend_and_json_never_pays_for_it() {
        let view = jira_view();
        let output = run(
            &["show", "--board", "work"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::ListBoardBackends {},
                    Ok(ResponseBody::BoardBackends(descriptors())),
                ),
            ],
        )
        .await
        .unwrap();
        assert!(
            output
                .text
                .contains("backend: Jira (acli) \u{b7} project OLD \u{b7} never synced"),
            "{}",
            output.text
        );
        // A daemon that cannot list backends still prints the board.
        let output = run(
            &["show", "--board", "work"],
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    RequestBody::ListBoardBackends {},
                    Err(ProtoError {
                        kind: ErrorKind::Unknown,
                        message: "unsupported".into(),
                    }),
                ),
            ],
        )
        .await
        .unwrap();
        // Which settings identify a board is the backend's own answer, so without a descriptor
        // the header names the kind and stops there rather than guessing at keys.
        assert!(
            output.text.contains("backend: jira \u{b7} never synced"),
            "{}",
            output.text
        );
        // `--json` returns the board verbatim, so the descriptor round trip is not made.
        let output = run(
            &["show", "--board", "work", "--json"],
            vec![(get_board(), Ok(ResponseBody::Board(view)))],
        )
        .await
        .unwrap();
        assert!(!output.text.contains("Jira (acli)"), "{}", output.text);
    }
}
