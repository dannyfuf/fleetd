//! `Toggle` — a labelled settings row with a [`Switch`] at its end, toggled with `Space`.
//!
//! The row is the control: it carries the cursor band and the key, exactly as it did when it
//! drew `[x]` / `[ ]`, and the switch only states the value at a glance and takes a click when
//! the caller wires [`Toggle::on_toggle`]. Use a bare [`Switch`] only outside a settings list.

use gpui::{App, ElementId, Pixels, SharedString, Window, div, prelude::*};

use super::switch::Switch;
use crate::{text::Text, theme::ActiveTheme, tone::Tone};

type ToggleHandler = dyn Fn(bool, &mut Window, &mut App);

/// A boolean settings row.
#[derive(IntoElement)]
pub struct Toggle {
    id: Option<ElementId>,
    label: Option<SharedString>,
    checked: bool,
    focused: bool,
    disabled: bool,
    detail: Option<SharedString>,
    label_width: Option<Pixels>,
    on_toggle: Option<Box<ToggleHandler>>,
}

impl Toggle {
    /// A toggle.
    pub fn new(checked: bool) -> Self {
        Self {
            id: None,
            label: None,
            checked,
            focused: false,
            disabled: false,
            detail: None,
            label_width: None,
            on_toggle: None,
        }
    }

    /// A labelled toggle.
    pub fn labeled(label: impl Into<SharedString>, checked: bool) -> Self {
        Self::new(checked).label(label)
    }

    /// Set the label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The switch's element id. Defaults to the label, which is unique within a settings list;
    /// set it when two toggles share a label under one parent.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Fix the label column so a stack of settings rows aligns on one gutter.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// A trailing muted detail, e.g. a live match count.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Whether the row carries the settings cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether the toggle can be changed here.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A click on the switch, with the value it asks for. The row's `Space` stays the
    /// keyboard's way; point this at the same update.
    pub fn on_toggle(mut self, on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(on_toggle));
        self
    }

    /// Whether the switch reads as on.
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// The tone of the label: a disabled row never reaches full contrast.
    fn label_tone(&self) -> Tone {
        if self.disabled {
            Tone::Muted
        } else {
            Tone::Default
        }
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let label_tone = self.label_tone();
        let disabled = self.disabled;
        let focused = self.focused && !disabled;
        let id = self
            .id
            .or_else(|| self.label.clone().map(ElementId::Name))
            .unwrap_or_else(|| ElementId::Name(SharedString::new_static("toggle")));
        let mut switch = Switch::new(id, self.checked).disabled(disabled);
        if let Some(label) = self.label.clone() {
            switch = switch.name(label);
        }
        if let Some(on_toggle) = self.on_toggle {
            switch = switch.on_toggle(on_toggle);
        }

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .h_full()
            .w_full()
            .px(theme.space.md)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.space.sm)
                    .children(self.label.map(|label| {
                        let text = Text::ui(label).tone(label_tone);
                        match self.label_width {
                            Some(width) => text.w(width).flex_none(),
                            None => text.ellipsize(),
                        }
                    }))
                    .children(
                        self.detail
                            .map(|detail| Text::caption(detail).faint().ellipsize()),
                    ),
            )
            .child(switch);

        super::control::cursor_row(theme, focused, disabled, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_toggle_never_reaches_full_contrast() {
        assert_eq!(Toggle::new(true).disabled(true).label_tone(), Tone::Muted);
        assert_eq!(Toggle::new(true).label_tone(), Tone::Default);
        assert!(!Toggle::new(false).is_checked());
    }
}
