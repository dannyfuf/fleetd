//! `DoctorTable` — `CHECK STATUS DETAIL`, red on fail.
//!
//! §3.12: `ok` renders in the secondary tone, not green. Zero-suppression at the color level:
//! good news does not get a hue.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// Width of the `CHECK` column, in pixels.
pub const CHECK_WIDTH: f32 = 120.0;

/// Width of the `STATUS` column, in pixels.
pub const STATUS_WIDTH: f32 = 64.0;

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
    ///
    /// `ok` is **secondary**, never green: good news does not get a hue (§1.2, applied at the
    /// color level). Only the two rows a user has to act on carry one.
    pub fn tone(self) -> Tone {
        match self {
            DoctorStatus::Ok => Tone::Secondary,
            DoctorStatus::Warn => Tone::Warning,
            DoctorStatus::Fail => Tone::Danger,
        }
    }

    /// The glyph, for the two statuses that have one. `ok` has none, for the same reason it has
    /// no hue.
    pub fn icon(self) -> Option<Icon> {
        match self {
            DoctorStatus::Ok => None,
            DoctorStatus::Warn => Some(Icon::TriangleAlert),
            DoctorStatus::Fail => Some(Icon::CircleX),
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
            check_width: px(CHECK_WIDTH),
            status_width: px(STATUS_WIDTH),
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
            .w_full()
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
                let color = tone.color(theme);
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .h(theme.metrics.row_h)
                    .child(Text::data(row.check).w(check_w))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space.xs)
                            .w(status_w)
                            .flex_none()
                            .children(
                                row.status
                                    .icon()
                                    .map(|icon| icon.el().size(IconSize::Small).color(color)),
                            )
                            .child(Text::data(row.status.word()).tone(tone)),
                    )
                    // The detail line is the daemon's, verbatim; it ellipsizes rather than
                    // wrapping so the table stays one row per check.
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .child(Text::data(row.detail).tone(tone).ellipsize()),
                    )
            }))
    }
}
