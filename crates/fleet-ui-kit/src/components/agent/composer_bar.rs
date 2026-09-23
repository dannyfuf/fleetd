//! The composer's settings strip: the clickable chips that name the model and the access mode,
//! and the context meter.
//!
//! `docs/NATIVE-AGENTS.md` §2: what the next send carries is visible and clickable, not hidden
//! behind `^s m` and `^s t`. A [`ComposerChip`] is the trigger of a `PopoverMenu` the owner
//! builds; the chip itself knows nothing about models or modes. [`ContextMeter`] draws how much
//! of the model's window the thread has used.
//!
//! Use a [`crate::components::Dropdown`] instead for a labelled form field; a composer chip is
//! chrome inside the composer, so it has no border until the pointer is on it.

use gpui::{App, ElementId, SharedString, Window, div, prelude::*, relative};

use super::metrics::AGENT_CONTEXT_METER_W;
use crate::{
    components::{Kbd, Tooltip, WithTooltip as _},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
};

/// A value and a chevron that opens a menu: `gpt-5 · high ⌄`, `asks before edits ⌄`.
#[derive(IntoElement)]
pub struct ComposerChip {
    id: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    open: bool,
    tooltip: Option<SharedString>,
    kbd: Option<Kbd>,
}

impl ComposerChip {
    /// A chip reading `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            open: false,
            tooltip: None,
            kbd: None,
        }
    }

    /// Lead the value with a glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Whether the menu the chip opens is open, which holds its hover look.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// What the chip changes, and the key that opens the same menu, shown when the pointer
    /// rests on it. The key comes from the live keymap.
    pub fn tooltip(mut self, text: impl Into<SharedString>, kbd: Option<Kbd>) -> Self {
        self.tooltip = Some(text.into());
        self.kbd = kbd;
        self
    }
}

impl RenderOnce for ComposerChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let (hover, border) = (colors.control_hover, colors.control_border);
        let tooltip = self
            .tooltip
            .map(|text| Tooltip::new(text).kbd(self.kbd.clone()));
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .h(theme.metrics.button_h_compact)
            .px(theme.space.sm)
            .rounded(theme.radii.control)
            .border(theme.metrics.hairline)
            .border_color(gpui::transparent_black())
            .text_color(colors.text_secondary)
            .cursor_pointer()
            .when(self.open, |el| el.bg(hover).border_color(border))
            .hover(move |style| style.bg(hover).border_color(border))
            .role(gpui::Role::ComboBox)
            .aria_label(self.label.clone())
            .aria_expanded(self.open)
            .children(
                self.icon
                    .map(|icon| icon.el().size(IconSize::Small).color(colors.text_secondary)),
            )
            .child(Text::ui(self.label).tone(crate::tone::Tone::Secondary))
            .child(
                Icon::ChevronDown
                    .el()
                    .size(IconSize::Small)
                    .color(colors.text_secondary),
            )
            .when_some(tooltip, |el, tooltip| el.with_tooltip(tooltip, cx))
    }
}

/// How much of the model's context window the thread has used: a short track and `34%`.
#[derive(IntoElement)]
pub struct ContextMeter {
    percent: u8,
    label: SharedString,
}

impl ContextMeter {
    /// A meter at `percent`, clamped to 100, reading `label` — the owner's prepared `34%`.
    pub fn new(percent: u8, label: impl Into<SharedString>) -> Self {
        Self {
            percent: percent.min(100),
            label: label.into(),
        }
    }
}

impl RenderOnce for ContextMeter {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let fraction = f32::from(self.percent) / 100.0;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .child(
                div()
                    .w(AGENT_CONTEXT_METER_W)
                    .h(theme.space.xs)
                    .rounded(theme.radii.full)
                    .overflow_hidden()
                    .bg(theme.colors.control)
                    .child(div().h_full().w(relative(fraction)).bg(theme.colors.accent)),
            )
            .child(Text::hint(self.label).faint())
    }
}
