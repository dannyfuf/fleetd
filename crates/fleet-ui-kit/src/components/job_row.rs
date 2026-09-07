//! `JobRow` — glyph · kind (7 ch) · target · elapsed · percent, plus a progress sub-line.
//!
//! §3.7. The `kind` column is a fixed 7-character slug so the column scans as a shape, and the
//! `target` is the real domain id (`RepoId` / `WorktreeId`) — swarm's footer showed
//! `hot-copy:<repo>`, which matched no row anywhere in the app.
//!
//! The row is deliberately two heights and no more: 44 px while a job is in flight (because the
//! last stdout line is the only way to see a stuck clone) and 30 px once it is not (because
//! finished work must stop competing for the eye). Nothing here decays on a timer — [D-9]
//! makes retention the caller's decision, and a failed job is never auto-dismissed.

use gpui::{App, ElementId, SharedString, Window, prelude::*};

use crate::{
    components::{ColumnAlign, Row, RowColumn, StatusGlyph, StatusKind},
    icons::Icon,
    text::Text,
    theme::{ActiveTheme, ch},
    tone::Tone,
};

/// The `kind` column: seven characters, so `clone` and `inspect` line up as shapes.
const KIND_CH: f32 = 7.0;
/// `m:ss`, right-aligned.
const ELAPSED_CH: f32 = 6.0;
/// `100%`, right-aligned.
const PERCENT_CH: f32 = 5.0;
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

    /// Whether the row's own text is stepped down a level, because the job is over and the
    /// eye should land on the ones that are not.
    fn is_finished(self) -> bool {
        matches!(self, JobStatus::Cancelled | JobStatus::Done)
    }
}

/// One job item.
#[derive(IntoElement)]
pub struct JobRow {
    id: Option<ElementId>,
    status: JobStatus,
    kind: SharedString,
    target: SharedString,
    elapsed: Option<SharedString>,
    percent: Option<u8>,
    progress: Option<SharedString>,
    trailing_key: Option<SharedString>,
    retryable: Option<bool>,
    selected: bool,
    cursor: bool,
}

impl JobRow {
    /// A job row. `kind` is one of the fixed slugs: clone, pool, hooks, prune, create, delete,
    /// fetch, prs, inspect, update, import.
    pub fn new(
        status: JobStatus,
        kind: impl Into<SharedString>,
        target: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: None,
            status,
            kind: kind.into(),
            target: target.into(),
            elapsed: None,
            percent: None,
            progress: None,
            trailing_key: None,
            retryable: None,
            selected: false,
            cursor: false,
        }
    }

    /// Stable id for hover and click.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// `m:ss`. The spec shows it only after 30 s; the caller decides.
    pub fn elapsed(mut self, elapsed: impl Into<SharedString>) -> Self {
        self.elapsed = Some(elapsed.into());
        self
    }

    /// Percent, when it is parseable from the job's output.
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

    /// A right-aligned key (`R` on a failed job).
    pub fn trailing_key(mut self, key: impl Into<SharedString>) -> Self {
        self.trailing_key = Some(key.into());
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

    /// The id the spinning glyph animates under.
    ///
    /// It has to be a *child* of the row id: reusing the row's own id would put two elements
    /// with the same [`ElementId`] in one frame, and the animation would then share state with
    /// the row's hover.
    fn glyph_id(&self) -> ElementId {
        match &self.id {
            Some(id) => ElementId::NamedChild(
                std::sync::Arc::new(id.clone()),
                SharedString::new_static("glyph"),
            ),
            None => ElementId::Name(SharedString::new_static("job-row-glyph")),
        }
    }
}

impl RenderOnce for JobRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let status = self.status;
        // Only a job that is actually producing output earns the second line — and therefore
        // the taller row.
        let two_line = self.progress.is_some();
        let height = if two_line {
            theme.metrics.job_row_h
        } else {
            theme.metrics.row_h
        };
        let glyph_id = self.glyph_id();
        let tone = status.tone();

        let leading = if status.spins() {
            StatusGlyph::new(StatusKind::JobRunning)
                .id(glyph_id)
                .into_any_element()
        } else {
            status
                .icon()
                .el()
                .color(tone.color(theme))
                .into_any_element()
        };

        let mut row = Row::new()
            .height(height)
            .selected(self.selected)
            .cursor(self.cursor)
            .leading(leading)
            .column(RowColumn::fixed(
                ch(KIND_CH),
                // The kind is the shape the column is scanned by, so it keeps full contrast
                // while the job is live and steps down once it is not.
                if status.is_finished() {
                    Text::data(self.kind).muted()
                } else {
                    Text::data(self.kind)
                },
            ))
            .column(RowColumn::flex(
                Text::data(self.target)
                    .tone(if status == JobStatus::Failed {
                        Tone::Default
                    } else {
                        Tone::Secondary
                    })
                    .ellipsize(),
            ));

        if let Some(elapsed) = self.elapsed {
            row = row.column(
                RowColumn::fixed(ch(ELAPSED_CH), Text::data(elapsed).faint())
                    .align(ColumnAlign::Right),
            );
        }
        if let Some(percent) = self.percent {
            row = row.column(
                RowColumn::fixed(
                    ch(PERCENT_CH),
                    Text::data(format!("{}%", percent.min(100))).muted(),
                )
                .align(ColumnAlign::Right),
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
        if let Some(key) = self.trailing_key {
            row = row.column(
                RowColumn::fixed(theme.metrics.job_key_w, Text::hint(key))
                    .align(ColumnAlign::Right),
            );
        }
        if let Some(progress) = self.progress {
            row = row.second_line(
                Text::data_small(progress)
                    // A failed job's last line *is* the error, so it is not allowed to fade
                    // into the same grey as a healthy progress line.
                    .tone(if status == JobStatus::Failed {
                        Tone::Danger
                    } else {
                        Tone::Muted
                    })
                    .ellipsize(),
            );
        }
        if let Some(id) = self.id {
            row = row.id(id);
        }
        row
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
    fn the_glyph_id_is_a_child_of_the_row_id() {
        let row = JobRow::new(JobStatus::Running, "clone", "nixos").id("job-1");
        assert_ne!(row.glyph_id(), ElementId::from("job-1"));
    }
}
