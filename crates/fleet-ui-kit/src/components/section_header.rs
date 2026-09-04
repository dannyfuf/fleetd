//! `SectionHeader` — a 20 px label row with an optional right-aligned stamp or action.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme};

/// `SESSION                   ◉ attached`
#[derive(IntoElement)]
pub struct SectionHeader {
    label: SharedString,
    trailing: Option<AnyElement>,
}

impl SectionHeader {
    /// A section label. It is uppercased by the `Label` type role.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            trailing: None,
        }
    }

    /// A right-aligned stamp (`checked 14s ago`) or action.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(theme.metrics.section_header_h)
            .w_full()
            .child(Text::label(self.label))
            .children(self.trailing)
    }
}
