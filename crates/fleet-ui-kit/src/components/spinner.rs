//! `Spinner` — `loader-circle` turning once per second. The only looping animation in Fleet.

use gpui::{App, ElementId, Hsla, SharedString, Window, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    theme::ActiveTheme,
    tone::Tone,
};

/// A spinning `loader-circle`.
#[derive(IntoElement)]
pub struct Spinner {
    id: ElementId,
    size: IconSize,
    tone: Tone,
    color: Option<Hsla>,
}

impl Spinner {
    /// A spinner. The id must be stable across frames or the animation restarts every frame.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            size: IconSize::Large,
            tone: Tone::Warning,
            color: None,
        }
    }

    /// Set the glyph size.
    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    /// Set the tone. Amber by default, because "in flight" is amber everywhere in Fleet.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// An explicit color.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Spinner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or_else(|| self.tone.color(cx.theme()));
        Icon::LoaderCircle
            .el()
            .size(self.size)
            .color(color)
            .spinning(true)
            .id(self.id)
    }
}

/// A spinner with a caption, used by the cold-start screen.
#[derive(IntoElement)]
pub struct SpinnerWithLabel {
    id: ElementId,
    label: SharedString,
}

impl SpinnerWithLabel {
    /// A spinner followed by one line of UI text.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

impl RenderOnce for SpinnerWithLabel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let gap = cx.theme().space.sm;
        gpui::div()
            .flex()
            .items_center()
            .gap(gap)
            .child(Spinner::new(self.id).size(IconSize::Medium))
            .child(crate::text::Text::ui(self.label).muted())
    }
}
