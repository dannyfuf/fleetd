//! `StatusDot` — a filled dot with no glyph inside it.
//!
//! The only two dots in Fleet are the daemon liveness dot (§2.2) and the terminal-tab activity
//! dot (§3.6). Everything else that expresses state is a [`crate::components::StatusGlyph`],
//! because a shape carries more information than a color.

use gpui::{App, Hsla, Pixels, Window, div, prelude::*, px};

use crate::{theme::ActiveTheme, tone::Tone};

/// A filled circle.
#[derive(IntoElement)]
pub struct StatusDot {
    size: Pixels,
    tone: Tone,
    color: Option<Hsla>,
    opacity: Option<f32>,
}

impl StatusDot {
    /// An 8 px dot: the daemon dot.
    pub fn new(tone: Tone) -> Self {
        Self {
            size: px(8.0),
            tone,
            color: None,
            opacity: None,
        }
    }

    /// A 6 px dot: the terminal-tab activity dot.
    pub fn small(tone: Tone) -> Self {
        Self::new(tone).size(px(6.0))
    }

    /// Set the diameter.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// Override the fill color.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Lower the dot's opacity.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let mut color = self.color.unwrap_or_else(|| self.tone.color(theme));
        if let Some(opacity) = self.opacity {
            color = color.opacity(opacity);
        }
        div()
            .flex_none()
            .size(self.size)
            .rounded(theme.radii.full)
            .bg(color)
    }
}
