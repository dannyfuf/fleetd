//! `Tooltip` — a short label and, when the control has one, its [`Kbd`], floating over the
//! control the pointer rests on.
//!
//! A tooltip never carries information the keyboard user lacks: it names an icon-only control
//! and repeats that control's key. It appears after `motion.tooltip_delay` through gpui's own
//! tooltip slot, so [`WithTooltip::with_tooltip`] works on any element with an id.
//!
//! This is the one `Render` in the kit that owns no state: gpui's tooltip slot takes an
//! `AnyView`, so the tooltip has to be an entity. It is built fresh each time the tooltip
//! opens and holds only what it paints.

use std::time::Duration;

use gpui::{
    AnyView, App, Context, SharedString, StatefulInteractiveElement, Window, div, prelude::*,
};

use super::kbd::{Kbd, KbdSize, KbdTone};
use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// A tooltip's content: a label and an optional key chip.
#[derive(IntoElement, Clone, Debug, PartialEq)]
pub struct Tooltip {
    label: SharedString,
    kbd: Option<Kbd>,
}

impl Tooltip {
    /// A tooltip saying `label`.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            kbd: None,
        }
    }

    /// Show the control's key beside the label. `None` shows no chip, which is what an
    /// unbound action resolves to.
    pub fn kbd(mut self, kbd: impl Into<Option<Kbd>>) -> Self {
        self.kbd = kbd.into();
        self
    }

    /// The label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The view gpui's tooltip slot shows.
    pub fn build(self, _window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| TooltipView(self)).into()
    }
}

/// Attach a kit [`Tooltip`] to an element with an id, shown after `motion.tooltip_delay`.
pub trait WithTooltip: StatefulInteractiveElement + Sized {
    /// Show `tooltip` while the pointer rests on this element.
    fn with_tooltip(self, tooltip: Tooltip, cx: &App) -> Self {
        let delay = Duration::from_millis(cx.theme().motion.tooltip_delay);
        self.tooltip(move |window, cx| tooltip.clone().build(window, cx))
            .tooltip_show_delay(delay)
    }
}

impl<E: StatefulInteractiveElement> WithTooltip for E {}

struct TooltipView(Tooltip);

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // gpui places the tooltip one pixel off the pointer; the inset keeps the cursor from
        // covering the label.
        let inset = cx.theme().space.sm;
        div().pl(inset).pt(inset).child(self.0.clone())
    }
}

/// The floating surface itself. Rendered directly only by the gallery; a control shows it
/// through [`WithTooltip::with_tooltip`].
impl RenderOnce for Tooltip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let kbd = self
            .kbd
            .map(|kbd| kbd.tone(KbdTone::Default).size(KbdSize::Small));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.sm)
            .px(theme.space.sm)
            .py(theme.space.xs)
            .rounded(theme.radii.popover)
            .bg(theme.colors.elevated)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border_strong)
            .shadow(theme.popover_shadow())
            .child(Text::caption(self.label).tone(Tone::Default))
            .children(kbd)
    }
}
