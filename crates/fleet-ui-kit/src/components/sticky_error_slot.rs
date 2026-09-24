//! `StickyErrorSlot` — red, addressable with `!`, persists until dismissed.
//!
//! §1.8: errors are sticky and actionable; successes are transient. This slot owns the last
//! failed job, replaces the job ticker while present, and never decays on a timer.
//!
//! It is the reason Fleet has no error toasts. A toast that carries the only copy of "gh:
//! HTTP 502" is a bug: the one message the user needs is also the one that disappears while
//! they are reading something else. The slot keeps it, names the key that focuses it in its
//! tooltip, and lets the user decide when it is over.
//!
//! ## Pointer and keyboard (ADR 0023)
//!
//! ```text
//! [ ⚠ gh: HTTP 502  x3 ][ ✕ ]
//! ```
//!
//! The error itself is one control: clicking it runs [`StickyErrorSlot::action`] (Fleet's `!`,
//! which opens the failure in the Jobs panel), and its tooltip names that action's key, read from
//! the live keymap by the caller. The ✕ beside it is [`StickyErrorSlot::dismiss_action`]
//! and paints `sticky_error.close`. The two are siblings, not nested, so a click on the ✕ never
//! also opens the failure.

use gpui::{Action, App, ElementId, SharedString, Window, div, prelude::*};

use super::{
    Kbd,
    button::ButtonSize,
    dismiss::{Dismiss, dismiss_builders},
    tooltip::{Tooltip, WithTooltip},
};
use crate::{
    harness::HarnessTargetExt as _,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

type ActivateFn = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// `⚠ gh: HTTP 502`
#[derive(IntoElement)]
pub struct StickyErrorSlot {
    id: ElementId,
    text: SharedString,
    kbd: Option<Kbd>,
    count: usize,
    action: Option<Box<dyn Action>>,
    on_activate: Option<ActivateFn>,
    dismiss: Option<Dismiss>,
}

impl StickyErrorSlot {
    /// An error reading `text`. Give it an [`StickyErrorSlot::action`] to make it clickable.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            kbd: None,
            count: 1,
            action: None,
            on_activate: None,
            dismiss: None,
        }
    }

    /// The key that focuses the error, named in the tooltip; resolved by the caller from the
    /// live keymap.
    ///
    /// Inside the Workspace this is the prefixed chord (`⌃S !`, never `!`) — [D-8]: over a
    /// terminal a bare `!` goes to the PTY.
    pub fn kbd(mut self, kbd: Option<Kbd>) -> Self {
        self.kbd = kbd;
        self
    }

    /// How many failures this slot stands for. Rendered `xN` and suppressed at one, so a
    /// flapping `gh` does not scroll the same line past the user twenty times.
    pub fn count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    /// What a click on the error does: dispatch `action` to the focused element, exactly as its
    /// key would.
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(action);
        self
    }

    /// What a click on the error does, for a caller whose activation is not an action.
    pub fn on_activate(mut self, on_activate: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Box::new(on_activate));
        self
    }

    dismiss_builders!();
}

impl RenderOnce for StickyErrorSlot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = theme.colors.danger;
        let count = self.count;
        let hover_bg = color.opacity(theme.metrics.error_hover_opacity);
        let activate: Option<ActivateFn> = match (self.action, self.on_activate) {
            (Some(action), _) => Some(Box::new(move |window: &mut Window, cx: &mut App| {
                window.dispatch_action(action.boxed_clone(), cx);
            })),
            (None, on_activate) => on_activate,
        };
        let close = self.dismiss.as_ref().map(|dismiss| {
            dismiss
                .close_button("sticky-error-close")
                .size(ButtonSize::Compact)
                .harness_target("sticky_error.close")
        });

        let body = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .min_w_0()
            .h(theme.metrics.chip_h)
            .px(theme.space.xs)
            .rounded(theme.radii.sm)
            // A low-alpha danger fill, not a solid one: the status bar is 28 px tall and a
            // saturated block there reads as a broken app rather than a failed job.
            .bg(Tone::Danger.fill(theme))
            .child(
                div()
                    .flex_none()
                    .child(Icon::TriangleAlert.el().size(IconSize::Small).color(color)),
            )
            .child(Text::ui(self.text).tone(Tone::Danger).ellipsize())
            .children((count > 1).then(|| {
                Text::hint(format!("x{count}"))
                    .tone(Tone::Danger)
                    .flex_none()
            }))
            .when_some(self.kbd, |el, kbd| {
                el.with_tooltip(Tooltip::new("Show the failure").kbd(kbd), cx)
            })
            .when_some(activate, |el, activate| {
                super::control::on_activate(
                    el.hover(move |s| s.bg(hover_bg)),
                    "Show the failure",
                    activate,
                )
            });

        div()
            .flex()
            .items_center()
            .gap(theme.space.xxs)
            .min_w_0()
            .child(body)
            .children(close)
    }
}
