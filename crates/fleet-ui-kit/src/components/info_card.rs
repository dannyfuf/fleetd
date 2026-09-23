//! `InfoCard` — a raised, rounded box that groups a few lines about one thing inside a detail
//! panel: the worktree panel's *Session* card.
//!
//! A sentence-case title (`Session`), an optional right-aligned stamp beside it, and a column of
//! lines the caller already built. It is a card on the page, not a surface: it has no shadow, no
//! header bar and no actions of its own — the panel's action row owns those.
//!
//! Not a [`super::Callout`], which tints a consequence inside a dialog. Not a
//! [`super::KeyValueList`], which is the flat `label  value` block; put one *inside* a card when
//! the facts need grouping, or leave the facts flat when the panel already reads as sections.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme};

/// A titled card of lines. See the module doc.
#[derive(IntoElement)]
pub struct InfoCard {
    title: Option<SharedString>,
    trailing: Option<AnyElement>,
    lines: Vec<AnyElement>,
}

impl InfoCard {
    /// An empty card.
    pub fn new() -> Self {
        Self {
            title: None,
            trailing: None,
            lines: Vec::new(),
        }
    }

    /// The sentence-case title, e.g. `Session`.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A right-aligned stamp on the title line.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// Append one line, top to bottom.
    pub fn line(mut self, line: impl IntoElement) -> Self {
        self.lines.push(line.into_any_element());
        self
    }
}

impl Default for InfoCard {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for InfoCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let head = (self.title.is_some() || self.trailing.is_some()).then(|| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.space.sm)
                .children(self.title.map(Text::sentence_label))
                .children(self.trailing)
        });
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.sm)
            .px(theme.space.md)
            .py(theme.space.md)
            .rounded(theme.radii.card)
            .bg(theme.colors.surface_raised)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .children(head)
            .children(self.lines)
    }
}
