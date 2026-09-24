//! `fleet schedule`: list, create, edit, delete, run and inspect a board's scheduled agent tasks.
//!
//! The board selectors are `fleet board`'s. `list` and `new` always act on the selected board;
//! the verbs that name a schedule id look it up across every board, and check it belongs to the
//! selected board only when a selector was given, so `fleet schedule show <id>` works from any
//! directory without ensuring a board it does not need.

mod output;
#[cfg(test)]
mod tests;

use super::{
    CommandOutput, FAILURE,
    board::{BoardSelector, resolve_board},
    subagents::read_text,
    validation,
};
use crate::{
    args::{
        AgentChoice, AgentModeChoice, BoardWorktreeSelector, ScheduleArgs, ScheduleCommand,
        ScheduleEditArgs, ScheduleNewArgs, ScheduleStarter,
    },
    envelope::{OkEnvelope, PROTOCOL, to_json},
};
use fleet_client::Client;
use fleet_core::{
    agents::{AgentKind, PermissionMode},
    board::BoardView,
    ids::{BoardId, ContextId, ScheduleId},
    schedule::{
        Cadence, STARTER_PROMPT_GITHUB_REVIEWS, Schedule, ScheduleAgent, ScheduleDraft,
        ScheduleOutcome, SchedulePatch, ScheduleRun,
    },
};
use fleet_proto::error::{ErrorKind, ProtoError};
use output::{ScheduleEnvelope, SchedulesEnvelope};
use std::{path::Path, time::Duration};

/// How often `run --wait` re-reads the schedule while its run is going.
const WAIT_POLL: Duration = Duration::from_secs(1);

/// The refusal for an `edit` that names no field, the sentence `docs/BOARD.md` §12.6 prints.
const NOTHING_TO_CHANGE: &str = "nothing to change";

/// The four board selectors, as the command line gave them.
struct Selector {
    board: Option<BoardId>,
    worktree: Option<BoardWorktreeSelector>,
    context: Option<ContextId>,
    reviews: bool,
}

impl Selector {
    fn is_given(&self) -> bool {
        self.board.is_some() || self.worktree.is_some() || self.context.is_some() || self.reviews
    }

    /// Clap checks conflicts within one command level, but global flags can be supplied at
    /// different levels, exactly as `fleet board` notes; the pairs are refused again here.
    fn refuse_conflicts(&self) -> Result<(), ProtoError> {
        let given = [
            ("--board", self.board.is_some()),
            ("--worktree", self.worktree.is_some()),
            ("--context", self.context.is_some()),
            ("--reviews", self.reviews),
        ];
        let conflicting = |left: &str, right: &str| {
            !matches!(
                (left, right),
                ("--context", "--reviews") | ("--reviews", "--context")
            )
        };
        for (index, (left, left_given)) in given.iter().enumerate() {
            for (right, right_given) in given.iter().skip(index + 1) {
                if *left_given && *right_given && conflicting(left, right) {
                    return Err(validation(format!(
                        "{left} and {right} cannot be used together"
                    )));
                }
            }
        }
        Ok(())
    }

    async fn resolve(self, client: &Client) -> Result<BoardView, ProtoError> {
        resolve_board(
            client,
            BoardSelector {
                board: self.board,
                worktree: self.worktree,
                context: self.context,
                reviews: self.reviews,
            },
        )
        .await
    }
}

/// Runs one `fleet schedule` verb against the board the selectors name.
pub(super) async fn schedule(
    client: &Client,
    arguments: ScheduleArgs,
) -> Result<CommandOutput, ProtoError> {
    let ScheduleArgs {
        board,
        worktree,
        context,
        reviews,
        json,
        command,
    } = arguments;
    let selector = Selector {
        board,
        worktree,
        context,
        reviews,
    };
    selector.refuse_conflicts()?;
    match command {
        ScheduleCommand::List => list(client, selector, json).await,
        ScheduleCommand::Show { id } => {
            let schedule = find(client, selector, &id).await?;
            Ok(CommandOutput::success(schedule_text(&schedule, json)?))
        }
        ScheduleCommand::New(new_args) => new(client, selector, new_args, json).await,
        ScheduleCommand::Edit(edit_args) => edit(client, selector, edit_args, json).await,
        ScheduleCommand::Rm { id } => remove(client, selector, id, json).await,
        ScheduleCommand::Run { id, wait } => run(client, selector, id, wait, json).await,
        ScheduleCommand::Runs { id } => {
            let schedule = find(client, selector, &id).await?;
            let text = if json {
                to_json(&ScheduleEnvelope::new(&schedule))?
            } else {
                output::runs(&schedule)
            };
            Ok(CommandOutput::success(text))
        }
    }
}

async fn list(
    client: &Client,
    selector: Selector,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let view = selector.resolve(client).await?;
    let schedules = client.list_schedules(Some(view.board.id.clone())).await?;
    let text = if json {
        to_json(&SchedulesEnvelope::new(&schedules))?
    } else {
        output::list(&view.board.id, &schedules)
    };
    Ok(CommandOutput::success(text))
}

/// The schedule `id` names, checked against the selected board when a selector was given.
async fn find(
    client: &Client,
    selector: Selector,
    id: &ScheduleId,
) -> Result<Schedule, ProtoError> {
    let board = if selector.is_given() {
        Some(selector.resolve(client).await?.board.id)
    } else {
        None
    };
    let schedule = client
        .list_schedules(board.clone())
        .await?
        .into_iter()
        .find(|schedule| &schedule.id == id);
    match (schedule, board) {
        (Some(schedule), _) => Ok(schedule),
        (None, Some(board)) => Err(not_found(format!(
            "schedule not found: {id} on board {board}"
        ))),
        (None, None) => Err(not_found(format!("schedule not found: {id}"))),
    }
}

fn not_found(message: String) -> ProtoError {
    ProtoError {
        kind: ErrorKind::NotFound,
        message,
    }
}

async fn new(
    client: &Client,
    selector: Selector,
    arguments: ScheduleNewArgs,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    // Everything the flags can get wrong is refused before a board is ensured.
    let fields = NewFields::from_args(arguments)?;
    let view = selector.resolve(client).await?;
    let schedule = client.create_schedule(fields.draft(view.board.id)).await?;
    headed(&format!("Created {}", schedule.id), &schedule, json)
}

async fn edit(
    client: &Client,
    selector: Selector,
    arguments: ScheduleEditArgs,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let (id, mut patch, agent) = edit_request(arguments)?;
    let current = find(client, selector, &id).await?;
    // `SchedulePatch.agent` replaces the whole block, so `--model` alone would reset the
    // provider; the flags are written over the schedule's agent here, as `card edit` does.
    if !agent.is_empty() {
        patch.agent = Some(agent.over(current.agent));
    }
    let schedule = client.update_schedule(id, patch).await?;
    headed(&format!("Updated {}", schedule.id), &schedule, json)
}

async fn remove(
    client: &Client,
    selector: Selector,
    id: ScheduleId,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    // The lookup is what checks the schedule is on the selected board.
    let schedule = find(client, selector, &id).await?;
    client.delete_schedule(schedule.id.clone()).await?;
    let text = if json {
        to_json(&OkEnvelope {
            protocol: PROTOCOL,
            ok: true,
        })?
    } else {
        format!("Deleted {}", schedule.id)
    };
    Ok(CommandOutput::success(text))
}

/// Fires the schedule now; with `--wait`, polls it once a second until that run has an outcome.
///
/// A schedule run is a daemon job, but its outcome, summary and cost live on the schedule, and
/// the client has no job-wait call, so the wait reads the schedule back rather than the job.
async fn run(
    client: &Client,
    selector: Selector,
    id: ScheduleId,
    wait: bool,
    json: bool,
) -> Result<CommandOutput, ProtoError> {
    let schedule = find(client, selector, &id).await?;
    let mut schedule = client.run_schedule_now(schedule.id).await?;
    let mut started = started_run(&schedule)?.clone();
    if wait {
        while started.outcome.is_none() {
            tokio::time::sleep(WAIT_POLL).await;
            schedule = find(client, no_selector(), &id).await?;
            started = matching_run(&schedule, &started)?.clone();
        }
    }
    let exit_code = match started.outcome {
        Some(ScheduleOutcome::Failed | ScheduleOutcome::TimedOut) if wait => FAILURE,
        _ => 0,
    };
    let text = if json {
        to_json(&ScheduleEnvelope::new(&schedule))?
    } else {
        output::started(&schedule.id, &started, wait)
    };
    Ok(CommandOutput::with_exit_code(text, exit_code))
}

fn no_selector() -> Selector {
    Selector {
        board: None,
        worktree: None,
        context: None,
        reviews: false,
    }
}

/// The run `RunScheduleNow` recorded: the newest one on the schedule it answered with.
fn started_run(schedule: &Schedule) -> Result<&ScheduleRun, ProtoError> {
    schedule.runs.last().ok_or_else(|| ProtoError {
        kind: ErrorKind::Unknown,
        message: format!("the daemon recorded no run of schedule {}", schedule.id),
    })
}

/// `run` as the schedule now records it, matched by job and start time.
fn matching_run<'a>(
    schedule: &'a Schedule,
    run: &ScheduleRun,
) -> Result<&'a ScheduleRun, ProtoError> {
    schedule
        .runs
        .iter()
        .find(|candidate| candidate.started_at == run.started_at && candidate.job_id == run.job_id)
        .ok_or_else(|| {
            not_found(format!(
                "the run of schedule {} started at {} is no longer recorded",
                schedule.id, run.started_at
            ))
        })
}

fn schedule_text(schedule: &Schedule, json: bool) -> Result<String, ProtoError> {
    if json {
        to_json(&ScheduleEnvelope::new(schedule))
    } else {
        Ok(output::show(schedule))
    }
}

/// `heading` then the schedule's facts, or the schedule's envelope alone.
fn headed(heading: &str, schedule: &Schedule, json: bool) -> Result<CommandOutput, ProtoError> {
    let text = if json {
        schedule_text(schedule, json)?
    } else {
        format!("{heading}\n{}", output::show(schedule))
    };
    Ok(CommandOutput::success(text))
}

/// A validated `new`, before the board it goes on is known.
#[derive(Debug, PartialEq)]
struct NewFields {
    name: String,
    prompt: String,
    cadence: Cadence,
    agent: AgentFlags,
    disabled: bool,
    timeout: Option<u32>,
}

impl NewFields {
    fn from_args(arguments: ScheduleNewArgs) -> Result<Self, ProtoError> {
        let ScheduleNewArgs {
            name,
            prompt,
            prompt_file,
            starter,
            every,
            once,
            provider,
            model,
            effort,
            mode,
            timeout,
            disabled,
        } = arguments;
        // Clap requires one of each group; these refusals only guard a caller that bypassed it.
        let prompt = prompt_source(prompt, prompt_file.as_deref(), starter)?
            .ok_or_else(|| validation("one of --prompt, --prompt-file or --starter is required"))?;
        let cadence = cadence(every, once)
            .ok_or_else(|| validation("one of --every or --once is required"))?;
        Ok(Self {
            name,
            prompt,
            cadence,
            agent: AgentFlags::new(provider, model, effort, mode),
            disabled,
            timeout,
        })
    }

    fn draft(self, board_id: BoardId) -> ScheduleDraft {
        ScheduleDraft {
            board_id,
            name: self.name,
            prompt: self.prompt,
            cadence: self.cadence,
            agent: (!self.agent.is_empty()).then(|| self.agent.over(ScheduleAgent::default())),
            enabled: self.disabled.then_some(false),
            timeout_minutes: self.timeout,
        }
    }
}

/// The schedule id, the patch without its agent, and the agent flags to merge into it.
///
/// An edit that names nothing is refused here, before any request goes out.
fn edit_request(
    arguments: ScheduleEditArgs,
) -> Result<(ScheduleId, SchedulePatch, AgentFlags), ProtoError> {
    let ScheduleEditArgs {
        id,
        name,
        prompt,
        prompt_file,
        starter,
        every,
        once,
        provider,
        model,
        effort,
        mode,
        timeout,
        enable,
        disable,
    } = arguments;
    let patch = SchedulePatch {
        name,
        prompt: prompt_source(prompt, prompt_file.as_deref(), starter)?,
        cadence: cadence(every, once),
        agent: None,
        enabled: match (enable, disable) {
            (true, _) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
        },
        timeout_minutes: timeout,
    };
    let agent = AgentFlags::new(provider, model, effort, mode);
    if patch == SchedulePatch::default() && agent.is_empty() {
        return Err(validation(NOTHING_TO_CHANGE));
    }
    Ok((id, patch, agent))
}

/// The prompt one of the three mutually exclusive sources gives, or `None` when none was given.
fn prompt_source(
    prompt: Option<String>,
    prompt_file: Option<&Path>,
    starter: Option<ScheduleStarter>,
) -> Result<Option<String>, ProtoError> {
    if let Some(path) = prompt_file {
        return read_text(Some(path), "prompt").map(Some);
    }
    Ok(prompt.or_else(|| {
        starter.map(|starter| match starter {
            ScheduleStarter::GithubReviews => STARTER_PROMPT_GITHUB_REVIEWS.to_owned(),
        })
    }))
}

fn cadence(every: Option<u32>, once: Option<String>) -> Option<Cadence> {
    every
        .map(|minutes| Cadence::Every { minutes })
        .or_else(|| once.map(|at| Cadence::Once { at }))
}

/// The agent flags a command line gave; each one given replaces that field of a base agent.
#[derive(Debug, Default, PartialEq, Eq)]
struct AgentFlags {
    provider: Option<AgentKind>,
    /// Outer `None` means no flag; inner `None` means an explicit empty value clears the field.
    model: Option<Option<String>>,
    /// Outer `None` means no flag; inner `None` means an explicit empty value clears the field.
    effort: Option<Option<String>>,
    mode: Option<PermissionMode>,
}

impl AgentFlags {
    fn new(
        provider: Option<AgentChoice>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<AgentModeChoice>,
    ) -> Self {
        Self {
            provider: provider.map(|choice| match choice {
                AgentChoice::Claude => AgentKind::Claude,
                AgentChoice::Codex => AgentKind::Codex,
            }),
            model: model.map(|model| (!model.is_empty()).then_some(model)),
            effort: effort.map(|effort| (!effort.is_empty()).then_some(effort)),
            mode: mode.map(permission_mode),
        }
    }

    fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    fn over(self, base: ScheduleAgent) -> ScheduleAgent {
        let ScheduleAgent {
            provider: base_provider,
            model: base_model,
            effort: base_effort,
            mode: base_mode,
        } = base;
        let provider_changed = self
            .provider
            .is_some_and(|provider| provider != base_provider);
        let model = match self.model {
            Some(model) => model,
            None if provider_changed => None,
            None => base_model,
        };
        let effort = match self.effort {
            Some(effort) => effort,
            None if provider_changed => None,
            None => base_effort,
        };
        ScheduleAgent {
            provider: self.provider.unwrap_or(base_provider),
            model,
            effort,
            mode: self.mode.unwrap_or(base_mode),
        }
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
