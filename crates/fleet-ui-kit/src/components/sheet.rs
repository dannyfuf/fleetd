//! `Sheet` — a right-docked, non-blocking panel.
//!
//! §3.7: 440 px, 640 px when a log is expanded, full height between the two bars, 1 px left
//! border, 160 ms slide. It is deliberately **not** a centered modal: the list behind it stays
//! fully visible, because the jobs it lists are about those rows.

use gpui::{AnyElement, App, Pixels, Window, deferred, div, prelude::*, px};

use crate::theme::ActiveTheme;

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

    /// Override the width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// The header block.
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
}

impl RenderOnce for Sheet {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let width = self.width.unwrap_or(if self.expanded {
            theme.metrics.sheet_expanded_w
        } else {
            theme.metrics.sheet_w
        });
        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_end()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .h_full()
                        .w(width)
                        .bg(theme.colors.elevated)
                        .border_l(px(1.0))
                        .border_color(theme.colors.border_strong)
                        .shadow(theme.sheet_shadow())
                        .occlude()
                        .children(self.header)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .children(self.body),
                        )
                        .children(self.footer),
                ),
        )
        .into_any_element()
    }
}
