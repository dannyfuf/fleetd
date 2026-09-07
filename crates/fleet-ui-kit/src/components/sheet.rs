//! `Sheet` — a right-docked, non-blocking panel.
//!
//! §3.7: 440 px, 640 px when a log is expanded, full height between the two bars, 1 px left
//! border. It is deliberately **not** a centered modal: the list behind it stays fully visible
//! and fully readable, because the jobs it lists are about those rows.
//!
//! The caller owns opening and closing transitions; this component draws the supplied width.
//!
//! Place a sheet in [`super::AppFrame::body_overlay`], not in `overlay`: §3.7 pins it to the
//! region *between* the context bar and the status bar, so both bars stay reachable while it
//! is open.

use gpui::{AnyElement, App, Pixels, Window, deferred, div, prelude::*};

use crate::{components::OverlayLayer, theme::ActiveTheme};

/// The right-docked panel.
#[derive(IntoElement)]
pub struct Sheet {
    open: bool,
    expanded: bool,
    width: Option<Pixels>,
    header: Option<AnyElement>,
    body: Option<AnyElement>,
    footer: Option<AnyElement>,
}

impl Sheet {
    /// A sheet. When `open` is false it renders nothing.
    pub fn new(open: bool) -> Self {
        Self {
            open,
            expanded: false,
            width: None,
            header: None,
            body: None,
            footer: None,
        }
    }

    /// Widen to the expanded width (640 px) for an inline log view.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Override the width. A view animating the 160 ms slide passes the interpolated width
    /// here each frame.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// The header block: the panel title, its counts and the selected job's log path.
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// The scrolling body.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// The pinned key rows.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// The width this sheet resolves to, given a theme. Exposed so a view can drive the
    /// slide without re-deriving the 440 / 640 rule.
    pub fn resolved_width(&self, theme: &crate::theme::Theme) -> Pixels {
        self.width.unwrap_or(if self.expanded {
            theme.metrics.sheet_expanded_w
        } else {
            theme.metrics.sheet_w
        })
    }
}

impl RenderOnce for Sheet {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let width = self.resolved_width(theme);

        deferred(
            div().absolute().inset_0().flex().justify_end().child(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .w(width)
                    .bg(theme.colors.elevated)
                    .border_l(theme.metrics.hairline)
                    .border_color(theme.colors.border_strong)
                    .shadow(theme.sheet_shadow())
                    .overflow_hidden()
                    .occlude()
                    .children(self.header.map(|header| {
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w_full()
                            .border_b(theme.metrics.hairline)
                            .border_color(theme.colors.border)
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
                    .children(self.footer.map(|footer| {
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w_full()
                            .border_t(theme.metrics.hairline)
                            .border_color(theme.colors.border)
                            .child(footer)
                    })),
            ),
        )
        .with_priority(OverlayLayer::Sheet.priority())
        .into_any_element()
    }
}
