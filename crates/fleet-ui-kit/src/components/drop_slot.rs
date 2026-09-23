//! `DropSlot` — where a dragged tile will land, drawn in the gap it will take.
//!
//! A dashed, accent-washed well the height of a short tile, holding one centred caption that
//! says what the drop will do (`Drop to start FLT-3 · codex will pick it up`). It is not a
//! tile and not a button: nothing in it is pressable, and it exists only while a drag hovers
//! the place it marks. A [`super::KanbanColumn`] draws it through
//! [`super::KanbanColumn::drop_slot`]; use it directly only in a container that is not a column.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// The landing place of a dragged tile.
#[derive(IntoElement)]
pub struct DropSlot {
    label: SharedString,
}

impl DropSlot {
    /// A slot saying `label`, the sentence the drop will carry out.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl RenderOnce for DropSlot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_none()
            .w_full()
            .items_center()
            .justify_center()
            .min_h(theme.metrics.row_h_comfortable)
            .px(theme.space.md)
            .py(theme.space.lg)
            .rounded(theme.radii.card)
            .border(theme.metrics.hairline)
            .border_dashed()
            .border_color(theme.colors.accent)
            .bg(theme.colors.accent_subtle)
            .child(Text::caption(self.label).tone(Tone::Accent))
    }
}
