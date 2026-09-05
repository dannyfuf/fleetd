//! `Pane` — a bordered region with a header slot, a body slot, a footer slot, a scroll thumb
//! and the focus ring.
//!
//! §3 of the design system allows exactly two blue affordances, and neither of them is drawn
//! by the component that needs it: the pane wraps itself in [`crate::focus::FocusRing`], and a
//! row wraps itself in the cursor-bar variant. That is why no list, header or row anywhere in
//! Fleet ever reaches for `colors.accent` itself.
//!
//! The ring is a **2 px inset border that is always present** and merely changes color, so
//! focusing a pane costs zero pixels of layout: the rows behind it do not shift, which is the
//! whole point of a ring rather than an outline.
//!
//! ## States
//!
//! default · focused (2 px ring) · scrolled (3 px thumb) · raised (`surface` instead of the
//! app ground). A pane has no disabled, loading or error state: loading is
//! [`super::SkeletonRows`] in the body, empty is [`super::EmptyState`] in the body, and an
//! error is a glyph on the row it belongs to — never a tint on the container.

use gpui::{AnyElement, App, Pixels, Window, div, prelude::*, px};

use crate::{focus::FocusRing, theme::ActiveTheme};

/// Which edges carry a hairline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaneBorder {
    /// No border. The pane relies on its neighbours.
    None,
    /// A hairline on the right edge: the repos rail.
    Right,
    /// A hairline on the left edge: the detail panel and the Jobs sheet.
    #[default]
    Left,
    /// Hairlines on both vertical edges.
    Horizontal,
}

/// One region of the body.
#[derive(IntoElement)]
pub struct Pane {
    header: Option<AnyElement>,
    body: Option<AnyElement>,
    footer: Option<AnyElement>,
    focused: bool,
    width: Option<Pixels>,
    flex: bool,
    border: PaneBorder,
    raised: bool,
    scroll_fraction: Option<(f32, f32)>,
}

impl Pane {
    /// A flexible pane: the list, which must absorb every width change.
    pub fn new() -> Self {
        Self {
            header: None,
            body: None,
            footer: None,
            focused: false,
            width: None,
            flex: true,
            border: PaneBorder::None,
            raised: false,
            scroll_fraction: None,
        }
    }

    /// A fixed-width pane (240 px rail, 340 px detail panel).
    pub fn fixed(width: Pixels) -> Self {
        Self::new().width(width)
    }

    /// The 30 px header, normally a [`super::PaneHeader`].
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// The scrolling content.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// An optional pinned footer (the Jobs panel's key rows).
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// Draw the 2 px focus ring. Only one pane per screen may be focused.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Fix the width and stop flexing.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self.flex = false;
        self
    }

    /// Which edges carry a hairline.
    pub fn border(mut self, border: PaneBorder) -> Self {
        self.border = border;
        self
    }

    /// Paint the pane on `surface` instead of the app ground.
    pub fn raised(mut self, raised: bool) -> Self {
        self.raised = raised;
        self
    }

    /// Draw the 3 px scroll thumb: `(offset_fraction, visible_fraction)`, both `0..=1`.
    ///
    /// The thumb is zero-suppressed when the content fits (`visible >= 1`), because §2.10
    /// only asks for it "whenever the content overflows" — a permanent thumb would say the
    /// list is scrollable when it is not.
    pub fn scroll_thumb(mut self, offset: f32, visible: f32) -> Self {
        self.scroll_fraction = Some((offset.clamp(0.0, 1.0), visible.clamp(0.0, 1.0)));
        self
    }
}

impl Default for Pane {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Pane {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.colors.border;
        let header_h = theme.metrics.pane_header_h;

        let content = div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .children(self.header.map(|header| {
                div()
                    .flex()
                    .flex_none()
                    .h(header_h)
                    .w_full()
                    .overflow_hidden()
                    .child(header)
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .children(self.body),
            )
            .children(
                self.footer
                    .map(|footer| div().flex().flex_col().flex_none().w_full().child(footer)),
            );

        div()
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .when(self.flex, |el| el.flex_1())
            .when_some(self.width, |el, width| el.w(width).flex_none())
            .when(self.raised, |el| el.bg(theme.colors.surface))
            .map(|el| match self.border {
                PaneBorder::None => el,
                PaneBorder::Left => el.border_l(px(1.0)).border_color(border),
                PaneBorder::Right => el.border_r(px(1.0)).border_color(border),
                PaneBorder::Horizontal => {
                    el.border_l(px(1.0)).border_r(px(1.0)).border_color(border)
                }
            })
            .child(FocusRing::pane(self.focused).child(content))
            .when_some(self.scroll_fraction, |el, (offset, visible)| {
                if visible >= 1.0 {
                    return el;
                }
                // A thumb shorter than 4 % of the track stops being a thumb and becomes a
                // dot, so the visible fraction is floored rather than allowed to vanish.
                let height = visible.max(0.04);
                let top = offset.min(1.0 - height);
                el.child(
                    div()
                        .absolute()
                        .right_0()
                        .top(gpui::relative(top))
                        .h(gpui::relative(height))
                        .w(theme.metrics.scroll_thumb_w)
                        .bg(theme.colors.scroll_thumb),
                )
            })
    }
}
