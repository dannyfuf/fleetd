//! `Pane` — a bordered region with a header slot, a body slot and a scroll thumb.
//!
//! The pane owns the focus ring (§2.2) so that no list, header or row ever draws blue itself.

use gpui::{AnyElement, App, Pixels, Window, div, prelude::*};

use crate::theme::ActiveTheme;

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
    /// A flexible pane.
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

    /// Draw the 2 px focus ring.
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

    /// Draw the 3 px scroll thumb: `(offset_fraction, visible_fraction)`, both 0..=1.
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
        let ring = theme.metrics.focus_ring_w;
        div()
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .min_w_0()
            .when(self.flex, |el| el.flex_1())
            .when_some(self.width, |el, width| el.w(width).flex_none())
            .when(self.raised, |el| el.bg(theme.colors.surface))
            .map(|el| match self.border {
                PaneBorder::None => el,
                PaneBorder::Left => el.border_l(gpui::px(1.0)).border_color(border),
                PaneBorder::Right => el.border_r(gpui::px(1.0)).border_color(border),
                PaneBorder::Horizontal => el
                    .border_l(gpui::px(1.0))
                    .border_r(gpui::px(1.0))
                    .border_color(border),
            })
            .children(self.header.map(|header| {
                div()
                    .flex_none()
                    .h(theme.metrics.pane_header_h)
                    .w_full()
                    .child(header)
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .children(self.body),
            )
            .children(self.footer)
            .when_some(self.scroll_fraction, |el, (offset, visible)| {
                el.child(
                    div()
                        .absolute()
                        .right_0()
                        .top(gpui::relative(offset))
                        .h(gpui::relative(visible.max(0.04)))
                        .w(theme.metrics.scroll_thumb_w)
                        .bg(theme.colors.scroll_thumb),
                )
            })
            .when(self.focused, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border(ring)
                        .border_color(theme.colors.focus_ring),
                )
            })
    }
}
