//! `Banner` — a 28 px full-width strip with a countdown and prefixed keys.
//!
//! §3.12 case C. The banner never claims more than it knows: after a reconnect it must say,
//! verbatim, that terminal sessions did not survive ([D-17]), because a warm "reconnected"
//! banner is the single most damaging false reassurance in the app.

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
    /// An amber banner.
    pub fn warning(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Warning,
            icon: Some(Icon::TriangleAlert),
            countdown: None,
            hints: None,
        }
    }

    /// A red banner.
    pub fn danger(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Danger,
            icon: Some(Icon::Unplug),
            countdown: None,
            hints: None,
        }
    }

    /// Set the glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The reconnect countdown, e.g. `reconnecting in 3s`.
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
            .children(self.icon.map(|i| i.el().size(IconSize::Medium).color(color)))
            .child(Text::ui(self.text).color(color))
            .children(self.countdown.map(|c| Text::ui(c).tone(Tone::Secondary)))
            .child(div().flex_1())
            .children(self.hints)
    }
}
