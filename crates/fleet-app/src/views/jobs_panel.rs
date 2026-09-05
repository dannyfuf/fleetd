//! The pieces the Jobs panel of UX-SPEC §3.7 is made of.
//!
//! [`crate::screens::jobs::JobsPanel`] owns the keyboard, the cursor and the daemon calls; this
//! module owns everything that is a *pure function of the job list* — which rows the `f` filter
//! lets through, what the header counts say, how an elapsed time is spelled, which rows carry a
//! progress sub-line — plus the kit composition that turns one [`JobRecord`] into a row.
//!
//! Splitting it this way is what makes §3.7 testable without a window: every decision below is
//! a plain function over `&[JobRecord]`.

use std::{collections::HashSet, path::Path};

use fleet_core::ids::JobId;
use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use fleet_ui_kit::{
    ActiveTheme, Chip, EmptyState, Icon, IconSize, JobRow, JobStatus as RowStatus, KeyHint,
    KeyHintRow, PaneHeader, Text, Tone, format_age,
};
use gpui::{AnyElement, App, SharedString, div, prelude::*};

use crate::shell::{domain_target, job_kind_label};

/// The fact line of the empty Jobs panel (§3.13), verbatim.
pub const EMPTY_FACT: &str = "Nothing running.";
/// The second line of the empty Jobs panel (§3.13), verbatim. It is the product promise, stated
/// once, in the one place that is demonstrating it.
pub const EMPTY_ACTION: &str =
    "Jobs and sessions live in fleetd, so they survive closing this window.";
/// The amber strip the panel grows while the daemon is unreachable (§3.7 "States").
pub const DAEMON_DOWN_PREFIX: &str = "The daemon is unreachable — job state is from";
/// How long a job must run before its elapsed time is worth showing (§3.7).
pub const ELAPSED_AFTER_SECONDS: i64 = 30;

// ---------------------------------------------------------------------------- the `f` filter

/// The three positions of the `f` filter (§3.7: all → running → failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobFilter {
    /// Every retained job.
    #[default]
    All,
    /// Queued, running and cancelling jobs.
    Running,
    /// Failed jobs only.
    Failed,
}

impl JobFilter {
    /// The next position of the cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::All => Self::Running,
            Self::Running => Self::Failed,
            Self::Failed => Self::All,
        }
    }

    /// The word the pane header shows, or `None` for the unfiltered default (§1.2).
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Running => Some("running"),
            Self::Failed => Some("failed"),
        }
    }

    /// Whether a job passes this filter.
    #[must_use]
    pub fn matches(self, job: &JobRecord) -> bool {
        match self {
            Self::All => true,
            Self::Running => is_active(&job.status),
            Self::Failed => matches!(job.status, JobStatus::Failed { .. }),
        }
    }
}

/// Whether a job is still doing something: queued, running or shutting down.
#[must_use]
pub const fn is_active(status: &JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
    )
}

/// Whether `D` may dismiss a job: §3.7 dismisses *finished and failed* jobs, never live ones.
#[must_use]
pub const fn is_dismissable(status: &JobStatus) -> bool {
    !is_active(status)
}

/// The jobs the panel draws, in daemon order, after the filter and the dismiss set.
#[must_use]
pub fn visible_jobs<'a>(
    jobs: &'a [JobRecord],
    filter: JobFilter,
    dismissed: &HashSet<JobId>,
) -> Vec<&'a JobRecord> {
    jobs.iter()
        .filter(|job| !dismissed.contains(&job.id) && filter.matches(job))
        .collect()
}

// ---------------------------------------------------------------------------- header counts

/// The `⟳n running · ✕n failed · ✓n done` counters of the panel header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobCounts {
    /// Queued, running and cancelling.
    pub running: usize,
    /// Failed, and never auto-dismissed ([D-9]).
    pub failed: usize,
    /// Succeeded and cancelled — the evidence of what ran.
    pub done: usize,
}

impl JobCounts {
    /// Whether the header has anything to say at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.running == 0 && self.failed == 0 && self.done == 0
    }
}

/// Counts the header's three numbers.
#[must_use]
pub fn job_counts(jobs: &[JobRecord]) -> JobCounts {
    let mut counts = JobCounts::default();
    for job in jobs {
        match &job.status {
            JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling => counts.running += 1,
            JobStatus::Failed { .. } => counts.failed += 1,
            JobStatus::Succeeded | JobStatus::Cancelled => counts.done += 1,
        }
    }
    counts
}

// ---------------------------------------------------------------------------- time

/// Parses the daemon's RFC 3339 timestamps into whole seconds since the Unix epoch.
///
/// The daemon writes `chrono::Utc::now().to_rfc3339()`, so the input is always
/// `YYYY-MM-DDThh:mm:ss[.fraction](Z|±hh:mm)`. Anything else returns `None`, which every caller
/// renders as "no age yet" — never as `0` (§1.3).
#[must_use]
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: i64 = text.get(5..7)?.parse().ok()?;
    let day: i64 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    seconds -= offset_seconds(text.get(19..).unwrap_or(""));
    Some(seconds)
}

/// The trailing `Z` or `±hh:mm` of an RFC 3339 timestamp, in seconds east of UTC.
fn offset_seconds(tail: &str) -> i64 {
    let tail = tail.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let sign = match tail.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return 0,
    };
    let Some((hours, minutes)) = tail.get(1..).and_then(|rest| rest.split_once(':')) else {
        return 0;
    };
    let hours: i64 = hours.parse().unwrap_or(0);
    let minutes: i64 = minutes.parse().unwrap_or(0);
    sign * (hours * 3_600 + minutes * 60)
}

/// Days between 1970-01-01 and the given civil date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Wall-clock seconds since the Unix epoch, or `0` when the clock is before the epoch.
#[must_use]
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// How long a job has been running, or how long it took, in seconds.
#[must_use]
pub fn elapsed_seconds(job: &JobRecord, now: i64) -> Option<i64> {
    let started = parse_timestamp(&job.started_at)?;
    let end = match job.finished_at.as_deref() {
        Some(finished) => parse_timestamp(finished)?,
        None => now,
    };
    Some((end - started).max(0))
}

/// The elapsed column of §3.7: `m:ss` for live jobs after 30 s, a single-unit age for finished
/// ones, and the en dash for a cancelled job — never `0`.
#[must_use]
pub fn elapsed_label(job: &JobRecord, now: i64) -> Option<String> {
    match &job.status {
        JobStatus::Cancelled => Some("\u{2013}".to_owned()),
        status if is_active(status) => {
            let seconds = elapsed_seconds(job, now)?;
            (seconds >= ELAPSED_AFTER_SECONDS)
                .then(|| format!("{}:{:02}", seconds / 60, seconds % 60))
        }
        _ => elapsed_seconds(job, now).map(format_age),
    }
}

// ---------------------------------------------------------------------------- row content

/// The kit's glyph for a wire job status.
#[must_use]
pub const fn row_status(status: &JobStatus) -> RowStatus {
    match status {
        JobStatus::Queued => RowStatus::Queued,
        JobStatus::Running => RowStatus::Running,
        JobStatus::Cancelling => RowStatus::Cancelling,
        JobStatus::Cancelled => RowStatus::Cancelled,
        JobStatus::Succeeded => RowStatus::Done,
        JobStatus::Failed { .. } => RowStatus::Failed,
    }
}

/// The 7-character kind slug of the second column.
#[must_use]
pub fn kind_slug(kind: &JobKind) -> &str {
    job_kind_label(kind)
}

/// The sub-line of §3.7: the last stdout line of a job that is still doing something.
///
/// Finished jobs collapse to a single 30 px line ([D-9]), so their progress is deliberately
/// dropped; a failed job says its error instead, which is the one line that decides `R`.
#[must_use]
pub fn sub_line(job: &JobRecord) -> Option<&str> {
    match &job.status {
        JobStatus::Failed { error } => Some(error.as_str()),
        status if is_active(status) => job.progress.as_deref(),
        _ => None,
    }
}

/// The percent the row shows, when the last progress line contains one.
#[must_use]
pub fn percent(job: &JobRecord) -> Option<u8> {
    if !is_active(&job.status) {
        return None;
    }
    job.progress
        .as_deref()
        .and_then(crate::state::parse_percent)
}

/// Rewrites a path under `$HOME` as `~/…`, which is how §3.7 prints the log path.
#[must_use]
pub fn tilde(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.to_owned();
    };
    let home = home.to_string_lossy();
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_owned();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// The user's home directory, for [`tilde`].
#[must_use]
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

// ---------------------------------------------------------------------------- elements

/// The panel header: `JOBS` plus the three zero-suppressed counters (§3.7).
#[must_use]
pub fn header(
    counts: JobCounts,
    filter: JobFilter,
    shown: usize,
    total: usize,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let chips = div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .child(
            Chip::counter(Icon::LoaderCircle, counts.running)
                .tone(Tone::Warning)
                .spinning(true)
                .id("jobs-panel-running"),
        )
        .child(Chip::counter(Icon::CircleX, counts.failed).tone(Tone::Danger))
        .child(Chip::counter(Icon::CircleCheck, counts.done).tone(Tone::Success));

    let mut header = PaneHeader::new("jobs").total(total).trailing(chips);
    if let Some(label) = filter.label() {
        header = header.scope(label).shown(shown);
    }
    div()
        .flex_none()
        .h(theme.metrics.pane_header_h)
        .w_full()
        .child(header)
        .into_any_element()
}

/// The header's second row: the selected job's log path, verbatim, with the `y` that copies it.
///
/// This is what makes a failure survivable **outside** the app, so it renders even when the
/// selected job succeeded.
#[must_use]
pub fn log_path_row(path: Option<SharedString>, cx: &App) -> AnyElement {
    let Some(path) = path else {
        return div().into_any_element();
    };
    let theme = cx.theme();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(theme.space.md)
        .px(theme.space.lg)
        .h(theme.metrics.strip_h)
        .child(Text::data_small(path).faint().ellipsize())
        .child(KeyHint::new("y"))
        .into_any_element()
}

/// One job row.
#[must_use]
pub fn job_row(job: &JobRecord, cursor: bool, now: i64) -> AnyElement {
    let mut row = JobRow::new(
        row_status(&job.status),
        kind_slug(&job.kind).to_owned(),
        // §3.7: the row names the domain id; the job id lives in the log path and on `y`.
        domain_target(&job.target).to_owned(),
    )
    .id(SharedString::from(format!("job-{}", job.id.as_str())))
    .selected(cursor)
    .cursor(cursor);

    if let Some(elapsed) = elapsed_label(job, now) {
        row = row.elapsed(elapsed);
    }
    if let Some(percent) = percent(job) {
        row = row.percent(percent);
    }
    if let Some(line) = sub_line(job) {
        row = row.progress(line.to_owned());
    }
    if matches!(job.status, JobStatus::Failed { .. }) && job.retryable {
        row = row.trailing_key("R");
    }
    row.into_any_element()
}

/// The panel's two pinned key rows (§3.7). Inside an expanded log, `f` means *follow*.
#[must_use]
pub fn footer(expanded: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let second = if expanded {
        KeyHintRow::new()
            .key("f", "follow")
            .key("G", "tail")
            .key("esc", "collapse")
    } else {
        KeyHintRow::new()
            .key("f", "filter")
            .key("D", "dismiss")
            .key("X", "cancel all")
            .key("esc", "close")
    };
    div()
        .flex()
        .flex_none()
        .flex_col()
        .px(theme.space.lg)
        .py(theme.space.xs)
        .gap(theme.space.xxs)
        .border_t(gpui::px(1.0))
        .border_color(theme.colors.border)
        .child(
            KeyHintRow::new()
                .key("\u{23CE}", "log")
                .key("c", "cancel")
                .key("R", "retry")
                .key("y", "copy log path"),
        )
        .child(second)
        .into_any_element()
}

/// The empty state of §3.13, rendered in the panel and nowhere else.
#[must_use]
pub fn empty_state(filter: JobFilter) -> AnyElement {
    match filter.label() {
        Some(label) => EmptyState::new(format!("No {label} jobs."))
            .action("f  show every job")
            .into_any_element(),
        None => EmptyState::new(EMPTY_FACT)
            .action(EMPTY_ACTION)
            .into_any_element(),
    }
}

/// The strip §3.7 puts above the rows while the daemon is unreachable: the rows stay readable,
/// they are simply stamped with their age.
#[must_use]
pub fn daemon_down_strip(age_seconds: u64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let age = format_age(i64::try_from(age_seconds).unwrap_or(i64::MAX));
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.space.sm)
        .px(theme.space.lg)
        .h(theme.metrics.banner_h)
        .bg(Tone::Warning.fill(theme))
        .child(
            Icon::Unplug
                .el()
                .size(IconSize::Small)
                .color(theme.colors.warning),
        )
        .child(Text::ui(format!("{DAEMON_DOWN_PREFIX} {age} ago")).tone(Tone::Warning))
        .into_any_element()
}

/// The in-panel confirmation of `X` (§3.7: "cancel every cancellable job (confirm)").
#[must_use]
pub fn cancel_all_confirm(count: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(theme.space.md)
        .px(theme.space.lg)
        .h(theme.metrics.banner_h)
        .bg(Tone::Warning.fill(theme))
        .child(Text::ui(format!("Cancel {count} cancellable jobs?")).tone(Tone::Warning))
        .child(KeyHintRow::new().key("X", "confirm").key("esc", "keep"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, status: JobStatus) -> JobRecord {
        JobRecord {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            kind: JobKind::Clone,
            target: "nixos".to_owned(),
            title: "Clone nixos".to_owned(),
            status,
            progress: Some("Receiving objects: 40% (81/202)".to_owned()),
            log_path: "/home/u/.fleet/logs/jobs/j.log".to_owned(),
            started_at: "2026-09-04T12:00:00Z".to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: true,
        }
    }

    #[test]
    fn the_filter_cycles_all_running_failed() {
        assert_eq!(JobFilter::All.next(), JobFilter::Running);
        assert_eq!(JobFilter::Running.next(), JobFilter::Failed);
        assert_eq!(JobFilter::Failed.next(), JobFilter::All);
        assert_eq!(JobFilter::All.label(), None, "the default is unlabelled");
    }

    #[test]
    fn the_filter_selects_the_right_rows() {
        let jobs = vec![
            job("job-a", JobStatus::Running),
            job(
                "job-b",
                JobStatus::Failed {
                    error: "gh: HTTP 502".to_owned(),
                },
            ),
            job("job-c", JobStatus::Succeeded),
            job("job-d", JobStatus::Queued),
        ];
        let none = HashSet::new();
        assert_eq!(visible_jobs(&jobs, JobFilter::All, &none).len(), 4);
        assert_eq!(visible_jobs(&jobs, JobFilter::Running, &none).len(), 2);
        assert_eq!(visible_jobs(&jobs, JobFilter::Failed, &none).len(), 1);
    }

    #[test]
    fn dismissed_jobs_leave_the_list() {
        let jobs = vec![job("job-a", JobStatus::Succeeded)];
        let mut dismissed = HashSet::new();
        dismissed.insert(jobs[0].id.clone());
        assert!(visible_jobs(&jobs, JobFilter::All, &dismissed).is_empty());
    }

    #[test]
    fn only_finished_jobs_can_be_dismissed() {
        assert!(!is_dismissable(&JobStatus::Running));
        assert!(!is_dismissable(&JobStatus::Queued));
        assert!(!is_dismissable(&JobStatus::Cancelling));
        assert!(is_dismissable(&JobStatus::Succeeded));
        assert!(is_dismissable(&JobStatus::Cancelled));
        assert!(is_dismissable(&JobStatus::Failed {
            error: String::new()
        }));
    }

    #[test]
    fn the_header_counts_three_families() {
        let jobs = vec![
            job("job-a", JobStatus::Running),
            job("job-b", JobStatus::Queued),
            job(
                "job-c",
                JobStatus::Failed {
                    error: "boom".to_owned(),
                },
            ),
            job("job-d", JobStatus::Succeeded),
            job("job-e", JobStatus::Cancelled),
        ];
        let counts = job_counts(&jobs);
        assert_eq!(counts.running, 2);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.done, 2);
        assert!(!counts.is_empty());
        assert!(job_counts(&[]).is_empty());
    }

    #[test]
    fn timestamps_parse_to_epoch_seconds() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2026-09-04T12:00:00Z"), Some(1_788_523_200));
        assert_eq!(
            parse_timestamp("2026-09-04T14:00:00+02:00"),
            parse_timestamp("2026-09-04T12:00:00Z"),
            "an offset must be normalised, not ignored"
        );
        assert_eq!(
            parse_timestamp("2026-09-04T12:00:00.123456789Z"),
            Some(1_788_523_200)
        );
        assert_eq!(parse_timestamp("not a timestamp"), None);
        assert_eq!(parse_timestamp(""), None);
    }

    #[test]
    fn a_live_job_shows_no_elapsed_for_the_first_thirty_seconds() {
        let job = job("job-a", JobStatus::Running);
        let started = parse_timestamp(&job.started_at).unwrap_or_default();
        assert_eq!(elapsed_label(&job, started + 29), None);
        assert_eq!(elapsed_label(&job, started + 42), Some("0:42".to_owned()));
        assert_eq!(elapsed_label(&job, started + 605), Some("10:05".to_owned()));
    }

    #[test]
    fn a_finished_job_shows_a_single_unit_age_and_a_cancelled_one_a_dash() {
        let mut done = job("job-a", JobStatus::Succeeded);
        done.finished_at = Some("2026-09-04T12:00:12Z".to_owned());
        assert_eq!(elapsed_label(&done, now_unix()), Some("12s".to_owned()));

        let mut cancelled = job("job-b", JobStatus::Cancelled);
        cancelled.finished_at = Some("2026-09-04T12:00:12Z".to_owned());
        assert_eq!(
            elapsed_label(&cancelled, now_unix()),
            Some("\u{2013}".to_owned()),
            "a cancelled job never claims a duration"
        );
    }

    #[test]
    fn only_live_jobs_carry_a_progress_line_and_a_percent() {
        let running = job("job-a", JobStatus::Running);
        assert_eq!(sub_line(&running), Some("Receiving objects: 40% (81/202)"));
        assert_eq!(percent(&running), Some(40));

        let done = job("job-b", JobStatus::Succeeded);
        assert_eq!(
            sub_line(&done),
            None,
            "a finished job collapses to one line"
        );
        assert_eq!(percent(&done), None);

        let failed = job(
            "job-c",
            JobStatus::Failed {
                error: "gh: HTTP 502 upstream connect error".to_owned(),
            },
        );
        assert_eq!(
            sub_line(&failed),
            Some("gh: HTTP 502 upstream connect error"),
            "a failure states its reason, which is what decides R"
        );
        assert_eq!(percent(&failed), None);
    }

    #[test]
    fn the_log_path_is_printed_under_a_tilde() {
        let home = Path::new("/home/u");
        assert_eq!(
            tilde("/home/u/.fleet/logs/jobs/j-8f3c.log", Some(home)),
            "~/.fleet/logs/jobs/j-8f3c.log"
        );
        assert_eq!(tilde("/var/tmp/j.log", Some(home)), "/var/tmp/j.log");
        assert_eq!(tilde("/home/u", Some(home)), "~");
        assert_eq!(tilde("/home/u/.fleet", None), "/home/u/.fleet");
        assert_eq!(
            tilde("/home/user2/.fleet", Some(home)),
            "/home/user2/.fleet",
            "a sibling home must not be abbreviated"
        );
    }

    #[test]
    fn every_status_maps_to_its_documented_glyph() {
        assert_eq!(row_status(&JobStatus::Queued), RowStatus::Queued);
        assert_eq!(row_status(&JobStatus::Running), RowStatus::Running);
        assert_eq!(row_status(&JobStatus::Cancelling), RowStatus::Cancelling);
        assert_eq!(row_status(&JobStatus::Cancelled), RowStatus::Cancelled);
        assert_eq!(row_status(&JobStatus::Succeeded), RowStatus::Done);
        assert_eq!(
            row_status(&JobStatus::Failed {
                error: String::new()
            }),
            RowStatus::Failed
        );
    }

    #[test]
    fn the_empty_state_states_the_promise_verbatim() {
        assert_eq!(EMPTY_FACT, "Nothing running.");
        assert!(EMPTY_ACTION.contains("survive closing this window"));
    }
}
