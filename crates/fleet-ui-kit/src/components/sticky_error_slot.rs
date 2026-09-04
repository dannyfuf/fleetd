//! `StickyErrorSlot` — red, addressable with `!`, persists until dismissed.
//!
//! §1.8: errors are sticky and actionable; successes are transient. This slot owns the last
//! failed job, replaces the job ticker while present, and never decays on a timer.
//!
//! It is the reason Fleet has no error toasts. A toast that carries the only copy of "gh:
//! HTTP 502" is a bug: the one message the user needs is also the one that disappears while
//! they are reading something else. The slot keeps it, states the key that focuses it, and
//! lets the user decide when it is over.

use gpui::{App, ElementId, SharedString, Window, div, prelude::*};

use crate::{
    components::KeyHint,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// `⚠ gh: HTTP 502 · !`
#[derive(IntoElement)]
pub struct StickyErrorSlot {
    id: Option<ElementId>,
    text: SharedString,
    key: SharedString,
    count: usize,
    #[allow(clippy::type_complexity)]
    on_activate: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl StickyErrorSlot {
    /// An error with the default `!` focus key.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            text: text.into(),
            key: SharedString::new_static("!"),
            count: 1,
            on_activate: None,
        }
    }

    /// Override the key that focuses the error.
    ///
    /// Inside the Workspace this must carry its prefix (`^s !`, never `!`) — [D-8]: in
    /// Terminal mode a bare `!` goes to the PTY.
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = key.into();
        self
    }

    /// How many failures this slot stands for. Rendered `xN` and suppressed at one, so a
    /// flapping `gh` does not scroll the same line past the user twenty times.
    pub fn count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    /// Mouse parity for the focus key. Requires [`StickyErrorSlot::id`] to be set.
    pub fn on_activate(mut self, on_activate: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Box::new(on_activate));
        self
    }

    /// A stable id, needed for hover and [`StickyErrorSlot::on_activate`].
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl RenderOnce for StickyErrorSlot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = theme.colors.danger;
        let on_activate = self.on_activate;
        let count = self.count;
        let hover_bg = color.opacity(0.22);

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .min_w_0()
            .h(theme.metrics.chip_h)
            .px(theme.space.xs)
            .rounded(theme.radii.sm)
            // A low-alpha danger fill, not a solid one: the status bar is 26 px tall and a
            // saturated block there reads as a broken app rather than a failed job.
            .bg(Tone::Danger.fill(theme))
            .child(
                div()
                    .flex_none()
                    .child(Icon::TriangleAlert.el().size(IconSize::Small).color(color)),
            )
            .child(Text::ui(self.text).tone(Tone::Danger).ellipsize())
            .children((count > 1).then(|| {
                div()
                    .flex_none()
                    .child(Text::hint(format!("x{count}")).tone(Tone::Danger))
            }))
            .child(div().flex_none().child(KeyHint::new(self.key)));

        match self.id {
            Some(id) => body
                .id(id)
                .hover(move |s| s.bg(hover_bg))
                .when_some(on_activate, |el, on_activate| {
                    el.cursor_pointer()
                        .on_click(move |_, window, cx| on_activate(window, cx))
                })
                .into_any_element(),
            None => body.into_any_element(),
        }
    }
}
