//! `SplitLayout` — a fixed leading region, a hairline and a flexible trailing region.
//!
//! The Hub is two nested splits: rail | (list | detail). The component exists so that "the
//! rail never moves when the detail panel opens" (§2.1) is a property of one layout instead of
//! a convention repeated in three views.

use gpui::{AnyElement, App, Pixels, Window, div, prelude::*, px};

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

    /// Draw the hairline. On by default.
    pub fn divider(mut self, divider: bool) -> Self {
        self.divider = divider;
        self
    }
}

impl RenderOnce for SplitLayout {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let horizontal = self.axis == SplitAxis::Horizontal;
        div()
            .flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .map(|el| if horizontal { el.flex_row() } else { el.flex_col() })
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .map(|el| match self.leading_size {
                        Some(size) if horizontal => el.w(size).h_full().flex_none(),
                        Some(size) => el.h(size).w_full().flex_none(),
                        None => el.flex_1(),
                    })
                    .children(self.leading),
            )
            .when(self.divider, |el| {
                el.child(
                    div()
                        .flex_none()
                        .bg(theme.colors.border)
                        .map(|el| {
                            if horizontal {
                                el.w(px(1.0)).h_full()
                            } else {
                                el.h(px(1.0)).w_full()
                            }
                        }),
                )
            })
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .map(|el| match self.trailing_size {
                        Some(size) if horizontal => el.w(size).h_full().flex_none(),
                        Some(size) => el.h(size).w_full().flex_none(),
                        None => el.flex_1(),
                    })
                    .children(self.trailing),
            )
    }
}
