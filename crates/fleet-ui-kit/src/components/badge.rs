//! `Badge` — a small square-cornered label. Not a chip: no icon, no count, no pill.
//!
//! Use a badge for a fixed vocabulary word rendered inside a row column (`Approved`,
//! `CI fail`, `default`, `current`). Use a [`crate::components::Chip`] when the thing carries
//! an icon or a count and lives in the chrome.

use gpui::{App, Hsla, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// How much the badge asserts itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BadgeStyle {
    /// Tinted text only. The default: zero chrome for a word that is already short.
    #[default]
    Bare,
    /// Tinted text on a low-alpha fill of the same hue.
    Filled,
    /// Tinted text inside a 1 px hairline.
    Outlined,
}

/// A short word carrying a tone.
#[derive(IntoElement)]
pub struct Badge {
    text: SharedString,
    tone: Tone,
    style: BadgeStyle,
    color: Option<Hsla>,
}

impl Badge {
    /// A bare badge.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Secondary,
            style: BadgeStyle::Bare,
            color: None,
        }
    }

    /// Set the tone.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the chrome level.
    pub fn style(mut self, style: BadgeStyle) -> Self {
        self.style = style;
        self
    }

    /// Override the text color.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = self.color.unwrap_or_else(|| self.tone.color(theme));
        let fill = self.tone.fill(theme);
        div()
            .flex()
            .flex_none()
            .items_center()
            .rounded(theme.radii.sm)
            // A box costs 22 px of row height, so only a badge that has to survive on a tinted
            // ground pays for one (§6.3: `Bare` by default).
            .when(self.style != BadgeStyle::Bare, |el| {
                el.h(theme.metrics.chip_h).px(theme.space.xs)
            })
            .when(self.style == BadgeStyle::Filled, |el| el.bg(fill))
            .when(self.style == BadgeStyle::Outlined, |el| {
                el.border(theme.metrics.hairline)
                    .border_color(theme.colors.border)
            })
            .child(Text::ui(self.text).color(color))
    }
}
