//! `JobRow` — glyph · kind (7 ch) · target · elapsed · percent, plus a progress sub-line.
//!
//! §3.7. The `kind` column is a fixed 7-character slug so the column scans as a shape, and the
//! `target` is the real domain id (`RepoId` / `WorktreeId`) — swarm's footer showed
//! `hot-copy:<repo>`, which matched no row anywhere in the app.

use gpui::{App, ElementId, SharedString, Window, prelude::*, px};

use crate::{
    components::{Row, RowColumn, ColumnAlign, StatusGlyph, StatusKind},
    icons::Icon,
    text::Text,
    theme::{ActiveTheme, ch},
    tone::Tone,
};

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

    /// Whether the row is two lines tall (only running jobs carry a progress sub-line).
    pub fn has_progress(self) -> bool {
        matches!(self, JobStatus::Running | JobStatus::Cancelling)
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
}

impl RenderOnce for JobRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let two_line = self.progress.is_some();
        let height = if two_line {
            theme.metrics.job_row_h
        } else {
            theme.metrics.row_h
        };
        let glyph_id: ElementId = self
            .id
            .clone()
            .unwrap_or_else(|| ElementId::from(SharedString::new_static("job-row")));
        let status_kind = match self.status {
            JobStatus::Running | JobStatus::Cancelling => StatusKind::JobRunning,
            _ => StatusKind::NoSession,
        };

        let mut row = Row::new()
            .height(height)
            .selected(self.selected)
            .cursor(self.cursor)
            .leading(if self.status.has_progress() {
                StatusGlyph::new(status_kind).id(glyph_id).into_any_element()
            } else {
                self.status
                    .icon()
                    .el()
                    .color(self.status.tone().color(theme))
                    .into_any_element()
            })
            .column(RowColumn::fixed(ch(7.0), Text::data(self.kind)))
            .column(RowColumn::flex(Text::data(self.target).muted().ellipsize()));

        if let Some(elapsed) = self.elapsed {
            row = row.column(
                RowColumn::fixed(ch(6.0), Text::data(elapsed).faint()).align(ColumnAlign::Right),
            );
        }
        if let Some(percent) = self.percent {
            row = row.column(
                RowColumn::fixed(ch(5.0), Text::data(format!("{percent}%")).muted())
                    .align(ColumnAlign::Right),
            );
        }
        if let Some(key) = self.trailing_key {
            row = row.column(RowColumn::fixed(px(20.0), Text::hint(key)).align(ColumnAlign::Right));
        }
        if let Some(progress) = self.progress {
            row = row.second_line(Text::data_small(progress).faint().ellipsize());
        }
        if let Some(id) = self.id {
            row = row.id(id);
        }
        row
    }
}
