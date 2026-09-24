//! `JobRow` — one background job, written as a sentence, with what it needs to be acted on.
//!
//! §3.7. The row's line is `glyph · sentence · elapsed`: "Clone `acme/infra`", "Create
//! `acme/api#injected-0`" — a verb in the interface face and the real domain id in mono, so the
//! target still matches the row it is about. Below the line, only what the job's state earns:
//!
//! * **in flight** — a progress bar with its percent (when the output states one) and the last
//!   stdout line, the only way to see a stuck clone;
//! * **failed** — the error, inline, in the danger wash, and the buttons that act on it
//!   (normally Retry, Show log, Copy log path);
//! * **finished** — nothing: one quiet line with its duration and age.
//!
//! A control that belongs to the pointer only while the row is under it (a running job's Cancel)
//! goes in [`JobRow::hover_action`], which is also drawn on the selected row, so the key it shows
//! is always visible where the key would act. Nothing here decays on a timer — [D-9] makes
//! retention the caller's decision, and a failed job is never auto-dismissed.
//!
//! Use a plain [`Row`] for anything that is not a job; use [`super::JobTicker`] for the one-line
//! status-bar summary of the newest running job.

use gpui::{AnyElement, App, ElementId, SharedString, Window, div, prelude::*, relative};

use crate::{
    components::{ColumnAlign, ListPointer, Row, RowColumn, StatusGlyph, StatusKind},
    icons::Icon,
    text::Text,
    theme::{ActiveTheme, ch},
    tone::Tone,
};

/// `(not restartable)`, the longest label the §3.8.9 confirm shows.
const RETRYABLE_CH: f32 = 18.0;

/// The six job states of §3.7.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    /// Waiting for a slot. `clock`.
    Queued,
    /// Running. `loader-circle`, spinning.
    Running,
    /// A cancel was requested. `circle-stop`.
    Cancelling,
    /// Cancelled. `circle-slash`.
    Cancelled,
    /// Finished successfully. `circle-check`.
    Done,
    /// Failed. `circle-x`. Never auto-dismissed ([D-9]).
    Failed,
}

impl JobStatus {
    /// The glyph.
    pub fn icon(self) -> Icon {
        match self {
            JobStatus::Queued => Icon::Clock,
            JobStatus::Running => Icon::LoaderCircle,
            JobStatus::Cancelling => Icon::CircleStop,
            JobStatus::Cancelled => Icon::CircleSlash,
            JobStatus::Done => Icon::CircleCheck,
            JobStatus::Failed => Icon::CircleX,
        }
    }

    /// The tone.
    pub fn tone(self) -> Tone {
        match self {
            JobStatus::Queued | JobStatus::Cancelled => Tone::Muted,
            JobStatus::Running | JobStatus::Cancelling => Tone::Warning,
            JobStatus::Done => Tone::Success,
            JobStatus::Failed => Tone::Danger,
        }
    }

    /// Whether the glyph spins. Only `Running` does; `Cancelling` has *stopped* making
    /// progress and says so with a static `circle-stop`.
    fn spins(self) -> bool {
        matches!(self, JobStatus::Running)
    }

    /// Whether the job is over and done with: it steps its sentence down a level so the eye
    /// lands on the jobs that are not. A failure is over too, but it is not done with.
    pub fn is_finished(self) -> bool {
        matches!(self, JobStatus::Cancelled | JobStatus::Done)
    }
}

/// One job item.
#[derive(IntoElement)]
pub struct JobRow {
    id: ElementId,
    status: JobStatus,
    lead: SharedString,
    subject: Option<SharedString>,
    elapsed: Option<SharedString>,
    percent: Option<u8>,
    progress: Option<SharedString>,
    error: Option<SharedString>,
    error_detail: Option<SharedString>,
    actions: Option<AnyElement>,
    hover_action: Option<AnyElement>,
    retryable: Option<bool>,
    selected: bool,
    cursor: bool,
    pointer: Option<(ListPointer, usize)>,
}

impl JobRow {
    /// A job row whose sentence starts with `lead` ("Clone", "Run hooks for", "Inspect
    /// worktrees"). Add the domain id it acts on with [`Self::subject`].
    pub fn new(id: impl Into<ElementId>, status: JobStatus, lead: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            status,
            lead: lead.into(),
            subject: None,
            elapsed: None,
            percent: None,
            progress: None,
            error: None,
            error_detail: None,
            actions: None,
            hover_action: None,
            retryable: None,
            selected: false,
            cursor: false,
            pointer: None,
        }
    }

    /// The object of the sentence, in mono: the `RepoId` or `WorktreeId` the job acts on, so it
    /// matches the row it is about.
    pub fn subject(mut self, subject: impl Into<SharedString>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// The trailing time: `m:ss` while running (the caller shows it after 30 s), `2s · 1m ago`
    /// once finished.
    pub fn elapsed(mut self, elapsed: impl Into<SharedString>) -> Self {
        self.elapsed = Some(elapsed.into());
        self
    }

    /// The percent, when the job's output states one. Draws the progress bar; a job with no
    /// parseable percent gets no bar (§3.7 omits progress bars for non-percent jobs).
    pub fn percent(mut self, percent: u8) -> Self {
        self.percent = Some(percent);
        self
    }

    /// The last stdout line: the single most reassuring artifact for a long job, and the only
    /// way to see a stuck clone.
    pub fn progress(mut self, progress: impl Into<SharedString>) -> Self {
        self.progress = Some(progress.into());
        self
    }

    /// The failure, shown inline in the danger wash so it is read without opening the log.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// A second, quieter line under [`Self::error`]: the last thing the job printed.
    pub fn error_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.error_detail = Some(detail.into());
        self
    }

    /// Buttons under the row that are always shown: a failed job's Retry, Show log and Copy
    /// log path. Every one must also be reachable by its key and from the row's menu.
    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }

    /// One trailing control drawn while the row is hovered or selected: a running job's
    /// Cancel. Its width stays reserved while hidden, so revealing it never reflows the line.
    pub fn hover_action(mut self, action: impl IntoElement) -> Self {
        self.hover_action = Some(action.into_any_element());
        self
    }

    /// Whether a retry (`R`) would work on this job. Renders the §3.8.9 `(restartable)` /
    /// `(not restartable)` label, which is the difference between "this will come back" and
    /// "you will have to start it again by hand" in the quit-and-stop confirm. Unset renders
    /// nothing: a job whose retryability is unknown must not claim either.
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    /// Paint the selection background.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Draw the cursor bar.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// Wire the row, as item `ix` of its list, to the list's click / double-click /
    /// right-click contract (UX-SPEC §5.1).
    pub fn pointer(mut self, pointer: &ListPointer, ix: usize) -> Self {
        self.pointer = Some((pointer.clone(), ix));
        self
    }

    /// The id the spinning glyph animates under.
    ///
    /// It has to be a *child* of the row id: reusing the row's own id would put two elements
    /// with the same [`ElementId`] in one frame, and the animation would then share state with
    /// the row's hover.
    fn glyph_id(&self) -> ElementId {
        ElementId::NamedChild(
            std::sync::Arc::new(self.id.clone()),
            SharedString::new_static("glyph"),
        )
    }
}

impl RenderOnce for JobRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let status = self.status;
        let tone = status.tone();

        let leading = if status.spins() {
            StatusGlyph::new(StatusKind::JobRunning)
                .id(self.glyph_id())
                .into_any_element()
        } else {
            status
                .icon()
                .el()
                .color(tone.color(theme))
                .into_any_element()
        };

        let lead_tone = if status.is_finished() {
            Tone::Secondary
        } else {
            Tone::Default
        };
        let has_subject = self.subject.is_some();
        let sentence = div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .min_w_0()
            .overflow_hidden()
            .child(if has_subject {
                Text::ui(self.lead).tone(lead_tone).flex_none()
            } else {
                Text::ui(self.lead).tone(lead_tone).ellipsize()
            })
            .children(
                self.subject
                    .map(|subject| Text::data(subject).tone(lead_tone).ellipsize()),
            );

        let mut row = Row::new()
            .id(self.id)
            .selected(self.selected)
            .cursor(self.cursor)
            .leading(leading)
            .column(RowColumn::flex(sentence));
        if let Some(elapsed) = self.elapsed {
            row = row.column(
                RowColumn::auto(Text::data_small(elapsed).faint()).align(ColumnAlign::Right),
            );
        }
        if let Some(retryable) = self.retryable {
            let (label, tone) = if retryable {
                ("(restartable)", Tone::Muted)
            } else {
                ("(not restartable)", Tone::Warning)
            };
            row = row.column(
                RowColumn::fixed(ch(RETRYABLE_CH), Text::data_small(label).tone(tone))
                    .align(ColumnAlign::Right),
            );
        }
        if let Some(action) = self.hover_action {
            row = row.hover_actions(action);
        }

        let bar = self.percent.map(|percent| {
            let percent = percent.min(100);
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    div()
                        .flex_1()
                        .h(theme.metrics.progress_bar_h)
                        .rounded(theme.radii.full)
                        .bg(theme.colors.control)
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(f32::from(percent) / 100.0))
                                .bg(Tone::Info.color(theme)),
                        ),
                )
                .child(Text::data_small(format!("{percent}%")).muted().flex_none())
        });
        let progress = self
            .progress
            .map(|line| Text::data_small(line).muted().ellipsize());
        let error = self.error.map(|error| {
            div()
                .flex()
                .flex_col()
                .gap(theme.space.xxs)
                .px(theme.space.sm)
                .py(theme.space.xs)
                .rounded(theme.radii.sm)
                .bg(Tone::Danger.fill(theme))
                .child(Text::data_small(error).tone(Tone::Danger).ellipsize())
                .children(
                    self.error_detail
                        .map(|detail| Text::data_small(detail).muted().ellipsize()),
                )
        });
        let actions = self.actions.map(|actions| {
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(actions)
        });

        if bar.is_some() || progress.is_some() || error.is_some() || actions.is_some() {
            row = row.details(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space.xs)
                    .children(bar)
                    .children(progress)
                    .children(error)
                    .children(actions),
            );
        }
        match self.pointer {
            Some((pointer, ix)) => pointer.attach(ix, row),
            None => row,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_stops_spinning_because_it_stopped_progressing() {
        assert!(JobStatus::Running.spins());
        assert!(!JobStatus::Cancelling.spins());
    }

    #[test]
    fn a_failure_is_not_finished_with() {
        assert!(JobStatus::Done.is_finished());
        assert!(JobStatus::Cancelled.is_finished());
        assert!(!JobStatus::Failed.is_finished());
    }

    #[test]
    fn the_glyph_id_is_a_child_of_the_row_id() {
        let row = JobRow::new("job-1", JobStatus::Running, "Clone");
        assert_ne!(row.glyph_id(), ElementId::from("job-1"));
    }
}
