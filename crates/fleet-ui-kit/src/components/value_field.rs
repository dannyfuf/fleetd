//! `ValueField` — a labelled text setting in a settings list: the label, then the value in a field
//! box that takes the rest of the row.
//!
//! The box states the value; it is not an editor. The surface opens a live [`TextInput`] on the
//! row — its `Enter`, or a click through [`ValueField::on_click`] — and hands it back with
//! [`ValueField::editor`], which the field draws inside the same box, so opening and closing the
//! row never moves it. Hand it an embedded editor (`TextInput::set_embedded`): the box is the
//! chrome. An empty value shows the placeholder, which says what empty means
//! (`Harness default`).
//!
//! Use a [`super::NumberField`] for an integer and a bare [`TextInput`] for a form field that is
//! always editable, such as a dialog's name field.

use gpui::{App, ElementId, Entity, MouseButton, Pixels, SharedString, Window, div, prelude::*};

use crate::{components::input::TextInput, text::Text, theme::ActiveTheme};

type ValueFieldClick = dyn Fn(&mut Window, &mut App);

/// A read-only text value in a settings row, with the row's editor slotted in while it is open.
#[derive(IntoElement)]
pub struct ValueField {
    id: ElementId,
    label: Option<SharedString>,
    value: SharedString,
    placeholder: Option<SharedString>,
    mono: bool,
    focused: bool,
    label_width: Option<Pixels>,
    editor: Option<Entity<TextInput>>,
    on_click: Option<Box<ValueFieldClick>>,
}

impl ValueField {
    /// A field showing `value`.
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: None,
            value: value.into(),
            placeholder: None,
            mono: false,
            focused: false,
            label_width: None,
            editor: None,
            on_click: None,
        }
    }

    /// Set the label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// What an empty value means, drawn faint in the box.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Draw the value in the data face: a command, a path, an identifier.
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Whether the row carries the settings cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Fix the label column so a stack of settings rows aligns on one gutter.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// The live editor the row was opened with, embedded; drawn inside the box in the value's
    /// place, with the focus-ring border.
    pub fn editor(mut self, editor: Entity<TextInput>) -> Self {
        self.editor = Some(editor);
        self
    }

    /// A click on the box. Point it at what the row's `Enter` does: open the editor.
    pub fn on_click(mut self, on_click: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(on_click));
        self
    }

    /// Whether the row's editor is open.
    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// What the box reads: the value, or the placeholder when the value is empty.
    pub fn shown(&self) -> (SharedString, bool) {
        match (&self.placeholder, self.value.is_empty()) {
            (Some(placeholder), true) => (placeholder.clone(), true),
            _ => (self.value.clone(), false),
        }
    }
}

impl RenderOnce for ValueField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (shown, is_placeholder) = self.shown();
        let focused = self.focused;
        let name = self.label.clone().unwrap_or_else(|| shown.clone());

        let editing = self.editor.is_some();
        let hover_border = theme.colors.border_strong;
        let border = if editing {
            theme.colors.focus_ring
        } else if focused {
            theme.colors.border_strong
        } else {
            theme.colors.border
        };
        let field = div()
            .id(self.id)
            .flex()
            .items_center()
            .w_full()
            .h(theme.metrics.button_h_compact)
            .px(theme.space.sm)
            .rounded(theme.radii.sm)
            .bg(theme.colors.bg)
            .border(theme.metrics.hairline)
            .border_color(border)
            .overflow_hidden();
        let field = match self.editor {
            // The editor is embedded (no chrome of its own), so the box around it is this one
            // and the row keeps its height.
            Some(editor) => field.child(div().flex_1().min_w_0().child(editor)),
            None => {
                let text = if self.mono && !is_placeholder {
                    Text::data(shown)
                } else {
                    Text::ui(shown)
                };
                let text = if is_placeholder { text.faint() } else { text };
                field
                    .role(gpui::Role::TextInput)
                    .aria_label(name)
                    .child(text.ellipsize())
                    .when_some(self.on_click, |el, on_click| {
                        el.cursor_text()
                            .hover(move |style| style.border_color(hover_border))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, window, cx| {
                                on_click(window, cx);
                                cx.stop_propagation();
                            })
                    })
            }
        };
        let slot = div().flex().flex_1().min_w_0().child(field);

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .h_full()
            .w_full()
            .px(theme.space.md)
            .children(self.label.map(|label| {
                let text = Text::ui(label).flex_none();
                match self.label_width {
                    Some(width) => text.w(width),
                    None => text,
                }
            }))
            .child(slot);

        super::control::cursor_row(theme, focused, false, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_value_shows_its_placeholder_and_a_set_one_shows_itself() {
        let empty = ValueField::new("model", "").placeholder("Harness default");
        assert_eq!(empty.shown(), ("Harness default".into(), true));
        let set = ValueField::new("model", "opus").placeholder("Harness default");
        assert_eq!(set.shown(), ("opus".into(), false));
        assert_eq!(ValueField::new("command", "").shown(), ("".into(), false));
    }

    #[test]
    fn a_field_without_an_editor_is_not_editing() {
        assert!(!ValueField::new("command", "claude").is_editing());
    }
}
