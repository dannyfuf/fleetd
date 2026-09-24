//! The board header's schedules strip (BOARD §11.8, UX-SPEC § Board header).
//!
//! `⟳ GitHub reviews · next 14:05 · last 3 created` for one schedule, `⟳ 3 schedules · next
//! 14:05` for several. The words are folded here, from the schedules mirror and the clock, when
//! the projection rebuilds — never in render — and the header only draws them.

use chrono::{DateTime, Local, TimeZone, Utc};
use fleet_core::{
    ids::ScheduleId,
    schedule::{Schedule, ScheduleOutcome, ScheduleRun},
};
use fleet_ui_kit::{Truncate, truncate};
use gpui::SharedString;

/// The widest a run's summary clause may be in the strip, in `ch`: an agent that printed no
/// `SUMMARY:` line leaves its last output line there, up to the daemon's 280 characters, and
/// the strip is one compact line beside the header's subtitle.
pub const SUMMARY_BUDGET: usize = 40;

/// What the header's schedules strip says, already composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleStrip {
    /// `⟳ GitHub reviews · next 14:05 · last 3 created`.
    pub label: SharedString,
    /// Whether a schedule's last run failed or timed out: the strip takes the warning tone.
    pub failed: bool,
}

impl ScheduleStrip {
    /// The strip for a board's schedules, in the local time zone, or `None` when there is
    /// nothing to show: no schedule, or a daemon without the `schedules` capability.
    ///
    /// `now` is the epoch second the next run is read against; the projection keys it by the
    /// minute while the board has schedules, so `next now` and a day rollover stay current.
    #[must_use]
    pub fn of(supported: bool, schedules: &[Schedule], now: i64) -> Option<Self> {
        Self::in_zone(supported, schedules, now, &Local)
    }

    /// [`Self::of`] in a given time zone, so the words are testable without the host's.
    #[must_use]
    pub(crate) fn in_zone<Tz: TimeZone>(
        supported: bool,
        schedules: &[Schedule],
        now: i64,
        zone: &Tz,
    ) -> Option<Self>
    where
        Tz::Offset: std::fmt::Display,
    {
        if !supported || schedules.is_empty() {
            return None;
        }
        let now = DateTime::<Utc>::from_timestamp(now, 0).unwrap_or(DateTime::UNIX_EPOCH);
        let next = schedules
            .iter()
            .filter(|schedule| schedule.enabled)
            .filter_map(|schedule| schedule.next_run_at.as_deref())
            .filter_map(parse_time)
            .min();
        let next = if schedules.iter().any(|schedule| schedule.enabled) {
            next.map(|at| format!("next {}", clock_label(at, now, zone)))
        } else {
            Some("disabled".to_owned())
        };
        let mut parts: Vec<String> = Vec::with_capacity(3);
        match schedules {
            [one] => {
                parts.push(format!("\u{27f3} {}", one.name));
                parts.extend(next);
                parts.extend(last_run(one).map(last_phrase));
            }
            several => {
                parts.push(format!("\u{27f3} {} schedules", several.len()));
                parts.extend(next);
            }
        }
        Some(Self {
            label: SharedString::from(parts.join(" \u{b7} ")),
            failed: schedules.iter().filter_map(last_run).any(run_failed),
        })
    }
}

/// The schedules `R` fires: every enabled one, in the mirror's order.
#[must_use]
pub fn runnable_schedules(schedules: &[Schedule]) -> Vec<ScheduleId> {
    schedules
        .iter()
        .filter(|schedule| schedule.enabled)
        .map(|schedule| schedule.id.clone())
        .collect()
}

/// The run the strip speaks of: the newest one that is not a skipped fire.
///
/// A skip only says the previous run was still going when the cadence came round again; the
/// strip reports what that previous run did, not that one fire was dropped.
fn last_run(schedule: &Schedule) -> Option<&ScheduleRun> {
    schedule
        .runs
        .iter()
        .rev()
        .find(|run| run.outcome != Some(ScheduleOutcome::Skipped))
}

/// Whether a run ended badly: failed, or stopped at its timeout.
fn run_failed(run: &ScheduleRun) -> bool {
    matches!(
        run.outcome,
        Some(ScheduleOutcome::Failed | ScheduleOutcome::TimedOut)
    )
}

/// `last 3 created` — the first clause of the run's `SUMMARY:` line — or what the run did when
/// it printed none; `running` while it is still going.
fn last_phrase(run: &ScheduleRun) -> String {
    let word = match run.outcome {
        None if run.ended_at.is_none() => return "running".to_owned(),
        Some(ScheduleOutcome::Failed) => return "last failed".to_owned(),
        Some(ScheduleOutcome::TimedOut) => return "last timed out".to_owned(),
        Some(ScheduleOutcome::Skipped) => "skipped",
        Some(ScheduleOutcome::Succeeded) | None => "succeeded",
    };
    run.summary
        .as_deref()
        .and_then(|summary| summary.split(',').next())
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .map_or_else(
            || format!("last {word}"),
            |clause| format!("last {}", truncate(clause, SUMMARY_BUDGET, Truncate::Tail)),
        )
}

/// An RFC 3339 stamp as UTC, or `None` when the daemon wrote something unreadable.
fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

/// When the next run fires, as a wall clock reads it: `14:05` today, `Sep 25 14:05` on another
/// day, and `now` once the time has come and the daemon has not fired it yet.
fn clock_label<Tz: TimeZone>(at: DateTime<Utc>, now: DateTime<Utc>, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    if at <= now {
        return "now".to_owned();
    }
    let at = at.with_timezone(zone);
    let today = now.with_timezone(zone).date_naive();
    if at.date_naive() == today {
        at.format("%H:%M").to_string()
    } else {
        at.format("%b %-d %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests;
