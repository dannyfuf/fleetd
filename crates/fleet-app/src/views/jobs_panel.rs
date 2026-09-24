//! The Jobs sheet's pieces (UX-SPEC §3.7): its header with the filter, the job rows with the
//! buttons that act on them, the log's toolbar and the one-line footer.
//!
//! Every button dispatches the action its key runs, and shows that key from the live keymap;
//! every label comes from the action catalogue. A button on a row first moves the cursor to that
//! row, so the action it dispatches acts on the job the pointer is on.

use std::rc::Rc;

use fleet_proto::job::{JobRecord, JobStatus};
use fleet_ui_kit::{
    ActiveTheme, Button, ButtonSize, ButtonStyle, ContextMenu, EmptyState, Icon, IconButton,
    IconSize, JobRow, ListPointer, MenuAnchor, MenuItem, PopoverMenu, Segment, SegmentedControl,
    Text, Tone, format_age, prelude::*,
};
use gpui::{Action, AnyElement, App, SharedString, Window, div};

use crate::{
    action_catalogue, actions::jobs as jobs_actions, presentation::JobDisplay, views::harness,
};

pub use crate::state::JobFilter;

/// The panel's title.
const TITLE: &str = "Jobs";
/// The quiet group finished jobs fold into.
const FINISHED: &str = "Finished";
/// The footer: the product promise, stated once, in the one place that is demonstrating it.
const FOOTER: &str = "Jobs run in fleetd and survive closing this window.";
/// The fact line of the empty Jobs panel (§3.13), verbatim.
const EMPTY_FACT: &str = "Nothing running.";
/// The state word of the log's follow toggle. It names what the log is doing, not a verb, so
/// it is not the action's catalogue label.
const FOLLOWING: &str = "Following";
/// The amber strip the panel grows while the daemon is unreachable (§3.7 "States").
const DAEMON_DOWN_PREFIX: &str = "The daemon is unreachable — job state is from";
/// The confirm strip's "no" answer.
const KEEP: &str = "Keep";

/// A handler that moves the job cursor to row `ix`.
pub type SelectRow = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// A handler that picks a filter.
pub type PickFilter = Rc<dyn Fn(JobFilter, &mut Window, &mut App)>;

/// The catalogue's short label for an action: what a button or a menu item reads.
fn label(action: &dyn Action) -> &'static str {
    action_catalogue::info(action.name()).map_or("", |info| info.short_label)
}

/// The header's four filter counts and the summary's two.
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
    /// Whether the panel lists no job at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.running == 0 && self.failed == 0 && self.done == 0
    }

    /// Every job.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.running + self.failed + self.done
    }

    /// How many jobs `filter` shows: the count on its segment.
    #[must_use]
    pub const fn of(&self, filter: JobFilter) -> usize {
        match filter {
            JobFilter::All => self.total(),
            JobFilter::Running => self.running,
            JobFilter::Failed => self.failed,
            JobFilter::Done => self.done,
        }
    }
}

/// Counts the jobs of each family.
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

/// The summary beside the title, zero-suppressed (§1.2): `1 running · 1 failed`. What is done
/// is on the Done segment; the summary says what still needs an eye.
#[must_use]
fn summary_parts(counts: JobCounts) -> Vec<(String, Tone)> {
    [
        (counts.running, "running", Tone::Secondary),
        (counts.failed, "failed", Tone::Danger),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, word, tone)| (format!("{count} {word}"), tone))
    .collect()
}

/// What the list header needs.
pub struct HeaderProps {
    /// The four families' counts.
    pub counts: JobCounts,
    /// The filter the list is showing.
    pub filter: JobFilter,
    /// How many jobs `X` would cancel; the ⋯ menu is hidden at zero.
    pub cancellable: usize,
    /// Picks a filter from its segment.
    pub on_filter: PickFilter,
}

/// The list header: `Jobs · 1 running · 1 failed`, Clear finished, the ⋯ menu, and the filter.
#[must_use]
pub fn header(props: HeaderProps, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let parts = summary_parts(props.counts);
    let last = parts.len().saturating_sub(1);
    let summary = div()
        .flex()
        .items_center()
        .gap(theme.space.xs)
        .min_w_0()
        .children(parts.into_iter().enumerate().map(|(index, (text, tone))| {
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(Text::ui(text).tone(tone))
                .when(index != last, |el| el.child(Text::ui("\u{00b7}").faint()))
        }));
    let clear = (props.counts.done + props.counts.failed > 0).then(|| {
        Button::new("jobs-clear-finished", label(&jobs_actions::DismissFinished))
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .action(Box::new(jobs_actions::DismissFinished))
            .harness_target("jobs.clear")
    });
    let more = (props.cancellable > 0).then(|| {
        PopoverMenu::new("jobs-more")
            .anchor(MenuAnchor::BottomRight)
            .trigger_with(|open, _, _| {
                IconButton::new("jobs-more-trigger", Icon::Ellipsis, "More job actions")
                    .size(ButtonSize::Compact)
                    .selected(open)
            })
            .menu(|menu, _, _| {
                menu.item(
                    MenuItem::new(label(&jobs_actions::CancelAll))
                        .action(Box::new(jobs_actions::CancelAll))
                        .destructive(true),
                )
            })
            .harness_target("jobs.more")
    });
    let on_filter = props.on_filter;
    let counts = props.counts;
    // A segment says `0` rather than vanish: an empty filter must still say it is empty.
    let filter = div().flex().child(
        SegmentedControl::new(
            "jobs-filter",
            JobFilter::ALL
                .map(|filter| Segment::new(filter.title()).count(Some(counts.of(filter)))),
        )
        .active(Some(props.filter.index()))
        .harness_segments("jobs.filter")
        .on_select(move |index, window, cx| {
            if let Some(filter) = JobFilter::ALL.get(index) {
                on_filter(*filter, window, cx);
            }
        }),
    );

    div()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        .px(theme.space.lg)
        .pt(theme.space.md)
        .pb(theme.space.sm)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(theme.space.md)
                        .child(Text::section_title(TITLE).flex_none())
                        .child(summary),
                )
                .children(clear)
                .children(more),
        )
        .child(filter)
        .into_any_element()
}

/// The job whose log is open, as the log's toolbar names it.
#[derive(Debug, Clone, Default)]
pub struct LogTitle {
    /// The sentence's verb phrase.
    pub lead: SharedString,
    /// The domain id, in mono.
    pub subject: Option<SharedString>,
    /// `~/.fleet/logs/jobs/<id>.log`, verbatim: what makes a failure survivable outside the app.
    pub path: SharedString,
}

/// The log's toolbar: `← Back`, the job, `Following f`, `Jump to end G`, and the log's path with
/// the button that copies it.
#[must_use]
pub fn log_header(title: &LogTitle, following: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(theme.space.xs)
        .px(theme.space.md)
        .pt(theme.space.sm)
        .pb(theme.space.sm)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    Button::new("jobs-log-back", label(&jobs_actions::CollapseLog))
                        .icon(Icon::ChevronLeft)
                        .style(ButtonStyle::Ghost)
                        .size(ButtonSize::Compact)
                        .action(Box::new(jobs_actions::CollapseLog))
                        .harness_target("jobs.log.back"),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(theme.space.xs)
                        .overflow_hidden()
                        .child(Text::ui_strong(title.lead.clone()).flex_none())
                        .children(
                            title
                                .subject
                                .clone()
                                .map(|subject| Text::data(subject).ellipsize()),
                        ),
                )
                .child(
                    Button::new("jobs-log-follow", FOLLOWING)
                        .style(ButtonStyle::Ghost)
                        .size(ButtonSize::Compact)
                        .selected(following)
                        .action(Box::new(jobs_actions::CycleFilter))
                        .harness_target("jobs.log.follow"),
                )
                .child(
                    Button::new("jobs-log-end", label(&jobs_actions::Bottom))
                        .style(ButtonStyle::Ghost)
                        .size(ButtonSize::Compact)
                        .action(Box::new(jobs_actions::Bottom))
                        .harness_target("jobs.log.end"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .pl(theme.space.sm)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Text::data_small(title.path.clone()).faint().ellipsize()),
                )
                .child(
                    Button::new("jobs-log-copy", label(&jobs_actions::CopyLogPath))
                        .style(ButtonStyle::Ghost)
                        .size(ButtonSize::Compact)
                        .action(Box::new(jobs_actions::CopyLogPath)),
                ),
        )
        .into_any_element()
}

/// A button on row `ix` that first puts the cursor on that row, then runs `action` on it.
fn row_button(
    id: (&'static str, usize),
    action: Box<dyn Action>,
    style: ButtonStyle,
    select: &SelectRow,
) -> Button {
    let select = select.clone();
    let ix = id.1;
    Button::new(id, label(action.as_ref()))
        .style(style)
        .size(ButtonSize::Compact)
        .on_click(move |_, window, cx| select(ix, window, cx))
        .action(action)
}

/// The quiet label the finished jobs are grouped under.
fn finished_label(cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .px(theme.space.md)
        .pt(theme.space.md)
        .pb(theme.space.xs)
        .child(Text::sentence_label(FINISHED))
        .into_any_element()
}

/// Everything one row of the list needs, besides the job.
pub struct RowContext<'a> {
    /// The row's index in the visible list: `jobs.row[N]`.
    pub ix: usize,
    /// Whether the cursor is on it.
    pub cursor: bool,
    /// Whether it opens the "Finished" group.
    pub first_finished: bool,
    /// The wall clock, for the elapsed label.
    pub now: i64,
    /// The list's click / double-click / right-click contract.
    pub pointer: &'a ListPointer,
    /// Moves the cursor, for the row's buttons.
    pub select: &'a SelectRow,
}

/// One job row: the sentence, its state's details and buttons, and its right-click menu.
#[must_use]
pub fn job_row(job: &JobDisplay, row: &RowContext<'_>, cx: &App) -> AnyElement {
    let ix = row.ix;
    let theme = cx.theme();
    let mut item = JobRow::new(job.element_id.clone(), job.status, job.lead.clone())
        .selected(row.cursor)
        .cursor(row.cursor)
        .pointer(row.pointer, ix);
    if let Some(subject) = &job.subject {
        item = item.subject(subject.clone());
    }
    if let Some(elapsed) = job.elapsed_label(row.now) {
        item = item.elapsed(elapsed);
    }
    if let Some(percent) = job.percent {
        item = item.percent(percent);
    }
    if let Some(line) = &job.progress {
        item = item.progress(line.clone());
    }
    if let Some(error) = &job.error {
        item = item.error(error.clone());
    }
    if let Some(detail) = &job.error_detail {
        item = item.error_detail(detail.clone());
    }
    if job.cancellable {
        item = item.hover_action(
            row_button(
                ("job-cancel", ix),
                Box::new(jobs_actions::CancelJob),
                ButtonStyle::Secondary,
                row.select,
            )
            .harness_target(harness::name(|| format!("jobs.row[{ix}].cancel"))),
        );
    }
    if job.error.is_some() {
        item = item.actions(
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .children(job.retryable.then(|| {
                    row_button(
                        ("job-retry", ix),
                        Box::new(jobs_actions::Retry),
                        ButtonStyle::Primary,
                        row.select,
                    )
                    .harness_target(harness::name(|| format!("jobs.row[{ix}].retry")))
                }))
                .child(
                    row_button(
                        ("job-log", ix),
                        Box::new(jobs_actions::ToggleLog),
                        ButtonStyle::Secondary,
                        row.select,
                    )
                    .harness_target(harness::name(|| format!("jobs.row[{ix}].log"))),
                )
                .child(row_button(
                    ("job-copy", ix),
                    Box::new(jobs_actions::CopyLogPath),
                    ButtonStyle::Ghost,
                    row.select,
                )),
        );
    }

    let (retryable, cancellable) = (job.retryable, job.cancellable);
    let menu = ContextMenu::new(
        ("job-menu", ix),
        item.harness_target_indexed("jobs.row", ix),
    )
    .menu(move |menu, _, _| {
        // The row's verbs, each only when it can work on this job: a menu leaves out what it
        // cannot do rather than greying it.
        let entry = |action: Box<dyn Action>| MenuItem::new(label(action.as_ref())).action(action);
        let mut menu = menu.item(entry(Box::new(jobs_actions::ToggleLog)));
        if retryable {
            menu = menu.item(entry(Box::new(jobs_actions::Retry)));
        }
        if cancellable {
            menu = menu.item(entry(Box::new(jobs_actions::CancelJob)));
        }
        menu.item(entry(Box::new(jobs_actions::CopyLogPath)))
    });
    div()
        .flex()
        .flex_col()
        .w_full()
        .when(row.first_finished, |el| el.child(finished_label(cx)))
        .child(menu)
        .into_any_element()
}

/// The footer: one muted line, the promise the panel demonstrates.
#[must_use]
pub fn footer(cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .px(theme.space.lg)
        .py(theme.space.sm)
        .child(Text::ui(FOOTER).muted())
        .into_any_element()
}

/// The empty state of §3.13, rendered in the panel and nowhere else. A filtered list names what
/// it is missing; the segmented control right above it is the way back.
#[must_use]
pub fn empty_state(filter: JobFilter) -> AnyElement {
    match filter.label() {
        Some(label) => EmptyState::new(format!("No {label} jobs.")).into_any_element(),
        None => EmptyState::new(EMPTY_FACT).into_any_element(),
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

/// The in-panel confirmation of `X` (§3.7: "cancel every cancellable job (confirm)"): the strong
/// answer is the same action again, the other is the one `Esc` runs.
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
        .py(theme.space.xs)
        .bg(Tone::Warning.fill(theme))
        .child(Text::ui(format!("Cancel {count} cancellable jobs?")).tone(Tone::Warning))
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(
                    Button::new("jobs-cancel-all-keep", KEEP)
                        .style(ButtonStyle::Ghost)
                        .size(ButtonSize::Compact)
                        .action(Box::new(jobs_actions::Close)),
                )
                .child(
                    Button::new("jobs-cancel-all-confirm", label(&jobs_actions::CancelAll))
                        .style(ButtonStyle::Danger)
                        .size(ButtonSize::Compact)
                        .action(Box::new(jobs_actions::CancelAll)),
                ),
        )
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
    fn the_filter_cycles_all_running_failed_done() {
        assert_eq!(JobFilter::All.next(), JobFilter::Running);
        assert_eq!(JobFilter::Running.next(), JobFilter::Failed);
        assert_eq!(JobFilter::Failed.next(), JobFilter::Done);
        assert_eq!(JobFilter::Done.next(), JobFilter::All);
        assert_eq!(JobFilter::All.label(), None, "the default is unlabelled");
        for (index, filter) in JobFilter::ALL.iter().enumerate() {
            assert_eq!(filter.index(), index, "a segment is its filter's index");
        }
    }

    #[test]
    fn the_filter_selects_the_right_rows_live_and_failed_before_finished() {
        let jobs = vec![
            job("job-a", JobStatus::Succeeded),
            job("job-b", JobStatus::Running),
            job(
                "job-c",
                JobStatus::Failed {
                    error: "gh: HTTP 502".to_owned(),
                },
            ),
            job("job-d", JobStatus::Cancelled),
            job("job-e", JobStatus::Queued),
        ];
        let ids = |filter: JobFilter| {
            filter
                .visible(&jobs)
                .map(|job| job.id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(JobFilter::All),
            vec!["job-b", "job-c", "job-e", "job-a", "job-d"],
            "the finished group folds under everything still needing an eye"
        );
        assert_eq!(ids(JobFilter::Running), vec!["job-b", "job-e"]);
        assert_eq!(ids(JobFilter::Failed), vec!["job-c"]);
        assert_eq!(ids(JobFilter::Done), vec!["job-a", "job-d"]);
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
    fn the_header_counts_three_families_and_each_segment() {
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
        assert_eq!(
            JobFilter::ALL.map(|filter| counts.of(filter)),
            [5, 2, 1, 2],
            "All / Running / Failed / Done"
        );
        assert!(!counts.is_empty());
        assert!(job_counts(&[]).is_empty());
    }

    #[test]
    fn the_summary_says_what_needs_an_eye_and_suppresses_zeros() {
        let text = |counts| {
            summary_parts(counts)
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            text(JobCounts {
                running: 1,
                failed: 1,
                done: 5,
            }),
            vec!["1 running", "1 failed"]
        );
        assert_eq!(
            text(JobCounts {
                running: 0,
                failed: 0,
                done: 3,
            }),
            Vec::<String>::new(),
            "§1.2: a zero counter is not drawn, and done lives on its segment"
        );
    }

    #[test]
    fn every_button_label_comes_from_the_catalogue() {
        for action in [
            &jobs_actions::CancelJob as &dyn Action,
            &jobs_actions::Retry,
            &jobs_actions::ToggleLog,
            &jobs_actions::CopyLogPath,
            &jobs_actions::DismissFinished,
            &jobs_actions::CancelAll,
            &jobs_actions::CollapseLog,
            &jobs_actions::Bottom,
        ] {
            assert!(!label(action).is_empty(), "{} has no label", action.name());
        }
        assert_eq!(label(&jobs_actions::ToggleLog), "Show log");
        assert_eq!(label(&jobs_actions::DismissFinished), "Clear finished");
        assert_eq!(label(&jobs_actions::CollapseLog), "Back");
        assert_eq!(label(&jobs_actions::Bottom), "Jump to end");
    }

    #[test]
    fn only_live_jobs_carry_a_progress_line_and_a_percent() {
        let running = job("job-a", JobStatus::Running);
        assert_eq!(sub_line(&running), Some("Receiving objects: 40% (81/202)"));
        let display = JobDisplay::new(&running);
        assert_eq!(display.percent, Some(40));
        assert!(display.cancellable);
        assert!(display.error.is_none());

        let done = JobDisplay::new(&job("job-b", JobStatus::Succeeded));
        assert_eq!(done.progress, None, "a finished job collapses to one line");
        assert_eq!(done.percent, None);
        assert!(done.is_finished());

        let failed = JobDisplay::new(&job(
            "job-c",
            JobStatus::Failed {
                error: "gh: HTTP 502 upstream connect error".to_owned(),
            },
        ));
        assert_eq!(
            failed.error.as_deref(),
            Some("gh: HTTP 502 upstream connect error"),
            "a failure states its reason inline, which is what decides R"
        );
        assert_eq!(
            failed.error_detail.as_deref(),
            Some("Receiving objects: 40% (81/202)"),
            "under it, the last thing the job printed"
        );
        assert_eq!(failed.progress, None);
        assert_eq!(failed.percent, None);
        assert!(failed.retryable && !failed.cancellable);
    }
}
