//! `Chip` — the 22 px pill of the context bar and the row chrome.
//!
//! Zero-suppression (§1.2) is built in: a chip whose count is `Some(0)` renders nothing at all
//! when [`Chip::zero_suppress`] is on, which is the default.

use gpui::{App, Hsla, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// An icon, an optional word and an optional count, in a pill.
#[derive(IntoElement)]
pub struct Chip {
    icon: Option<Icon>,
    text: Option<SharedString>,
    count: Option<usize>,
    tone: Tone,
    color: Option<Hsla>,
    filled: bool,
    spinning: bool,
    zero_suppress: bool,
    id: Option<SharedString>,
}

impl Chip {
    /// An empty chip. Add an icon and/or text.
    pub fn new() -> Self {
        Self {
            icon: None,
            text: None,
            count: None,
            tone: Tone::Secondary,
            color: None,
            filled: false,
            spinning: false,
            zero_suppress: true,
            id: None,
        }
    }

    /// A chip that is just a glyph and a count: the context-bar chips.
    pub fn counter(icon: Icon, count: usize) -> Self {
        Self::new().icon(icon).count(count)
    }

    /// A chip that is a glyph and a word: the host chip, the degraded chip.
    pub fn labeled(icon: Icon, text: impl Into<SharedString>) -> Self {
        Self::new().icon(icon).text(text)
    }

    /// Set the leading glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Set the word.
    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Set the trailing count.
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// Set the tone.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Override the color.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Draw a low-alpha fill behind the chip. Off by default; the context bar is flat.
    pub fn filled(mut self, filled: bool) -> Self {
        self.filled = filled;
        self
    }

    /// Spin the glyph (the jobs chip). Requires [`Chip::id`].
    pub fn spinning(mut self, spinning: bool) -> Self {
        self.spinning = spinning;
        self
    }

    /// Stable id for the spin animation.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Turn zero-suppression off, so `count == 0` still renders.
    pub fn zero_suppress(mut self, suppress: bool) -> Self {
        self.zero_suppress = suppress;
        self
    }

    /// Whether this chip will draw anything.
    pub fn is_visible(&self) -> bool {
        !(self.zero_suppress && self.count == Some(0) && self.text.is_none())
    }
}

impl Default for Chip {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Chip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.is_visible() {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let color = self.color.unwrap_or_else(|| self.tone.color(theme));
        let fill = self.tone.fill(theme);
        let id = self.id.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .h(theme.metrics.chip_h)
            .when(self.filled, |el| {
                el.px(theme.space.sm)
                    .rounded(theme.radii.full)
                    .bg(fill)
            })
            .children(self.icon.map(|icon| {
                icon.el()
                    .size(IconSize::Medium)
                    .color(color)
                    .spinning(self.spinning)
                    .id(gpui::ElementId::from(
                        id.unwrap_or(SharedString::new_static("chip")),
                    ))
            }))
            .children(self.text.map(|text| Text::ui(text).color(color)))
            .children(
                self.count
                    .map(|count| Text::ui(count.to_string()).color(color)),
            )
            .into_any_element()
    }
}
