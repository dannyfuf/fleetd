//! `DropSlot` — where a dragged tile will land, drawn over the gap between tiles.
//!
//! An accent line marks the insertion boundary without taking part in layout. The sentence that
//! says what the drop will do (`Drop to start FLT-3 · codex will pick it up`) remains its
//! accessible label. It is not a tile or a button: nothing in it is pressable, and it exists
//! only while a drag hovers the place it marks. A [`super::KanbanColumn`] draws it through
//! [`super::KanbanColumn::drop_slot`]; use it directly only inside a relative container.

use gpui::{App, Role, SharedString, Window, div, prelude::*};

use crate::theme::ActiveTheme;

#[derive(Clone, Copy, Default)]
enum MarkerEdge {
    /// Over the top edge of the first tile: a list clips above its first item, so the gap-centred
    /// `Before` offset would draw nothing there.
    Top,
    Before,
    After,
    #[default]
    Inside,
}

/// The landing place of a dragged tile.
#[derive(IntoElement)]
pub struct DropSlot {
    label: SharedString,
    edge: MarkerEdge,
}

impl DropSlot {
    /// A slot saying `label`, the sentence the drop will carry out.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            edge: MarkerEdge::Inside,
        }
    }

    /// Centre the marker in the gap immediately before its relative parent.
    pub fn before(mut self) -> Self {
        self.edge = MarkerEdge::Before;
        self
    }

    /// Draw the marker on the top edge of its relative parent: the first tile of a clipped list.
    pub fn top(mut self) -> Self {
        self.edge = MarkerEdge::Top;
        self
    }

    /// Centre the marker on the bottom edge of its relative parent.
    pub fn after(mut self) -> Self {
        self.edge = MarkerEdge::After;
        self
    }
}

impl RenderOnce for DropSlot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let marker_h = theme.metrics.drop_marker_h;
        let gap = theme.space.sm;
        let label = self.label;
        let edge_id: usize = match self.edge {
            MarkerEdge::Top => 0,
            MarkerEdge::Before => 1,
            MarkerEdge::After => 2,
            MarkerEdge::Inside => 3,
        };
        div()
            // The marker is not interactive, but GPUI's accessibility properties live on a
            // stateful element. Its consequence sentence and edge make a stable frame-local id.
            .id((label.clone(), edge_id))
            .absolute()
            .left(theme.space.xs)
            .right(theme.space.xs)
            .h(marker_h)
            .rounded(theme.radii.pill)
            .bg(theme.colors.accent)
            .role(Role::Label)
            .aria_label(label)
            .map(|marker| match self.edge {
                MarkerEdge::Top => marker.top_0(),
                MarkerEdge::Before => marker.top(-(gap + marker_h) / 2.0),
                MarkerEdge::After => marker.bottom(-marker_h / 2.0),
                MarkerEdge::Inside => marker.top((gap - marker_h) / 2.0),
            })
    }
}
