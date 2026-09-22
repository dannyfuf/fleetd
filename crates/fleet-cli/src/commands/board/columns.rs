//! `fleet board columns`: the column vector and the automation each column carries.
//!
//! Every verb is a read-modify-write of the whole vector and sends one `UpdateBoard` with
//! `statuses` set in full, so a concurrent editor loses exactly as `board set` already lets it.
//! `remove --move-cards-to` sends one `MoveCard` per card first, because the daemon refuses to
//! remove a status a card still uses and has no move-cards path of its own.
//!
//! The verbs never repeat a refusal the daemon owns — automation on a context or Jira board, a
//! route that points backwards, an `--env` name the delegation reserves, a column with live runs
//! — so both surfaces say the same sentence about the same document. What is refused here is
//! only what never reaches the wire: a column name that resolves to nothing, an `--on-enter`
//! word that is not one of the three, or an action flag on a column that runs nothing.

use crate::{
    args::{
        AgentChoice, AgentModeChoice, BoardColumnsArgs, BoardColumnsCommand, BoardColumnsPreset,
        BoardStatusCategory,
    },
    commands::{CommandOutput, validation},
    human,
};
use fleet_client::Client;
use fleet_core::{
    agents::{AgentKind, PermissionMode},
    board::{
        Action, ActionKind, Board, BoardPatch, BoardView, ColumnAgentPrefs, Status, StatusCategory,
        apply_workflow_preset, normalise_automation, workflow_preset,
    },
    ids::{CardId, StatusId},
    slug::slugify,
};
use fleet_proto::error::ProtoError;
use std::path::Path;

/// Lists or edits the board's columns.
///
/// `view` is the board the dispatcher already resolved: no verb reads the board a second time.
pub(super) async fn run(
    client: &Client,
    view: &BoardView,
    arguments: BoardColumnsArgs,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let Some(command) = arguments.command else {
        return output(view, &[], json);
    };
    let planned = plan(view, command)?;
    if !planned.changed {
        return output(view, &planned.notes, json);
    }
    // The cards leave first: the daemon refuses to drop a status a card still names, and a
    // half-done sequence leaves the moved cards where they landed, which `remove` can retry.
    for (card, status) in planned.moves {
        client.move_card(card, status, None, false).await?;
    }
    let view = client
        .update_board(
            view.board.id.clone(),
            BoardPatch {
                statuses: Some(planned.statuses),
                ..BoardPatch::default()
            },
        )
        .await?;
    output(&view, &planned.notes, json)
}

/// What one verb decided, before any of it reaches the daemon.
#[derive(Debug)]
struct Planned {
    /// The whole column vector the patch carries.
    statuses: Vec<Status>,
    /// Cards to move out of a removed column first, in board order.
    moves: Vec<(CardId, StatusId)>,
    /// Lines printed above the table; human output only.
    notes: Vec<String>,
    /// Whether anything is worth sending at all.
    changed: bool,
}

impl Planned {
    /// A verb that always writes, and has nothing to tell the reader.
    fn write(statuses: Vec<Status>) -> Self {
        Self {
            statuses,
            moves: Vec::new(),
            notes: Vec::new(),
            changed: true,
        }
    }
}

/// Turns one verb into the column vector it asks for.
///
/// Pure but for `--instructions-file`, so the vector arithmetic is tested without a daemon.
fn plan(view: &BoardView, command: BoardColumnsCommand) -> Result<Planned, ProtoError> {
    let board = &view.board;
    match command {
        BoardColumnsCommand::Add {
            name,
            id,
            category,
            after,
            before,
        } => {
            let mut statuses = board.statuses.clone();
            let slug = id.unwrap_or_else(|| slugify(&name));
            if slug.is_empty() {
                return Err(validation(format!(
                    "column `{name}` must contain a letter or number"
                )));
            }
            let id = StatusId::try_from(slug).map_err(|error| validation(error.to_string()))?;
            if statuses.iter().any(|status| status.id == id) {
                return Err(validation(format!("column `{id}` already exists")));
            }
            let at = match (after.as_deref(), before.as_deref()) {
                (Some(value), _) => index_of(board, value)? + 1,
                (None, Some(value)) => index_of(board, value)?,
                (None, None) => statuses.len(),
            };
            statuses.insert(
                at,
                Status {
                    id,
                    name,
                    // A column nobody categorised is planned work: the one category that neither
                    // starts a card's clock nor closes it.
                    category: category.map_or(StatusCategory::Unstarted, category_of),
                    color: None,
                    automation: None,
                },
            );
            Ok(Planned::write(statuses))
        }
        BoardColumnsCommand::Edit {
            id,
            name,
            category,
            color,
            on_enter,
            provider,
            model,
            effort,
            mode,
            instructions,
            instructions_file,
            expect,
            on_success,
            no_on_success,
            when_unblocked,
            no_when_unblocked,
            env,
            clear_env,
        } => {
            let at = index_of(board, &id)?;
            // Routes resolve against the board as it stands: a column added in another command
            // is already there, and one added in this command is not a route yet.
            let on_success = on_success
                .map(|value| super::resolve_status(board, &value))
                .transpose()?;
            let when_unblocked = when_unblocked
                .map(|value| super::resolve_status(board, &value))
                .transpose()?;
            let instructions = match (instructions, instructions_file) {
                (Some(text), _) => Some(text),
                (None, Some(path)) => Some(read_file(&path, "instructions")?),
                (None, None) => None,
            };
            let mut statuses = board.statuses.clone();
            let status = &mut statuses[at];
            if let Some(name) = name {
                status.name = name;
            }
            if let Some(category) = category {
                status.category = category_of(category);
            }
            if let Some(color) = color {
                status.color = Some(color);
            }
            let mut automation = status.automation.clone().unwrap_or_default();
            if let Some(kind) = on_enter.as_deref().map(parse_on_enter).transpose()? {
                match (kind, automation.on_enter.as_mut()) {
                    // `--on-enter` names what the column runs; the instructions, expectation,
                    // agent and environment beside it are the action's and survive the swap.
                    (Some(kind), Some(action)) => action.kind = kind,
                    (Some(kind), None) => {
                        automation.on_enter = Some(Action {
                            kind,
                            instructions: String::new(),
                            expect: String::new(),
                            agent: ColumnAgentPrefs::default(),
                            env: Vec::new(),
                        });
                    }
                    (None, _) => automation.on_enter = None,
                }
            }
            let wants_action = instructions.is_some()
                || expect.is_some()
                || provider.is_some()
                || model.is_some()
                || effort.is_some()
                || mode.is_some()
                || clear_env
                || !env.is_empty();
            if wants_action {
                let Some(action) = automation.on_enter.as_mut() else {
                    return Err(validation(format!(
                        "column `{}` runs nothing; give it an action with --on-enter prompt or --on-enter skill:<name>",
                        status.name
                    )));
                };
                if let Some(text) = instructions {
                    action.instructions = text;
                }
                if let Some(text) = expect {
                    action.expect = text;
                }
                if let Some(provider) = provider {
                    action.agent.provider = Some(provider_kind(provider));
                }
                if let Some(model) = model {
                    action.agent.model = Some(model);
                }
                if let Some(effort) = effort {
                    action.agent.effort = Some(effort);
                }
                if let Some(mode) = mode {
                    action.agent.mode = Some(permission_mode(mode));
                }
                // `--clear-env` empties the column first, so the pair reads as "these are the
                // variables now" — the only way to replace a whole environment in one command.
                if clear_env {
                    action.env.clear();
                }
                for entry in env {
                    // A key given again replaces the value it had. Extending would leave the
                    // column carrying the key twice, which `validate_env` refuses as "given
                    // twice" — so without this there is no single command that changes one
                    // variable's value.
                    if let Some((key, _)) = entry.split_once('=') {
                        action.env.retain(|held| {
                            held.split_once('=').is_none_or(|(held, _)| held != key)
                        });
                    }
                    action.env.push(entry);
                }
            }
            if no_on_success {
                automation.on_success = None;
            }
            if let Some(target) = on_success {
                automation.on_success = Some(target);
            }
            if no_when_unblocked {
                automation.advance_when_unblocked = None;
            }
            if let Some(target) = when_unblocked {
                automation.advance_when_unblocked = Some(target);
            }
            status.automation = Some(automation);
            // A column emptied of every rule carries no block at all, so it stops reading as
            // automated here as well as in the document the daemon writes.
            normalise_automation(&mut statuses);
            Ok(Planned::write(statuses))
        }
        BoardColumnsCommand::Move { id, after, before } => {
            let at = index_of(board, &id)?;
            let Some(target) = after.as_deref().or(before.as_deref()) else {
                return Err(validation("column move needs --after or --before"));
            };
            let target_at = index_of(board, target)?;
            if target_at == at {
                return Err(validation("a column cannot move relative to itself"));
            }
            let mut statuses = board.statuses.clone();
            let column = statuses.remove(at);
            // Taking the column out shifts everything after it down by one, so the target's
            // index is arithmetic rather than a second search that could miss.
            let target_at = if target_at > at {
                target_at - 1
            } else {
                target_at
            };
            let at = if after.is_some() {
                target_at + 1
            } else {
                target_at
            };
            statuses.insert(at, column);
            Ok(Planned::write(statuses))
        }
        BoardColumnsCommand::Remove { id, move_cards_to } => {
            let at = index_of(board, &id)?;
            let mut statuses = board.statuses.clone();
            let removed = statuses.remove(at);
            let mut moves = Vec::new();
            let mut notes = Vec::new();
            if let Some(target) = move_cards_to {
                let target = super::resolve_status(board, &target)?;
                if target == removed.id {
                    return Err(validation("--move-cards-to must name a different column"));
                }
                // Archived cards count: the daemon's refusal reads every card on the board, and
                // a column an archived card still names cannot be removed either.
                moves = view
                    .cards
                    .iter()
                    .filter(|card| card.status_id == removed.id)
                    .map(|card| (card.id.clone(), target.clone()))
                    .collect();
                if !moves.is_empty() {
                    let name = statuses
                        .iter()
                        .find(|status| status.id == target)
                        .map_or_else(|| target.to_string(), |status| status.name.clone());
                    notes.push(format!(
                        "moved {} card{} to {name}",
                        moves.len(),
                        if moves.len() == 1 { "" } else { "s" }
                    ));
                }
            }
            Ok(Planned {
                statuses,
                moves,
                notes,
                changed: true,
            })
        }
        BoardColumnsCommand::Preset { which } => {
            let BoardColumnsPreset::Workflow = which;
            let mut next = board.clone();
            let added = apply_workflow_preset(&mut next);
            let mut notes = Vec::new();
            if !added {
                notes.push("every workflow column is already on this board".to_owned());
            }
            notes.extend(preset_notes(board, &next));
            Ok(Planned {
                statuses: next.statuses,
                moves: Vec::new(),
                notes,
                changed: added,
            })
        }
    }
}

/// One line per preset column the board already had and the preset therefore left running
/// nothing.
///
/// A board built from the shipped five reaches here with `In Progress` already in place, so the
/// preset adds Ready and In review around it and never gives it the action the preset describes.
/// Saying so is the difference between a preset that looks applied and one that runs.
fn preset_notes(before: &Board, after: &Board) -> Vec<String> {
    let mut notes = Vec::new();
    for preset in workflow_preset() {
        let Some(wanted) = preset
            .automation
            .as_ref()
            .and_then(|automation| automation.on_enter.as_ref())
        else {
            continue;
        };
        if !before.statuses.iter().any(|status| status.id == preset.id) {
            continue;
        }
        let runs_nothing = after.statuses.iter().any(|status| {
            status.id == preset.id
                && status
                    .automation
                    .as_ref()
                    .is_none_or(|automation| automation.on_enter.is_none())
        });
        if !runs_nothing {
            continue;
        }
        let name = after
            .statuses
            .iter()
            .find(|status| status.id == preset.id)
            .map_or_else(|| preset.name.clone(), |status| status.name.clone());
        notes.push(format!(
            "{name} was already here, so the preset left it alone and it still runs nothing; give it one with: fleet board columns edit {} --on-enter {}",
            preset.id,
            on_enter_value(&wanted.kind)
        ));
    }
    notes
}

/// The columns table, or the board envelope `board show --json` prints.
///
/// There is no columns envelope: the columns **are** `board.statuses`, and a second shape for
/// them would be a second thing to keep true. The backend descriptor is a courtesy of the human
/// header, which this branch never prints.
fn output(view: &BoardView, notes: &[String], json: bool) -> Result<CommandOutput, ProtoError> {
    if json {
        return super::board_show_output(view, None, true);
    }
    let mut lines = notes.to_vec();
    lines.extend(table(&view.board.statuses));
    Ok(CommandOutput::success(lines.join("\n")))
}

/// `id  name  category  on enter  on success  when unblocked`, padded into columns.
///
/// Every automation cell prints what the flag that writes it accepts — `prompt`,
/// `skill:deep-review`, a column id — so a row can be typed back into `columns edit`.
fn table(statuses: &[Status]) -> Vec<String> {
    let mut rows = vec![
        [
            "ID",
            "NAME",
            "CATEGORY",
            "ON ENTER",
            "ON SUCCESS",
            "WHEN UNBLOCKED",
        ]
        .map(str::to_owned)
        .to_vec(),
    ];
    rows.extend(statuses.iter().map(|status| {
        let automation = status.automation.as_ref();
        vec![
            status.id.to_string(),
            status.name.clone(),
            category_word(status.category).to_owned(),
            automation
                .and_then(|automation| automation.on_enter.as_ref())
                .map_or_else(dash, |action| on_enter_value(&action.kind)),
            automation
                .and_then(|automation| automation.on_success.as_ref())
                .map_or_else(dash, ToString::to_string),
            automation
                .and_then(|automation| automation.advance_when_unblocked.as_ref())
                .map_or_else(dash, ToString::to_string),
        ]
    }));
    human::columns(&rows)
}

/// What `--on-enter` was given, or `None` for `none`: the word that clears the action.
///
/// `skill:` with no name is sent as it was typed: the daemon owns `a skill action needs a name`,
/// and repeating it here would let the two sentences drift.
fn parse_on_enter(value: &str) -> Result<Option<ActionKind>, ProtoError> {
    match value {
        "none" => Ok(None),
        "prompt" => Ok(Some(ActionKind::Prompt)),
        value => {
            let Some(rest) = value.strip_prefix("skill:") else {
                return Err(validation(
                    "--on-enter must be none, prompt, or skill:<name>[:<args>]",
                ));
            };
            let (name, args) = rest.split_once(':').unwrap_or((rest, ""));
            Ok(Some(ActionKind::Skill {
                name: name.to_owned(),
                args: args.to_owned(),
            }))
        }
    }
}

/// How an action spells itself for `--on-enter`.
fn on_enter_value(kind: &ActionKind) -> String {
    match kind {
        ActionKind::Prompt => "prompt".to_owned(),
        ActionKind::Skill { name, args } if args.is_empty() => format!("skill:{name}"),
        ActionKind::Skill { name, args } => format!("skill:{name}:{args}"),
    }
}

/// The em dash every `human` table prints for a cell with nothing in it.
fn dash() -> String {
    "\u{2014}".to_owned()
}

/// `subagents::read_text`'s file branch, whose sentence this repeats because that helper is
/// private to its own command group.
fn read_file(path: &Path, name: &str) -> Result<String, ProtoError> {
    std::fs::read_to_string(path).map_err(|error| {
        validation(format!(
            "could not read {name} file {}: {error}",
            path.display()
        ))
    })
}

/// The index of the column `value` names, matched as [`super::resolve_status`] matches.
fn index_of(board: &Board, value: &str) -> Result<usize, ProtoError> {
    if let Some(index) = board
        .statuses
        .iter()
        .position(|status| status.id.as_str() == value)
    {
        return Ok(index);
    }
    super::unique_match(
        board
            .statuses
            .iter()
            .enumerate()
            .filter(|(_, status)| {
                status.id.as_str().eq_ignore_ascii_case(value)
                    || status.name.eq_ignore_ascii_case(value)
            })
            .map(|(index, _)| index),
        "status",
        value,
    )
}

const fn category_of(choice: BoardStatusCategory) -> StatusCategory {
    match choice {
        BoardStatusCategory::Backlog => StatusCategory::Backlog,
        BoardStatusCategory::Unstarted => StatusCategory::Unstarted,
        BoardStatusCategory::Started => StatusCategory::Started,
        BoardStatusCategory::Completed => StatusCategory::Completed,
        BoardStatusCategory::Canceled => StatusCategory::Canceled,
    }
}

/// The wire name of a category, which is also the word `--category` takes.
const fn category_word(category: StatusCategory) -> &'static str {
    match category {
        StatusCategory::Backlog => "backlog",
        StatusCategory::Unstarted => "unstarted",
        StatusCategory::Started => "started",
        StatusCategory::Completed => "completed",
        StatusCategory::Canceled => "canceled",
    }
}

const fn provider_kind(choice: AgentChoice) -> AgentKind {
    match choice {
        AgentChoice::Claude => AgentKind::Claude,
        AgentChoice::Codex => AgentKind::Codex,
    }
}

const fn permission_mode(choice: AgentModeChoice) -> PermissionMode {
    match choice {
        AgentModeChoice::Ask => PermissionMode::Ask,
        AgentModeChoice::AcceptEdits => PermissionMode::AcceptEdits,
        AgentModeChoice::Plan => PermissionMode::Plan,
        AgentModeChoice::Auto => PermissionMode::Auto,
        AgentModeChoice::DontAsk => PermissionMode::DontAsk,
        AgentModeChoice::FullAccess => PermissionMode::FullAccess,
    }
}

#[cfg(test)]
mod tests;
