//! `Checkbox` — a square box and its label: a boolean that qualifies the action next to it.
//!
//! "Open after creating" beside a dialog's Create button is the model: the box changes what the
//! primary action does, rather than standing as a setting of its own. A boolean *setting* is a
//! [`super::Toggle`] row (a [`super::Switch`] in a settings list), not a checkbox.
//!
//! Like a button, a checkbox is not focusable (ADR 0023). The whole box-and-label reads as one
//! control and a click anywhere on it asks for the other value; the surface keeps whatever key
//! it already had for the same choice.

use gpui::{App, ElementId, MouseButton, SharedString, Toggled, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

type CheckboxToggle = dyn Fn(bool, &mut Window, &mut App);

/// A labelled checkbox. The caller owns the value; a click reports the value it asks for.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    label: SharedString,
    disabled: bool,
    on_toggle: Option<Box<CheckboxToggle>>,
}

impl Checkbox {
    /// A checkbox reading `label`, showing `checked`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            label: label.into(),
            disabled: false,
            on_toggle: None,
        }
    }

    /// Dim the checkbox and ignore the pointer.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A click, with the value the box should take (`!checked`). Without it the box is drawn
    /// only.
    pub fn on_toggle(mut self, on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(on_toggle));
        self
    }

    /// Whether the box reads as checked.
    pub fn is_checked(&self) -> bool {
        self.checked
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let checked = self.checked;
        let on_toggle = self.on_toggle.filter(|_| !self.disabled);
        let (fill, border) = if checked {
            (colors.accent_fill, colors.accent_fill)
        } else {
            (colors.control, colors.border_strong)
        };
        let hover = if checked {
            colors.accent_fill_hover
        } else {
            colors.control_hover
        };

        let square = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(theme.metrics.checkbox_size)
            .rounded(theme.radii.xs)
            .bg(fill)
            .border(theme.metrics.hairline)
            .border_color(border)
            .when(checked, |el| {
                el.child(
                    Icon::Check
                        .el()
                        .size(IconSize::Small)
                        .color(colors.accent_fill_text),
                )
            });

        div()
            .id(self.id)
            .group("checkbox")
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.sm)
            .when(self.disabled, |el| el.opacity(theme.metrics.dimmed_opacity))
            .role(gpui::Role::CheckBox)
            .aria_label(self.label.clone())
            .aria_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            })
            .child(square.group_hover("checkbox", move |style| style.bg(hover)))
            .child(Text::ui(self.label).tone(Tone::Default))
            .when_some(on_toggle, |el, on_toggle| {
                el.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        on_toggle(!checked, window, cx);
                        cx.stop_propagation();
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caller_owns_the_value() {
        assert!(Checkbox::new("open", "Open after creating", true).is_checked());
        assert!(!Checkbox::new("open", "Open after creating", false).is_checked());
    }
}
