//! Human text and protocol-one envelopes for `fleet schedule`.
//!
//! Times print as `2026-09-23 09:00`, the RFC 3339 value with its seconds dropped; the daemon
//! records UTC, so a zone suffix appears only for a time written with another offset.

use crate::{envelope::PROTOCOL, human};
use fleet_core::{
    agents::AgentKind,
    ids::{BoardId, ScheduleId},
    schedule::{Cadence, Schedule, ScheduleOutcome, ScheduleRun},
};
use serde::Serialize;

/// How many runs `show` lists; `runs` lists every one the daemon keeps.
pub(super) const SHOW_RUNS: usize = 5;

/// What an empty cell prints.
const EMPTY: &str = "—";

/// One schedule in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub(super) struct ScheduleEnvelope<'a> {
    pub protocol: u32,
    pub schedule: &'a Schedule,
}

impl<'a> ScheduleEnvelope<'a> {
    pub(super) const fn new(schedule: &'a Schedule) -> Self {
        Self {
            protocol: PROTOCOL,
            schedule,
        }
    }
}

/// A board's schedules in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub(super) struct SchedulesEnvelope<'a> {
    pub protocol: u32,
    pub schedules: &'a [Schedule],
}

impl<'a> SchedulesEnvelope<'a> {
    pub(super) const fn new(schedules: &'a [Schedule]) -> Self {
        Self {
            protocol: PROTOCOL,
            schedules,
        }
    }
}

/// A header row, then `id  name  cadence  next  last outcome  last summary` per schedule, as
/// `fleet board list` heads its table.
pub(super) fn list(board: &BoardId, schedules: &[Schedule]) -> String {
    if schedules.is_empty() {
        return format!("No schedules on board {board}");
    }
    let mut rows = vec![header(&[
        "ID",
        "NAME",
        "CADENCE",
        "NEXT",
        "LAST OUTCOME",
        "LAST SUMMARY",
    ])];
    rows.extend(schedules.iter().map(list_row));
    human::columns(&rows).join("\n")
}

fn header(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|cell| (*cell).to_owned()).collect()
}

pub(super) fn list_row(schedule: &Schedule) -> Vec<String> {
    let last = schedule.runs.last();
    vec![
        schedule.id.to_string(),
        schedule.name.clone(),
        cadence(&schedule.cadence),
        schedule.next_run_at.as_deref().map_or_else(empty, time),
        last.map_or_else(empty, |run| outcome(run).to_owned()),
        last.map_or_else(empty, summary),
    ]
}

/// `every 15m` or `once 2026-09-23 09:00`.
pub(super) fn cadence(cadence: &Cadence) -> String {
    match cadence {
        Cadence::Every { minutes } => format!("every {minutes}m"),
        Cadence::Once { at } => format!("once {}", time(at)),
    }
}

/// The schedule's fact lines, its prompt, and its last five runs in chronological order.
pub(super) fn show(schedule: &Schedule) -> String {
    let agent = &schedule.agent;
    let mut agent_cell = match agent.provider {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    }
    .to_owned();
    if let Some(model) = &agent.model {
        agent_cell.push_str(&format!(" \u{b7} model {model}"));
    }
    if let Some(effort) = &agent.effort {
        agent_cell.push_str(&format!(" \u{b7} effort {effort}"));
    }
    agent_cell.push_str(&format!(" \u{b7} {}", agent.mode.word()));
    let facts = [
        vec!["id:".to_owned(), schedule.id.to_string()],
        vec!["name:".to_owned(), schedule.name.clone()],
        vec!["board:".to_owned(), schedule.board_id.to_string()],
        vec![
            "enabled:".to_owned(),
            if schedule.enabled { "yes" } else { "no" }.to_owned(),
        ],
        vec!["cadence:".to_owned(), cadence(&schedule.cadence)],
        vec![
            "next:".to_owned(),
            schedule.next_run_at.as_deref().map_or_else(empty, time),
        ],
        vec!["agent:".to_owned(), agent_cell],
        vec![
            "timeout:".to_owned(),
            format!("{}m", schedule.timeout_minutes),
        ],
        vec!["created:".to_owned(), time(&schedule.created_at)],
        vec!["updated:".to_owned(), time(&schedule.updated_at)],
    ];
    let mut lines = human::columns(&facts);
    lines.push("prompt:".to_owned());
    lines.extend(schedule.prompt.lines().map(|line| format!("  {line}")));
    let total = schedule.runs.len();
    if total == 0 {
        lines.push("runs: none yet".to_owned());
    } else {
        let shown = total.min(SHOW_RUNS);
        lines.push(format!("runs (last {shown} of {total}):"));
        let rows = schedule
            .runs
            .get(total.saturating_sub(SHOW_RUNS)..)
            .unwrap_or(&[])
            .iter()
            .map(|run| {
                let mut row = run_cells(run);
                row.push(summary(run));
                row
            })
            .collect::<Vec<_>>();
        lines.extend(
            human::columns(&rows)
                .into_iter()
                .map(|row| format!("  {row}")),
        );
    }
    lines.join("\n")
}

/// A header row, then every recorded run, newest last as `fleet board card runs` lists a
/// card's, with its log path so a user can `less` it.
pub(super) fn runs(schedule: &Schedule) -> String {
    if schedule.runs.is_empty() {
        return format!("No runs of schedule {} yet", schedule.id);
    }
    let mut rows = vec![header(&[
        "STARTED", "OUTCOME", "JOB", "COST", "LOG", "SUMMARY",
    ])];
    rows.extend(schedule.runs.iter().map(|run| {
        let mut row = run_cells(run);
        row.push(run.log_path.clone().unwrap_or_else(empty));
        row.push(summary(run));
        row
    }));
    human::columns(&rows).join("\n")
}

/// `started  outcome  job  cost`; each printer adds its own trailing cells.
fn run_cells(run: &ScheduleRun) -> Vec<String> {
    vec![
        time(&run.started_at),
        outcome(run).to_owned(),
        run.job_id.as_ref().map_or_else(empty, ToString::to_string),
        run.cost_usd
            .map_or_else(empty, |cost| format!("${cost:.2}")),
    ]
}

fn summary(run: &ScheduleRun) -> String {
    run.summary.clone().unwrap_or_else(empty)
}

/// The line `run` prints: `Started {job id}`, then with `--wait` how the run ended.
pub(super) fn started(id: &ScheduleId, run: &ScheduleRun, waited: bool) -> String {
    let heading = match (&run.job_id, run.outcome) {
        (_, Some(ScheduleOutcome::Skipped)) | (None, _) => {
            let reason = run.summary.as_deref().unwrap_or("no job was started");
            return format!("Skipped {id}: {reason}");
        }
        (Some(job), _) => format!("Started {job}"),
    };
    if !waited {
        return heading;
    }
    let summary = run.summary.as_deref().unwrap_or(EMPTY);
    format!("{heading}\n{}: {summary}", outcome(run))
}

/// The outcome word, or `running` while the run has none.
fn outcome(run: &ScheduleRun) -> &'static str {
    match run.outcome {
        None => "running",
        Some(ScheduleOutcome::Succeeded) => "succeeded",
        Some(ScheduleOutcome::Failed) => "failed",
        Some(ScheduleOutcome::TimedOut) => "timed out",
        Some(ScheduleOutcome::Skipped) => "skipped",
    }
}

/// `2026-09-23T09:00:00Z` as `2026-09-23 09:00`; a value that is not RFC 3339 prints as given.
pub(super) fn time(value: &str) -> String {
    let (Some(date), Some(separator), Some(clock)) =
        (value.get(..10), value.get(10..11), value.get(11..16))
    else {
        return value.to_owned();
    };
    if !matches!(separator, "T" | "t" | " ") {
        return value.to_owned();
    }
    let zone = if value.ends_with(['Z', 'z']) || value.ends_with("+00:00") {
        ""
    } else {
        value
            .len()
            .checked_sub(6)
            .and_then(|start| value.get(start..))
            .filter(|zone| zone.starts_with(['+', '-']))
            .unwrap_or("")
    };
    format!("{date} {clock}{zone}")
}

fn empty() -> String {
    EMPTY.to_owned()
}
