//! Job filters, counters, and row composition.

use std::collections::HashSet;

use fleet_core::ids::JobId;
use fleet_proto::job::{JobRecord, JobStatus};
use fleet_ui_kit::{
    ActiveTheme, EmptyState, Icon, IconSize, JobRow, KeyHint, KeyHintRow, PaneHeader, Text, Tone,
    format_age,
};
use gpui::{AnyElement, App, SharedString, div, prelude::*};

use crate::presentation::{JobDisplay, is_active};

/// The fact line of the empty Jobs panel (§3.13), verbatim.
const EMPTY_FACT: &str = "Nothing running.";
/// The second line of the empty Jobs panel (§3.13), verbatim. It is the product promise, stated
/// once, in the one place that is demonstrating it.
const EMPTY_ACTION: &str = "Jobs and sessions live in fleetd, so they survive closing this window.";
/// The amber strip the panel grows while the daemon is unreachable (§3.7 "States").
const DAEMON_DOWN_PREFIX: &str = "The daemon is unreachable — job state is from";

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

/// The header's three labelled counters, in §3.7 order and zero-suppressed (§1.2).
///
/// The label is the point: `11` alone answers no question the panel was opened to answer, and
/// a glyph plus a bare number makes the reader decode the glyph first. Returned as data so the
/// wording is testable without a window.
#[must_use]
fn count_labels(counts: JobCounts) -> Vec<(Icon, String, Tone)> {
    [
        (Icon::LoaderCircle, counts.running, "running", Tone::Warning),
        (Icon::CircleX, counts.failed, "failed", Tone::Danger),
        (Icon::CircleCheck, counts.done, "done", Tone::Success),
    ]
    .into_iter()
    .filter(|(_, count, _, _)| *count > 0)
    .map(|(icon, count, word, tone)| (icon, format!("{count} {word}"), tone))
    .collect()
}

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
    let labels = count_labels(counts);
    let last = labels.len().saturating_sub(1);
    let counters = div().flex().items_center().gap(theme.space.xs).children(
        labels
            .into_iter()
            .enumerate()
            .map(|(index, (icon, label, tone))| {
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.xs)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space.xxs)
                            .child(icon.el().size(IconSize::Small).color(tone.color(theme)))
                            .child(Text::ui(label).tone(tone)),
                    )
                    .when(index != last, |el| el.child(Text::ui("\u{00b7}").faint()))
            }),
    );

    // §3.7's header is the three labelled counts and nothing else: an unlabelled total repeats
    // what they already add up to. `shown/total` returns only while `f` is filtering, where it
    // answers "how many did the filter hide".
    let mut header = PaneHeader::new("jobs").trailing(counters);
    if let Some(label) = filter.label() {
        header = header.scope(label).total(total).shown(shown);
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

/// Only the elapsed label changes between job updates.
pub(crate) fn prepared_job_row(job: &JobDisplay, cursor: bool, now: i64) -> AnyElement {
    let mut row = JobRow::new(job.status, job.kind.clone(), job.target.clone())
        .id(job.element_id.clone())
        .selected(cursor)
        .cursor(cursor);
    if let Some(elapsed) = job.elapsed_label(now) {
        row = row.elapsed(elapsed);
    }
    if let Some(percent) = job.percent {
        row = row.percent(percent);
    }
    if let Some(line) = &job.progress {
        row = row.progress(line.clone());
    }
    if job.retryable {
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
        .border_t(theme.metrics.hairline)
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
    use crate::presentation::{is_dismissable, sub_line};
    use fleet_proto::job::JobKind;

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
    fn the_header_counters_are_labelled_and_zero_suppressed() {
        let counts = JobCounts {
            running: 2,
            failed: 1,
            done: 5,
        };
        let labels: Vec<String> = count_labels(counts)
            .into_iter()
            .map(|(_, label, _)| label)
            .collect();
        assert_eq!(
            labels,
            vec!["2 running", "1 failed", "5 done"],
            "§3.7's header is `⟳n running · ✕n failed · ✓n done`, never bare numbers"
        );
        assert_eq!(
            count_labels(JobCounts {
                running: 0,
                failed: 0,
                done: 3,
            })
            .into_iter()
            .map(|(_, label, _)| label)
            .collect::<Vec<_>>(),
            vec!["3 done"],
            "§1.2: a zero counter is not drawn"
        );
        assert!(count_labels(JobCounts::default()).is_empty());
    }

    #[test]
    fn only_live_jobs_carry_a_progress_line_and_a_percent() {
        let running = job("job-a", JobStatus::Running);
        assert_eq!(sub_line(&running), Some("Receiving objects: 40% (81/202)"));
        assert_eq!(JobDisplay::new(&running).percent, Some(40));

        let done = job("job-b", JobStatus::Succeeded);
        assert_eq!(
            sub_line(&done),
            None,
            "a finished job collapses to one line"
        );
        assert_eq!(JobDisplay::new(&done).percent, None);

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
        assert_eq!(JobDisplay::new(&failed).percent, None);
    }
}
