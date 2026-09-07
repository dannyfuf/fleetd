use super::{CommandOutput, unknown, validation, worktrees::parse_host};
use crate::{
    args::{
        BoardArgs, BoardCardCommand, BoardCardFields, BoardCommand, BoardConflictPolicy,
        BoardCreateArgs, BoardPriority, BoardResolution, BoardSetArgs,
    },
    envelope::{
        BoardBackendSchemaEnvelope, BoardBackendsEnvelope, BoardCardEnvelope, BoardEnvelope,
        BoardListEnvelope, BoardSyncEnvelope, BoardWorktreeEnvelope, OkEnvelope, PROTOCOL, to_json,
    },
    human,
};
use fleet_client::Client;
use fleet_core::{
    board::{
        BackendDescriptor, BackendRef, Board, BoardPatch, BoardView, Card, CardDraft, CardPatch,
        ConflictPolicy, ConflictResolution, Label, Priority, merge_settings, summarize, valid_date,
    },
    ids::{BoardId, ContextId, JobId, LabelId, RepoId, StatusId},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    job::{JobRecord, JobStatus},
};
use std::time::Duration;

pub(super) async fn board(
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
        BoardCommand::List => list(client, board, context, json).await,
        BoardCommand::Backends => backends(client, json).await,
        BoardCommand::Create(arguments) => create(client, board, context, arguments, json).await,
        // Every remaining command names one board, and each resolves it the same way.
        command => {
            let view = resolve_board(client, board, context).await?;
            match command {
                BoardCommand::Show => show(client, &view, json).await,
                BoardCommand::Describe => describe(client, &view, json).await,
                BoardCommand::Set(arguments) => set(client, view, arguments, json).await,
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

/// Prints every board the daemon knows, or the one `--board` names.
async fn list(
    client: &Client,
    board: Option<BoardId>,
    context: Option<ContextId>,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
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

/// Prints the backend kinds this daemon registers.
///
/// Which kinds exist is a property of the daemon, not of any one board, so this command
/// resolves no board at all.
async fn backends(client: &Client, json: bool) -> Result<CommandOutput, ProtoError> {
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

/// Creates a board in the named or active context and prints it.
async fn create(
    client: &Client,
    board: Option<BoardId>,
    context: Option<ContextId>,
    arguments: BoardCreateArgs,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
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
    board_show_output(
        &view,
        descriptor_for(client, &view, json).await.as_ref(),
        json,
    )
}

/// Prints the board's columns and cards.
async fn show(client: &Client, view: &BoardView, json: bool) -> Result<CommandOutput, ProtoError> {
    board_show_output(
        view,
        descriptor_for(client, view, json).await.as_ref(),
        json,
    )
}

/// Prints what the board's backend reports about itself.
async fn describe(
    client: &Client,
    view: &BoardView,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
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

/// Applies the `--name`, `--prefix`, `--backend`, `--setting` and label flags to the board.
async fn set(
    client: &Client,
    view: BoardView,
    arguments: BoardSetArgs,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let patch = board_patch(&view.board, arguments)?;
    // `card edit` refuses an empty patch for the same reason: a request that
    // changes nothing still asks the daemon to rewrite the document.
    if patch.is_empty() {
        return Err(validation("board set requires at least one field"));
    }
    let view = client.update_board(view.board.id, patch).await?;
    // The same board view `show` prints, so it carries the same header: a raw `jira` here and
    // a `Jira (acli)` there read as two different things.
    board_show_output(
        &view,
        descriptor_for(client, &view, json).await.as_ref(),
        json,
    )
}

/// The descriptor naming the board's backend, or `None` when nobody will print it.
///
/// The label lives in the descriptor list, which only the human header reads: the JSON
/// envelope carries the board verbatim and must not pay for a second round trip.
async fn descriptor_for(
    client: &Client,
    view: &BoardView,
    json: bool,
) -> Option<BackendDescriptor> {
    if json {
        return None;
    }
    backend_descriptor(client, &view.board.backend.kind).await
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
    match command {
        BoardCardCommand::New { title, fields } => {
            card_new(client, view, title, fields, json).await
        }
        BoardCardCommand::Show { key } => {
            card_output(&view.board, &view.cards, resolve_card(view, &key)?, json)
        }
        BoardCardCommand::Edit {
            key,
            title,
            fields,
            archive,
        } => card_edit(client, view, &key, title, fields, archive, json).await,
        BoardCardCommand::Move { key, status, index } => {
            card_move(client, view, &key, &status, index, json).await
        }
        BoardCardCommand::Comment { key, body } => {
            card_comment(client, view, &key, body, json).await
        }
        BoardCardCommand::Delete { key } => card_delete(client, view, &key, json).await,
        BoardCardCommand::Worktree {
            key,
            repo,
            base,
            host,
        } => card_worktree(client, view, &key, repo, base, host, json).await,
        BoardCardCommand::Resolve { key, resolution } => {
            card_resolve(client, view, &key, resolution, json).await
        }
    }
}

/// Creates a card from the `card new` value flags.
async fn card_new(
    client: &Client,
    view: &BoardView,
    title: String,
    fields: BoardCardFields,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    // The contract gives `new` the value flags only: a clear flag would be a silent no-op on
    // a card that has nothing to clear yet.
    if let Some(flag) = fields.clear_flag() {
        return Err(validation(format!(
            "{flag} applies to `card edit`, not `card new`"
        )));
    }
    let patch = card_patch(&view.board, fields)?;
    let card = client
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
        .await?;
    card_output(&view.board, &view.cards, &card, json)
}

/// Applies the `card edit` field flags and `--archive` to one card.
async fn card_edit(
    client: &Client,
    view: &BoardView,
    key: &str,
    title: Option<String>,
    fields: BoardCardFields,
    archive: Option<bool>,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let card = resolve_card(view, key)?;
    let mut patch = card_patch(&view.board, fields)?;
    patch.title = title;
    patch.archived = archive;
    // `board set` refuses an empty patch for the same reason: a request that changes nothing
    // still asks the daemon to rewrite the document.
    if patch.is_empty() {
        return Err(validation(
            "card edit requires at least one field or --archive",
        ));
    }
    let card = client.update_card(card.id.clone(), patch).await?;
    card_output(&view.board, &view.cards, &card, json)
}

/// Moves a card to another column, optionally at a position inside it.
async fn card_move(
    client: &Client,
    view: &BoardView,
    key: &str,
    status: &str,
    index: Option<usize>,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let card = resolve_card(view, key)?;
    let card = client
        .move_card(card.id.clone(), resolve_status(&view.board, status)?, index)
        .await?;
    card_output(&view.board, &view.cards, &card, json)
}

/// Appends a comment to a card.
async fn card_comment(
    client: &Client,
    view: &BoardView,
    key: &str,
    body: String,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let card = client
        .add_card_comment(resolve_card(view, key)?.id.clone(), body)
        .await?;
    card_output(&view.board, &view.cards, &card, json)
}

/// Deletes a card and confirms it by the key the user typed.
async fn card_delete(
    client: &Client,
    view: &BoardView,
    key: &str,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let card = resolve_card(view, key)?;
    client.delete_card(card.id.clone()).await?;
    let text = if json {
        to_json(&OkEnvelope {
            protocol: PROTOCOL,
            ok: true,
        })?
    } else {
        format!("Deleted {}", card.display_key(&view.board))
    };
    Ok(CommandOutput::success(text))
}

/// Creates or adopts the worktree a card names.
async fn card_worktree(
    client: &Client,
    view: &BoardView,
    key: &str,
    repo: Option<RepoId>,
    base: Option<String>,
    host: Option<String>,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let card_id = resolve_card(view, key)?.id.clone();
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
    Ok(CommandOutput::success(text))
}

/// Resolves a card's conflict by keeping the local or the remote side.
async fn card_resolve(
    client: &Client,
    view: &BoardView,
    key: &str,
    resolution: BoardResolution,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let resolution = match resolution {
        BoardResolution::KeepLocal => ConflictResolution::KeepLocal,
        BoardResolution::TakeRemote => ConflictResolution::TakeRemote,
    };
    let card = client
        .resolve_card_conflict(resolve_card(view, key)?.id.clone(), resolution)
        .await?;
    // Taking the remote materializes its labels on the board itself. Rendering the card
    // against the snapshot this command opened with would print the new label's slug instead
    // of its name; a refresh that fails leaves that snapshot in place.
    let board = match client.get_board(view.board.id.clone()).await {
        Ok(refreshed) => refreshed.board,
        Err(_) => view.board.clone(),
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
mod tests;
