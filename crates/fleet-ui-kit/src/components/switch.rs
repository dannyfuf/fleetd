//! `Switch` — a pill track with a knob: on is the accent fill with the knob at the end, off is a
//! muted track with the knob at the start.
//!
//! Usage rule: a boolean setting puts one inside a [`super::SettingsRow`] as its `control`, at
//! the row's end; a rule row whose switch enables what the row describes puts it in the row's
//! `leading` slot instead. The row keeps the label, the helper, the cursor and the keyboard
//! (`Space`). A choice between named options is a [`super::SegmentedControl`] (through
//! [`super::Cycler::inline`]), not two switches.
//!
//! Like a button, a switch is not focusable (ADR 0023): the row it sits in carries the cursor and
//! the key.

use gpui::{App, ElementId, MouseButton, SharedString, Toggled, Window, div, prelude::*};

use crate::theme::ActiveTheme;

type SwitchToggle = dyn Fn(bool, &mut Window, &mut App);

/// An on/off switch. The caller owns the value; a click reports the value it asks for.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    on: bool,
    disabled: bool,
    name: Option<SharedString>,
    on_toggle: Option<Box<SwitchToggle>>,
}

impl Switch {
    /// A switch showing `on`.
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        Self {
            id: id.into(),
            on,
            disabled: false,
            name: None,
            on_toggle: None,
        }
    }

    /// Dim the switch and ignore the pointer.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// What assistive technology announces: the setting's label.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// A click, with the value the switch should take (`!on`). Without it the switch is drawn
    /// only and the row's key is the way to change it.
    pub fn on_toggle(mut self, on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(on_toggle));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let inset = theme.space.xxs;
        let knob = theme.metrics.switch_h - inset * 2.0;
        let on = self.on;
        let track = if on {
            colors.accent_fill
        } else {
            colors.text_muted
        };
        let hover = if on {
            colors.accent_fill_hover
        } else {
            colors.text_secondary
        };
        let on_toggle = self.on_toggle.filter(|_| !self.disabled);

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .w(theme.metrics.switch_w)
            .h(theme.metrics.switch_h)
            .px(inset)
            .rounded(theme.radii.pill)
            .bg(track)
            .when(on, |el| el.justify_end())
            .when(self.disabled, |el| el.opacity(theme.metrics.dimmed_opacity))
            .role(gpui::Role::Switch)
            .when_some(self.name, |el, name| el.aria_label(name))
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .when_some(on_toggle, |el, on_toggle| {
                el.cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        on_toggle(!on, window, cx);
                        cx.stop_propagation();
                    })
            })
            .child(
                div()
                    .size(knob)
                    .rounded(theme.radii.pill)
                    .bg(colors.accent_fill_text),
            )
    }
}
