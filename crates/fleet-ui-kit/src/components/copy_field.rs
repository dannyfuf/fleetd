//! `CopyField` — a read-only value in a field box, with the button that copies it: the detail
//! panel's worktree path.
//!
//! The box states the value in mono and ellipsizes it; it is not an input and takes no focus.
//! The trailing slot is the caller's [`super::IconButton`] (normally `Copy path`), which carries
//! its action and shows its key in the tooltip. Use a [`super::TextInput`] for anything the
//! user edits, and a [`super::FactRow`] for a value that needs no action.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme};

/// A value with a copy button. See the module doc.
#[derive(IntoElement)]
pub struct CopyField {
    value: SharedString,
    button: Option<AnyElement>,
}

impl CopyField {
    /// A field showing `value`, already shortened by the caller where that matters (`~`).
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            button: None,
        }
    }

    /// The trailing button, normally an `IconButton` that copies the value.
    pub fn button(mut self, button: impl IntoElement) -> Self {
        self.button = Some(button.into_any_element());
        self
    }
}

impl RenderOnce for CopyField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .w_full()
            .gap(theme.space.sm)
            .h(theme.metrics.button_h)
            .pl(theme.space.md)
            .pr(theme.space.xxs)
            .rounded(theme.radii.control)
            .bg(theme.colors.control)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.control_border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Text::data_small(self.value).muted().ellipsize()),
            )
            .children(self.button)
    }
}
