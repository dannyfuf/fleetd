//! `DoctorTable` — `CHECK STATUS DETAIL`, red on fail.
//!
//! §3.12: `ok` renders in the secondary tone, not green. Zero-suppression at the color level:
//! good news does not get a hue.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// The outcome of one doctor check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoctorStatus {
    /// Passed. Rendered `ok`, secondary.
    Ok,
    /// Passed with a caveat. Rendered `warn`, amber.
    Warn,
    /// Failed. Rendered `fail`, red.
    Fail,
}

impl DoctorStatus {
    /// The literal word.
    pub fn word(self) -> &'static str {
        match self {
            DoctorStatus::Ok => "ok",
            DoctorStatus::Warn => "warn",
            DoctorStatus::Fail => "fail",
        }
    }

    /// The tone.
    pub fn tone(self) -> Tone {
        match self {
            DoctorStatus::Ok => Tone::Secondary,
            DoctorStatus::Warn => Tone::Warning,
            DoctorStatus::Fail => Tone::Danger,
        }
    }
}

/// One `check status detail` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorRow {
    /// What was checked, e.g. `gh auth`.
    pub check: SharedString,
    /// The outcome.
    pub status: DoctorStatus,
    /// The verbatim detail line from the daemon.
    pub detail: SharedString,
}

impl DoctorRow {
    /// A row.
    pub fn new(
        check: impl Into<SharedString>,
        status: DoctorStatus,
        detail: impl Into<SharedString>,
    ) -> Self {
        Self {
            check: check.into(),
            status,
            detail: detail.into(),
        }
    }
}

/// The doctor output as a fixed three-column table.
#[derive(IntoElement)]
pub struct DoctorTable {
    rows: Vec<DoctorRow>,
    check_width: Pixels,
    status_width: Pixels,
}

impl DoctorTable {
    /// A table over the given rows.
    pub fn new(rows: impl IntoIterator<Item = DoctorRow>) -> Self {
        Self {
            rows: rows.into_iter().collect(),
            check_width: px(120.0),
            status_width: px(64.0),
        }
    }

    /// Width of the `CHECK` column.
    pub fn check_width(mut self, width: Pixels) -> Self {
        self.check_width = width;
        self
    }

    /// Width of the `STATUS` column.
    pub fn status_width(mut self, width: Pixels) -> Self {
        self.status_width = width;
        self
    }
}

impl RenderOnce for DoctorTable {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let check_w = self.check_width;
        let status_w = self.status_width;
        let header = div()
            .flex()
            .items_center()
            .h(theme.metrics.section_header_h)
            .child(Text::label("check").w(check_w))
            .child(Text::label("status").w(status_w))
            .child(Text::label("detail"));

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(header)
            .children(self.rows.into_iter().map(|row| {
                let tone = row.status.tone();
                div()
                    .flex()
                    .items_center()
                    .h(theme.metrics.row_h)
                    .child(Text::data(row.check).w(check_w))
                    .child(Text::data(row.status.word()).tone(tone).w(status_w))
                    .child(Text::data(row.detail).tone(tone).ellipsize())
            }))
    }
}
