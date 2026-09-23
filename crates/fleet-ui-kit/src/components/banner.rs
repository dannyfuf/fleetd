//! `Banner` — a 40 px full-width strip: what is wrong, why it is safe, and the buttons that act.
//!
//! §3.12 case C, and **only** case C: a daemon that died while the user was attached. Cases A
//! and B are chrome-less full-window surfaces ([`super::DaemonSplash`]), because a strip cannot
//! carry a spinner plus a socket path, or a mono tail of the daemon log.
//!
//! The banner never claims more than it knows. After a restart it must state what became of the
//! user's terminals [D-17] — how many were reattached, or that none survived. Terminals normally
//! do survive, because each PTY lives in a detached holder process (`ARCHITECTURE.md`, "Detached
//! PTY holders"), but a banner that assumes it would be the single most damaging false
//! reassurance in the app the one time it is wrong.
//!
//! ## Anatomy
//!
//! ```text
//! [ ⚠ ][ Lost connection to fleetd ][ — reconnecting in 3s ][ Your terminals … ]  [ Reconnect now r ][ Open log l ][ ✕ ]
//! ```
//!
//! The sentence is in the tone's colour; the countdown and the reassurance are secondary, each in
//! its own slot, so the sentence stays still while the number cycles `3s → reconnecting… → 6s`.
//! A line of text that reflows once a second cannot be read.
//!
//! ## States
//!
//! warning (amber) · danger (red). Both paint the tone's 14 % fill and a hairline underneath,
//! so the strip reads as chrome rather than as content. Buttons and the ✕ are optional: a banner
//! that only reports (`reconnected`) has neither.
//!
//! ## Pointer and keyboard (ADR 0023)
//!
//! Every button is a kit [`Button`] wired with [`Button::action`], so a click dispatches the same
//! action as the key, and the chip on the button is that key read from the live keymap. The ✕ is
//! [`Banner::dismiss_action`]. The **screen** binds the keys; the banner never types one.
//!
//! Harness names: the buttons paint `banner.button[N]`, `0` leftmost, and the ✕ `banner.close`.
//!
//! The embedded Git UI (`fleet-lazygit`) keeps lazygit's own vocabulary and states its keys as a
//! [`KeyHintRow`] ([`Banner::hints`]) rather than as buttons; Fleet's own chrome never does.

use gpui::{App, SharedString, Window, div, prelude::*};

use super::{
    KeyHintRow,
    button::{Button, ButtonSize},
    dismiss::{Dismiss, dismiss_builders},
};
use crate::{
    harness::HarnessTargetExt as _,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A full-width strip.
#[derive(IntoElement)]
pub struct Banner {
    text: SharedString,
    tone: Tone,
    icon: Option<Icon>,
    countdown: Option<SharedString>,
    detail: Option<SharedString>,
    buttons: Vec<Button>,
    hints: Option<KeyHintRow>,
    dismiss: Option<Dismiss>,
}

impl Banner {
    fn new(text: SharedString, tone: Tone, icon: Icon) -> Self {
        Self {
            text,
            tone,
            icon: Some(icon),
            countdown: None,
            detail: None,
            buttons: Vec::new(),
            hints: None,
            dismiss: None,
        }
    }

    /// An amber banner: something is wrong and recoverable.
    pub fn warning(text: impl Into<SharedString>) -> Self {
        Self::new(text.into(), Tone::Warning, Icon::TriangleAlert)
    }

    /// A red banner: something is broken and the user has lost work or reach.
    pub fn danger(text: impl Into<SharedString>) -> Self {
        Self::new(text.into(), Tone::Danger, Icon::Unplug)
    }

    /// Set the glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The reconnect countdown, e.g. `reconnecting in 3s`. Rendered in its own secondary slot
    /// so the sentence in front of it never reflows.
    pub fn countdown(mut self, countdown: impl Into<SharedString>) -> Self {
        self.countdown = Some(countdown.into());
        self
    }

    /// The reassurance after the sentence, e.g. `Your terminals and agents keep running.`
    /// Secondary, and the first thing to be cut when the window is narrow.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Append a button at the right end. Wire it with [`Button::action`] so the click and the
    /// key share one path; the banner makes it compact and names it `banner.button[N]`.
    pub fn button(mut self, button: Button) -> Self {
        self.buttons.push(button);
        self
    }

    /// Key hints in place of buttons, for the embedded Git UI only (see the module docs).
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.hints = Some(hints);
        self
    }

    dismiss_builders!();
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = self.tone.color(theme);
        let close = self.dismiss.as_ref().map(|dismiss| {
            dismiss
                .close_button("banner-close")
                .size(ButtonSize::Compact)
                .harness_target("banner.close")
        });
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .size_full()
            .pl(theme.space.lg)
            .pr(theme.space.md)
            .bg(self.tone.fill(theme))
            .border_b(theme.metrics.hairline)
            .border_color(color.opacity(theme.metrics.banner_border_opacity))
            .children(
                self.icon
                    .map(|i| i.el().size(IconSize::Medium).color(color)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.space.sm)
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w_0()
                            .child(Text::ui_strong(self.text).color(color).ellipsize()),
                    )
                    .children(self.countdown.map(|c| {
                        div()
                            .flex()
                            .flex_none()
                            .child(Text::ui(c).tone(Tone::Secondary))
                    }))
                    .children(self.detail.map(|detail| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Text::ui(detail).tone(Tone::Secondary).ellipsize())
                    })),
            )
            .children(
                self.hints
                    .map(|hints| div().flex().flex_none().child(hints)),
            )
            .children(self.buttons.into_iter().enumerate().map(|(ix, button)| {
                button
                    .size(ButtonSize::Compact)
                    .harness_target_indexed("banner.button", ix)
            }))
            .children(close)
    }
}
