//! Scheduled agent tasks: a board-owned prompt that runs headless on a cadence and records
//! what it found as cards on its board.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    agents::{AgentKind, PermissionMode},
    ids::{BoardId, JobId, ScheduleId},
};

#[cfg(test)]
mod tests;

/// Shortest interval an `every` cadence accepts, in minutes.
pub const SCHEDULE_MIN_EVERY_MINUTES: u32 = 5;
/// Longest interval an `every` cadence accepts, in minutes.
pub const SCHEDULE_MAX_EVERY_MINUTES: u32 = 1440;
/// Run timeout a new schedule gets when the draft names none, in minutes.
pub const SCHEDULE_DEFAULT_TIMEOUT_MINUTES: u32 = 20;
/// Longest run timeout a schedule accepts, in minutes.
pub const SCHEDULE_MAX_TIMEOUT_MINUTES: u32 = 120;
/// Runs kept per schedule; older ones are dropped with their logs.
pub const MAX_RUNS_PER_SCHEDULE: usize = 20;
/// Largest prompt a schedule accepts, in bytes.
pub const SCHEDULE_PROMPT_MAX_BYTES: usize = 16 * 1024;
/// Longest schedule name, in characters.
pub const SCHEDULE_NAME_MAX_CHARS: usize = 80;
/// Longest run summary kept, in characters.
pub const SCHEDULE_SUMMARY_MAX_CHARS: usize = 280;
/// Version written into `schedules.json`.
pub const SCHEDULES_DOCUMENT_VERSION: u32 = 1;

/// Appended after the rendered prompt of every run; `{fleet}` is the quoted path to `fleet`.
pub const SCHEDULE_FOOTER_TEMPLATE: &str = concat!(
    "--- Fleet scheduled task \"{name}\" for board {board} ---\n",
    "Record every pull request you are asked to review as a card on this board with:\n",
    "  {fleet} board --board {board} card new \"<pull request title>\" --pr <pull request URL> --requested-at <RFC 3339 time the review was requested> --label <source>\n",
    "<source> is one of the board's labels; run `{fleet} board --board {board} describe` to list them.\n",
    "The command prints \"Created <KEY>\", \"Existing <KEY>\" or \"Reopened <KEY>\" first; a pull request already on the board is never duplicated, so run it for every request you find.\n",
    "Only consider requests made after {last_run_at} when a source keeps old messages (chat channels, email).\n",
    "Do not review, comment on, or change any pull request. Do not edit files.\n",
    "End your reply with exactly one line: SUMMARY: <n> created, <n> existing, <n> reopened, <anything the user must know>",
);

/// The starter prompt that finds the GitHub pull requests waiting for the user's review.
pub const STARTER_PROMPT_GITHUB_REVIEWS: &str = "List every open pull request where my review is requested, directly or through one of my teams, with gh search prs --review-requested=@me --state=open --json url,title,repository,updatedAt. For each one, use its updatedAt as the requested time and github as the source label.";

/// A board-owned prompt that runs headless on a cadence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    /// Stable identifier, `sch-` + 8 hex.
    pub id: ScheduleId,
    /// The board whose cards this schedule writes.
    pub board_id: BoardId,
    /// Human name, 1..=80 characters.
    pub name: String,
    /// The user's prompt, before placeholders are rendered.
    pub prompt: String,
    /// When it fires.
    pub cadence: Cadence,
    /// Which agent runs it, and how.
    pub agent: ScheduleAgent,
    /// Whether it fires at all.
    pub enabled: bool,
    /// How long one run may take before it is stopped, in minutes.
    pub timeout_minutes: u32,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// RFC 3339 last-change time.
    pub updated_at: String,
    /// Recent runs, oldest first, at most `MAX_RUNS_PER_SCHEDULE`.
    #[serde(default)]
    pub runs: Vec<ScheduleRun>,
    /// RFC 3339 time of the next fire, as the daemon last computed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
}

/// When a schedule fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cadence {
    /// Every `minutes` minutes after the previous run started.
    Every {
        /// Interval, `SCHEDULE_MIN_EVERY_MINUTES..=SCHEDULE_MAX_EVERY_MINUTES`.
        minutes: u32,
    },
    /// Once, at an RFC 3339 time normalised to UTC seconds on create or edit.
    Once {
        /// UTC RFC 3339 time to the second.
        at: String,
    },
}

/// The agent a schedule runs, and with what permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleAgent {
    /// Harness that runs the prompt.
    pub provider: AgentKind,
    /// Harness-native model, or the configured default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning effort, or the harness default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Permission mode; a headless run has nobody to ask.
    pub mode: PermissionMode,
}

impl Default for ScheduleAgent {
    fn default() -> Self {
        Self {
            provider: AgentKind::Claude,
            model: None,
            effort: None,
            mode: PermissionMode::FullAccess,
        }
    }
}

/// One run of a schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRun {
    /// The daemon job that ran it; `None` for a skipped fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<JobId>,
    /// RFC 3339 start time.
    pub started_at: String,
    /// RFC 3339 end time; `None` while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// How it ended; `None` while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ScheduleOutcome>,
    /// The agent's `SUMMARY:` line, when it printed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Cost in US dollars, when the harness reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Absolute path to the run's log file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

/// How a schedule run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleOutcome {
    /// The agent finished.
    Succeeded,
    /// The agent or its launch failed.
    Failed,
    /// The run exceeded its timeout and was stopped.
    TimedOut,
    /// The fire came while the previous run was still going.
    Skipped,
}

/// What a caller supplies to create a schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDraft {
    /// The board the schedule belongs to.
    pub board_id: BoardId,
    /// Human name.
    pub name: String,
    /// The user's prompt.
    pub prompt: String,
    /// When it fires.
    pub cadence: Cadence,
    /// Agent; `ScheduleAgent::default()` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ScheduleAgent>,
    /// Whether it fires; `true` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Run timeout in minutes; `SCHEDULE_DEFAULT_TIMEOUT_MINUTES` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u32>,
}

/// A change to a schedule; `None` leaves the field unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulePatch {
    /// New name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// New cadence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cadence: Option<Cadence>,
    /// New agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ScheduleAgent>,
    /// Enable or disable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// New run timeout in minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u32>,
}

/// The persisted `schedules.json` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulesDocument {
    /// Document version; `SCHEDULES_DOCUMENT_VERSION` when written.
    pub version: u32,
    /// Every schedule, across all boards.
    #[serde(default)]
    pub schedules: Vec<Schedule>,
}

impl Default for SchedulesDocument {
    fn default() -> Self {
        Self {
            version: SCHEDULES_DOCUMENT_VERSION,
            schedules: Vec::new(),
        }
    }
}

/// Validation or lookup failure in a schedule operation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScheduleError {
    /// A named field violates its validation rules.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// Rejected field name.
        field: String,
        /// Explanation of the violated rule.
        reason: String,
    },
    /// No schedule has this id.
    #[error("schedule not found: {0}")]
    NotFound(String),
}

/// Checks every field rule of a schedule except its board, which the daemon checks.
pub fn validate_schedule(schedule: &Schedule) -> Result<(), ScheduleError> {
    if schedule.name.trim().is_empty() {
        return Err(invalid("name", "must not be empty"));
    }
    if schedule.name.contains('\0') {
        return Err(invalid("name", "must not contain a NUL byte"));
    }
    if schedule.name.chars().count() > SCHEDULE_NAME_MAX_CHARS {
        return Err(invalid("name", "must be at most 80 characters"));
    }
    if schedule.prompt.trim().is_empty() {
        return Err(invalid("prompt", "must not be empty"));
    }
    if schedule.prompt.contains('\0') {
        return Err(invalid("prompt", "must not contain a NUL byte"));
    }
    if schedule.prompt.len() > SCHEDULE_PROMPT_MAX_BYTES {
        return Err(invalid("prompt", "must be at most 16 KiB"));
    }
    match &schedule.cadence {
        Cadence::Every { minutes } => {
            if !(SCHEDULE_MIN_EVERY_MINUTES..=SCHEDULE_MAX_EVERY_MINUTES).contains(minutes) {
                return Err(invalid(
                    "cadence",
                    "every must be between 5 and 1440 minutes",
                ));
            }
        }
        Cadence::Once { at } => {
            if parse_time(at).is_none() {
                return Err(invalid("cadence", "once needs an RFC 3339 time"));
            }
        }
    }
    if !(1..=SCHEDULE_MAX_TIMEOUT_MINUTES).contains(&schedule.timeout_minutes) {
        return Err(invalid("timeout_minutes", "must be between 1 and 120"));
    }
    let agent = &schedule.agent;
    if agent
        .model
        .as_ref()
        .is_some_and(|model| model.trim().is_empty())
    {
        return Err(invalid("model", "must not be blank"));
    }
    if agent
        .effort
        .as_ref()
        .is_some_and(|effort| effort.trim().is_empty())
    {
        return Err(invalid("effort", "must not be blank"));
    }
    if !agent.provider.supported_modes().contains(&agent.mode) {
        return Err(invalid(
            "mode",
            &format!(
                "{} is not supported by {}",
                agent.mode,
                agent.provider.executable()
            ),
        ));
    }
    Ok(())
}

/// Builds a schedule from a draft, filling the defaults and normalising a valid `Once` time.
#[must_use]
pub fn apply_draft(draft: ScheduleDraft, id: ScheduleId, now: &str) -> Schedule {
    Schedule {
        id,
        board_id: draft.board_id,
        name: draft.name,
        prompt: draft.prompt,
        cadence: normalize_cadence(draft.cadence),
        agent: draft.agent.unwrap_or_default(),
        enabled: draft.enabled.unwrap_or(true),
        timeout_minutes: draft
            .timeout_minutes
            .unwrap_or(SCHEDULE_DEFAULT_TIMEOUT_MINUTES),
        created_at: now.to_owned(),
        updated_at: now.to_owned(),
        runs: Vec::new(),
        next_run_at: None,
    }
}

/// Applies every `Some` field, normalises a valid `Once` time, and stamps `updated_at`.
pub fn apply_patch(schedule: &mut Schedule, patch: SchedulePatch, now: &str) {
    let SchedulePatch {
        name,
        prompt,
        cadence,
        agent,
        enabled,
        timeout_minutes,
    } = patch;
    if let Some(name) = name {
        schedule.name = name;
    }
    if let Some(prompt) = prompt {
        schedule.prompt = prompt;
    }
    if let Some(cadence) = cadence {
        schedule.cadence = normalize_cadence(cadence);
    }
    if let Some(agent) = agent {
        schedule.agent = agent;
    }
    if let Some(enabled) = enabled {
        schedule.enabled = enabled;
    }
    if let Some(timeout_minutes) = timeout_minutes {
        schedule.timeout_minutes = timeout_minutes;
    }
    now.clone_into(&mut schedule.updated_at);
}

/// When the schedule fires next; one catch-up run after downtime, never several.
///
/// A disabled schedule never fires. A `Once` fires at `max(at, now)` until any run has started
/// at or after `at`. An `Every` fires `minutes` after the lesser of its last start and `now`, or
/// at `now` when it never ran (a stored time that does not parse counts as never ran).
#[must_use]
pub fn next_run_at(schedule: &Schedule, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if !schedule.enabled {
        return None;
    }
    match &schedule.cadence {
        Cadence::Once { at } => {
            let at = parse_time(at)?;
            let fired = schedule
                .runs
                .iter()
                .filter_map(|run| parse_time(&run.started_at))
                .any(|started_at| started_at >= at);
            (!fired).then_some(at.max(now))
        }
        Cadence::Every { minutes } => {
            let due = last_started_at(schedule)
                .map(|last| last.min(now))
                .and_then(|last| last.checked_add_signed(Duration::minutes(i64::from(*minutes))))
                .unwrap_or(now);
            Some(due.max(now))
        }
    }
}

/// Appends a run and drops the oldest *finished* runs past `MAX_RUNS_PER_SCHEDULE`, returning
/// the dropped runs' log paths so the caller can delete those files.
///
/// A run without an outcome is still going: its child writes to its log and its outcome is
/// recorded onto it when it ends, so it is never dropped. A burst of skipped fires during one
/// long run therefore drops older finished runs, and the list may briefly hold one more run
/// than the cap only when every run in it is live, which the one-live-run rule rules out.
pub fn push_run(schedule: &mut Schedule, run: ScheduleRun) -> Vec<String> {
    schedule.runs.push(run);
    let mut dropped = Vec::new();
    while schedule.runs.len() > MAX_RUNS_PER_SCHEDULE {
        let Some(oldest) = schedule.runs.iter().position(|run| run.outcome.is_some()) else {
            break;
        };
        if let Some(log_path) = schedule.runs.remove(oldest).log_path {
            dropped.push(log_path);
        }
    }
    dropped
}

/// The prompt a run receives: the user's prompt with `{board}`, `{last_run_at}` and `{now}`
/// substituted, two newlines, then the rendered `SCHEDULE_FOOTER_TEMPLATE`.
///
/// `{last_run_at}` is the previous run's `started_at`, or `never`. A `Skipped` fire launched no
/// agent and read no source, so it is not a previous run: counting it would tell the next run
/// to ignore requests that arrived while the live run was already past them. Substitution is a
/// single pass, so a value that itself contains a placeholder is never expanded again.
#[must_use]
pub fn render_prompt(schedule: &Schedule, fleet: &str, now: &str) -> String {
    let board = schedule.board_id.as_str();
    let last_run_at = schedule
        .runs
        .iter()
        .rev()
        .find(|run| run.outcome != Some(ScheduleOutcome::Skipped))
        .map_or("never", |run| run.started_at.as_str());
    let prompt = substitute(
        &schedule.prompt,
        &[("board", board), ("last_run_at", last_run_at), ("now", now)],
    );
    let footer = substitute(
        SCHEDULE_FOOTER_TEMPLATE,
        &[
            ("name", schedule.name.as_str()),
            ("board", board),
            ("fleet", fleet),
            ("last_run_at", last_run_at),
        ],
    );
    format!("{prompt}\n\n{footer}")
}

/// The last line of a run's final message that starts with `SUMMARY:`, prefix and whitespace
/// trimmed, cut to `SCHEDULE_SUMMARY_MAX_CHARS` at a char boundary. `None` when no line
/// carries one, or the last one is empty.
#[must_use]
pub fn summary_line(final_message: &str) -> Option<String> {
    let summary = final_message
        .lines()
        .rev()
        .find_map(|line| line.trim_start().strip_prefix(SUMMARY_PREFIX))?
        .trim();
    if summary.is_empty() {
        return None;
    }
    Some(summary.chars().take(SCHEDULE_SUMMARY_MAX_CHARS).collect())
}

/// The prefix of the line a run ends its reply with.
const SUMMARY_PREFIX: &str = "SUMMARY:";

fn invalid(field: &str, reason: &str) -> ScheduleError {
    ScheduleError::Invalid {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn normalize_cadence(cadence: Cadence) -> Cadence {
    match cadence {
        Cadence::Once { at } => Cadence::Once {
            at: parse_time(&at).map_or(at, |time| time.to_rfc3339_opts(SecondsFormat::Secs, true)),
        },
        cadence => cadence,
    }
}

fn last_started_at(schedule: &Schedule) -> Option<DateTime<Utc>> {
    schedule
        .runs
        .last()
        .and_then(|run| parse_time(&run.started_at))
}

/// Replaces each `{key}` of `template` with its value in one left-to-right pass; an unknown
/// `{…}` is kept as written.
fn substitute(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let replaced = values.iter().find_map(|(key, value)| {
            after
                .strip_prefix(key)
                .and_then(|tail| tail.strip_prefix('}'))
                .map(|tail| (*value, tail))
        });
        match replaced {
            Some((value, tail)) => {
                out.push_str(value);
                rest = tail;
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
