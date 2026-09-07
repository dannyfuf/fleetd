//! `Banner` — a 28 px full-width strip with a countdown and recovery keys.
//!
//! §3.12 case C, and **only** case C: a daemon that died while the user was attached. Cases A
//! and B are chrome-less full-window surfaces ([`super::DaemonSplash`]), because a 28 px strip
//! cannot carry a spinner plus a socket path, or a mono tail of the daemon log.
//!
//! The banner never claims more than it knows. After a reconnect it must say, verbatim, that
//! terminal sessions did **not** survive [D-17] — `ARCHITECTURE.md` is explicit that PTYs die
//! with the daemon, and a warm "reconnected" banner that implies the agents came back is the
//! single most damaging false reassurance in the app.
//!
//! ## Anatomy
//!
//! ```text
//! [ ⚠ ][ fleetd stopped ][ reconnecting in 3s ]      …      [ r reconnect · l log · esc ]
//! ```
//!
//! The countdown is a separate, secondary-toned slot rather than part of the sentence, so the
//! sentence stays still while the number cycles `3s → reconnecting… → 6s`. A line of text that
//! reflows once a second cannot be read.
//!
//! ## States
//!
//! warning (amber) · danger (red). Both paint the tone's 14 % fill and a hairline underneath,
//! so the strip reads as chrome rather than as content.
//!
//! ## Keyboard
//!
//! `r` reconnect now, `l` open log, `Esc` dismiss (the daemon dot stays red). The **screen**
//! binds them; the banner states them, and inside the Workspace they must be passed already
//! prefixed (`^s r`), because terminal mode owns every bare key.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    components::KeyHintRow,
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
    hints: Option<KeyHintRow>,
}

impl Banner {
    /// An amber banner: something is wrong and recoverable.
    pub fn warning(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Warning,
            icon: Some(Icon::TriangleAlert),
            countdown: None,
            hints: None,
        }
    }

    /// A red banner: something is broken and the user has lost work or reach.
    pub fn danger(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Danger,
            icon: Some(Icon::Unplug),
            countdown: None,
            hints: None,
        }
    }

    /// Set the glyph. Pass [`Icon::Dot`] for the `◍` daemon mark of §3.12 case C.
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

    /// The recovery keys. Inside the Workspace they must carry their `^s` prefix.
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.hints = Some(hints);
        self
    }
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = self.tone.color(theme);
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .size_full()
            .px(theme.space.lg)
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
                    .child(Text::ui(self.text).color(color).ellipsize())
                    .children(self.countdown.map(|c| {
                        div()
                            .flex()
                            .flex_none()
                            .child(Text::ui(c).tone(Tone::Secondary))
                    })),
            )
            .children(
                self.hints
                    .map(|hints| div().flex().flex_none().child(hints)),
            )
    }
}
