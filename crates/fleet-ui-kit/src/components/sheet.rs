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
//!
//! ADR 0023: a dismissable sheet ([`Sheet::dismiss_action`]) draws a close ✕ at the top of its
//! header, painted as `sheet.close`, and a click anywhere in the band outside it closes it
//! through the same action `esc` dispatches. The band is left clear by default so the rows
//! behind stay readable; [`Sheet::scrim`] darkens it for a sheet that is a detail of its own
//! (the board card).

use gpui::{AnyElement, App, Pixels, Window, deferred, div, prelude::*};

use super::dismiss::{Dismiss, dismiss_builders};
use crate::{components::OverlayLayer, harness::HarnessTargetExt, theme::ActiveTheme};

/// The window edge a [`Sheet`] docks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SheetSide {
    /// The right edge: the Jobs panel and the card detail.
    #[default]
    Right,
    /// The left edge.
    Left,
}

/// The right-docked panel.
#[derive(IntoElement)]
pub struct Sheet {
    open: bool,
    expanded: bool,
    width: Option<Pixels>,
    header: Option<AnyElement>,
    body: Option<AnyElement>,
    footer: Option<AnyElement>,
    side: SheetSide,
    scrim: bool,
    dismiss: Option<Dismiss>,
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
            side: SheetSide::Right,
            scrim: false,
            dismiss: None,
        }
    }

    dismiss_builders!();

    /// The edge to dock to. [`SheetSide::Right`] by default.
    pub fn side(mut self, side: SheetSide) -> Self {
        self.side = side;
        self
    }

    /// Darken the band outside the sheet with the `overlay` scrim. Off by default: a sheet is
    /// usually *about* the rows behind it, which must stay readable.
    pub fn scrim(mut self, scrim: bool) -> Self {
        self.scrim = scrim;
        self
    }

    /// Widen to the expanded width (640 px) for an inline log view.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Override the width: `sheet_w_detail` (736 px) for a full detail such as a board card. A
    /// view animating the 160 ms slide passes the interpolated width here each frame.
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
        let side = self.side;
        let on_outside = self.dismiss.as_ref().map(Dismiss::callback);
        let close = self.dismiss.as_ref().map(|dismiss| {
            div().flex_none().p(theme.space.sm).child(
                dismiss
                    .close_button("sheet-close")
                    .harness_target("sheet.close"),
            )
        });
        let header = match (self.header, close) {
            (None, None) => None,
            (header, close) => Some(
                div()
                    .flex()
                    .items_start()
                    .flex_none()
                    .w_full()
                    .border_b(theme.metrics.hairline)
                    .border_color(theme.colors.border)
                    .child(div().flex().flex_col().flex_1().min_w_0().children(header))
                    .children(close),
            ),
        };

        let panel = div()
            .id("sheet-panel")
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .w(width)
            .bg(theme.colors.elevated)
            .map(|el| match side {
                SheetSide::Right => el.border_l(theme.metrics.hairline),
                SheetSide::Left => el.border_r(theme.metrics.hairline),
            })
            .border_color(theme.colors.border_strong)
            .shadow(theme.sheet_shadow())
            .overflow_hidden()
            .occlude()
            .children(header)
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
            }));

        deferred(
            div()
                .id("sheet-band")
                .absolute()
                .inset_0()
                .flex()
                .map(|el| match side {
                    SheetSide::Right => el.justify_end(),
                    SheetSide::Left => el.justify_start(),
                })
                .when(self.scrim, |el| el.bg(theme.colors.overlay).occlude())
                // A dismissable sheet owns the band: a click outside the panel closes it rather
                // than reaching the rows behind, as a dialog's scrim does.
                .when_some(on_outside, |el, dismiss| {
                    el.occlude()
                        .on_click(move |_, window, cx| dismiss(window, cx))
                })
                .child(panel),
        )
        .with_priority(OverlayLayer::Sheet.priority())
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{AppContext, Context, Modifiers, Render, VisualTestContext, point, px};

    use super::*;
    use crate::components::Overlay;

    /// A sheet (or, with `overlay`, a scrimmed overlay that opts out of scrim dismissal) whose
    /// dismiss counts.
    struct Host {
        dismissed: Rc<Cell<usize>>,
        overlay: bool,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let dismissed = self.dismissed.clone();
            let on_dismiss = move |_: &mut Window, _: &mut App| dismissed.set(dismissed.get() + 1);
            div().size_full().map(|el| {
                if self.overlay {
                    el.child(
                        Overlay::new()
                            .scrim(true)
                            .dismiss_on_scrim_click(false)
                            .on_dismiss(on_dismiss)
                            .content(div().h(px(40.0))),
                    )
                } else {
                    el.child(
                        Sheet::new(true)
                            .width(px(200.0))
                            .on_dismiss(on_dismiss)
                            .body(div().size_full()),
                    )
                }
            })
        }
    }

    fn click_at(cx: &mut gpui::TestAppContext, overlay: bool, x: f32) -> usize {
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let dismissed = Rc::new(Cell::new(0));
        let counter = dismissed.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| Host {
                    dismissed: counter,
                    overlay,
                })
            })
            .expect("test window")
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        let size = cx.update(|window, _| window.viewport_size());
        let at = point(size.width - px(x), size.height / 2.0);
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        dismissed.get()
    }

    #[gpui::test]
    fn a_click_outside_a_dismissable_sheet_closes_it(cx: &mut gpui::TestAppContext) {
        assert_eq!(click_at(cx, false, 600.0), 1);
    }

    #[gpui::test]
    fn a_click_inside_the_sheet_does_not(cx: &mut gpui::TestAppContext) {
        assert_eq!(click_at(cx, false, 100.0), 0);
    }

    #[gpui::test]
    fn an_overlay_that_opts_out_ignores_a_scrim_click(cx: &mut gpui::TestAppContext) {
        assert_eq!(click_at(cx, true, 4.0), 0);
    }
}
