//! `SplitLayout` — a fixed region, a hairline and a flexible region.
//!
//! The Hub is two nested splits: `rail | (list | detail)`. The component exists so that "the
//! rail never moves when `i` opens the detail panel" (§2.1) is a property of one layout
//! instead of a convention repeated in three views: fix the **rail** and the **detail
//! panel**, never the list, and the list is the only thing that can absorb the change.
//!
//! A region with a fixed size stops flexing and never shrinks below it; the flexible region
//! carries `min_w_0` / `min_h_0` so its content ellipsizes instead of pushing the fixed side
//! out of place.

use gpui::{AnyElement, App, Pixels, Window, div, prelude::*};

use crate::theme::ActiveTheme;

/// Which way the split runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SplitAxis {
    /// Side by side.
    #[default]
    Horizontal,
    /// Stacked.
    Vertical,
}

/// Two regions and the hairline between them.
#[derive(IntoElement)]
pub struct SplitLayout {
    axis: SplitAxis,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    leading_size: Option<Pixels>,
    trailing_size: Option<Pixels>,
    divider: bool,
}

impl SplitLayout {
    /// A horizontal split.
    pub fn horizontal() -> Self {
        Self {
            axis: SplitAxis::Horizontal,
            leading: None,
            trailing: None,
            leading_size: None,
            trailing_size: None,
            divider: true,
        }
    }

    /// A vertical split.
    pub fn vertical() -> Self {
        Self {
            axis: SplitAxis::Vertical,
            ..Self::horizontal()
        }
    }

    /// The leading region.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// The trailing region.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// Fix the leading region's size. It stops flexing.
    pub fn leading_size(mut self, size: Pixels) -> Self {
        self.leading_size = Some(size);
        self
    }

    /// Fix the trailing region's size, e.g. the 340 px detail panel.
    pub fn trailing_size(mut self, size: Pixels) -> Self {
        self.trailing_size = Some(size);
        self
    }

    /// Draw the hairline. On by default, and zero-suppressed when one side is empty: a
    /// divider with nothing on the other side of it is a line that means nothing.
    pub fn divider(mut self, divider: bool) -> Self {
        self.divider = divider;
        self
    }
}

impl RenderOnce for SplitLayout {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let horizontal = self.axis == SplitAxis::Horizontal;
        let divider = self.divider && self.leading.is_some() && self.trailing.is_some();
        let leading_size = self.leading_size;
        let trailing_size = self.trailing_size;

        let region = |size: Option<Pixels>, child: AnyElement| {
            div()
                .flex()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .map(|el| match size {
                    Some(size) if horizontal => el.w(size).h_full().flex_none(),
                    Some(size) => el.h(size).w_full().flex_none(),
                    None if horizontal => el.flex_1().h_full(),
                    None => el.flex_1().w_full(),
                })
                .child(child)
        };

        div()
            .flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .map(|el| {
                if horizontal {
                    el.flex_row()
                } else {
                    el.flex_col()
                }
            })
            .children(self.leading.map(|child| region(leading_size, child)))
            .when(divider, |el| {
                el.child(div().flex_none().bg(theme.colors.border).map(|el| {
                    if horizontal {
                        el.w(theme.metrics.hairline).h_full()
                    } else {
                        el.h(theme.metrics.hairline).w_full()
                    }
                }))
            })
            .children(self.trailing.map(|child| region(trailing_size, child)))
    }
}
