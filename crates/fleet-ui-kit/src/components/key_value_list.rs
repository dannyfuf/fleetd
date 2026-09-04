//! `KeyValueList` — a block of [`super::FactRow`]s under an optional [`super::SectionHeader`].
//!
//! This is the shape of every detail-panel block (§3.4) and of the PR detail table (§3.5).
//! It exists so a view never has to remember the label column width twice.

use gpui::{AnyElement, App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{
    components::{FactRow, FactValue, SectionHeader, fact_row::LABEL_WIDTH},
    theme::ActiveTheme,
};

/// A titled block of label/value rows.
#[derive(IntoElement)]
pub struct KeyValueList {
    title: Option<SharedString>,
    trailing: Option<AnyElement>,
    rows: Vec<(SharedString, FactValue, bool)>,
    label_width: Pixels,
    refreshing: bool,
}

impl KeyValueList {
    /// An untitled block.
    pub fn new() -> Self {
        Self {
            title: None,
            trailing: None,
            rows: Vec::new(),
            label_width: px(LABEL_WIDTH),
            refreshing: false,
        }
    }

    /// A block under a section header.
    pub fn titled(title: impl Into<SharedString>) -> Self {
        Self::new().title(title)
    }

    /// Set the section title.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A right-aligned element on the section header, normally a
    /// [`super::FreshnessStamp`].
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// Append a row.
    pub fn row(mut self, label: impl Into<SharedString>, value: FactValue) -> Self {
        self.rows.push((label.into(), value, false));
        self
    }

    /// Append a row whose value renders in the data face.
    pub fn mono_row(mut self, label: impl Into<SharedString>, value: FactValue) -> Self {
        self.rows.push((label.into(), value, true));
        self
    }

    /// Width of the label column.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = width;
        self
    }

    /// A re-inspection is in flight: every value dims to 60 % and stays readable (§3.4).
    pub fn refreshing(mut self, refreshing: bool) -> Self {
        self.refreshing = refreshing;
        self
    }
}

impl Default for KeyValueList {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for KeyValueList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let label_width = self.label_width;
        let refreshing = self.refreshing;
        let title = self.title;
        let trailing = self.trailing;
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(title.map(|title| {
                let mut header = SectionHeader::new(title);
                if let Some(trailing) = trailing {
                    header = header.trailing(trailing);
                }
                header
            }))
            .children(self.rows.into_iter().map(|(label, value, mono)| {
                FactRow::new(label, value)
                    .label_width(label_width)
                    .mono(mono)
                    .refreshing(refreshing)
            }))
    }
}
