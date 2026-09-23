//! `FilterField` — a page toolbar's filter box: a magnifier, the query or `Filter`, and the key
//! that opens it.
//!
//! The field has two faces in one box, so opening the filter never moves anything:
//!
//! - **Idle.** A control drawn as a field. A click dispatches [`FilterField::action`] (the
//!   list's own open-filter action, the one `/` runs), which puts the surface in filter mode;
//!   the chip shows that key from the live keymap. A retained query (§3.10 stage two of `Esc`)
//!   reads in body text instead of the placeholder, so a filtered list always says why rows are
//!   missing.
//! - **Editing.** The surface hands in its live [`TextInput`] with [`FilterField::editor`]
//!   (built embedded: the field is the chrome), and the box draws it with the `shown/total`
//!   count where the chip was — amber when the query hides every row.
//!
//! Either face can carry a clear `✕` ([`FilterField::on_clear`]) while a query is set, and a
//! surface whose filter is not an action (a board header's own editor) opens it with
//! [`FilterField::on_click`] instead of [`FilterField::action`]. The box is `filter_field_w`
//! wide unless [`FilterField::width`] says otherwise.
//!
//! The editing face owns no keys: the surface's filter mode does, exactly as with
//! [`super::FilterBar`], which stays the in-place form for a dense pane header. Use this one in
//! a [`super::PageHeader`].

use std::rc::Rc;

use gpui::{Action, App, ElementId, Entity, Pixels, SharedString, Window, div, prelude::*};

use super::{
    ButtonSize, IconButton, TextInput, control,
    kbd::{Kbd, KbdSize},
};
use crate::{
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
    tone::Tone,
};

/// A filter box for a page toolbar. See the module doc.
#[derive(IntoElement)]
pub struct FilterField {
    id: ElementId,
    placeholder: SharedString,
    query: Option<SharedString>,
    editor: Option<Entity<TextInput>>,
    counts: Option<(usize, usize)>,
    action: Option<Box<dyn Action>>,
    kbd: Option<Kbd>,
    width: Option<Pixels>,
    on_click: Option<Handler>,
    on_clear: Option<Handler>,
}

/// A press on the field or on its clear button.
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

impl FilterField {
    /// An idle field reading `placeholder` (`Filter`).
    pub fn new(id: impl Into<ElementId>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            placeholder: placeholder.into(),
            query: None,
            editor: None,
            counts: None,
            action: None,
            kbd: None,
            width: None,
            on_click: None,
            on_clear: None,
        }
    }

    /// The retained query to show in place of the placeholder while the field is idle.
    pub fn query(mut self, query: impl Into<SharedString>) -> Self {
        self.query = Some(query.into()).filter(|query: &SharedString| !query.is_empty());
        self
    }

    /// Draw the live editor instead of the idle face: the surface is in filter mode.
    pub fn editor(mut self, editor: Entity<TextInput>) -> Self {
        self.editor = Some(editor);
        self
    }

    /// `shown/total`, drawn while editing.
    pub fn counts(mut self, shown: usize, total: usize) -> Self {
        self.counts = Some((shown, total));
        self
    }

    /// Dispatch `action` to the focused element on click, and show its live key binding.
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(action);
        self
    }

    /// Show this key instead of the one [`Self::action`] resolves.
    pub fn kbd(mut self, kbd: Kbd) -> Self {
        self.kbd = Some(kbd);
        self
    }

    /// Run `handler` when the idle field is clicked, for a surface whose filter is not an action.
    /// With [`Self::action`] as well, the handler runs first.
    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Draw a clear `✕` while a query is set (idle or editing), running `handler` when pressed.
    pub fn on_clear(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_clear = Some(Rc::new(handler));
        self
    }

    /// Override the `filter_field_w` width, e.g. to fill a narrow header.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Whether the field is drawing its editor.
    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// The tone of the editing count: amber when the query hides every row.
    pub fn count_tone(&self) -> Tone {
        match self.counts {
            Some((0, total)) if total > 0 => Tone::Warning,
            _ => Tone::Muted,
        }
    }
}

impl RenderOnce for FilterField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let count_tone = self.count_tone();
        let kbd = self.kbd.or_else(|| {
            self.action
                .as_deref()
                .and_then(|action| Kbd::for_action(action, window, cx))
        });
        let has_query = self.query.is_some()
            || self
                .editor
                .as_ref()
                .is_some_and(|editor| !editor.read(cx).text().is_empty());
        let clear = self.on_clear.clone().filter(|_| has_query).map(|clear| {
            IconButton::new("filter-field-clear", Icon::X, "Clear filter")
                .size(ButtonSize::Compact)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    clear(window, cx);
                })
        });
        let theme = cx.theme();
        let editing = self.editor.is_some();
        let hover = theme.colors.control_hover;
        let icon_color = if editing || self.query.is_some() {
            theme.colors.text_secondary
        } else {
            theme.colors.text_muted
        };
        let body = match self.editor {
            Some(editor) => styled_with(div(), TextRole::Ui.style(theme), theme)
                .flex()
                .flex_1()
                .min_w_0()
                .h(theme.text.ui.line_height)
                .text_color(theme.colors.text)
                .child(editor)
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .child(match self.query.clone() {
                    Some(query) => Text::ui(query).ellipsize(),
                    None => Text::ui(self.placeholder.clone()).muted().ellipsize(),
                })
                .into_any_element(),
        };
        let trailing = if editing {
            self.counts.map(|(shown, total)| {
                Text::label(format!("{shown}/{total}"))
                    .tone(count_tone)
                    .into_any_element()
            })
        } else {
            kbd.clone()
                .map(|kbd| kbd.size(KbdSize::Small).into_any_element())
        };
        let field = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.sm)
            .w(self.width.unwrap_or(theme.metrics.filter_field_w))
            .h(theme.metrics.button_h)
            .px(theme.space.sm)
            .rounded(theme.radii.control)
            .bg(theme.colors.control)
            .border(theme.metrics.hairline)
            .border_color(if editing {
                theme.colors.focus_ring
            } else {
                theme.colors.control_border
            })
            .when(!editing, |el| el.hover(move |style| style.bg(hover)))
            .when_some(kbd.as_ref().and_then(Kbd::aria_shortcut), |el, shortcut| {
                el.aria_keyshortcuts(shortcut)
            })
            .child(Icon::Search.el().size(IconSize::Small).color(icon_color))
            .child(body)
            .children(trailing)
            .children(clear);
        if editing {
            return field.into_any_element();
        }
        let action = self.action;
        let on_click = self.on_click;
        control::on_click_named(field, self.placeholder, move |_, window, cx| {
            if let Some(on_click) = &on_click {
                on_click(window, cx);
            }
            if let Some(action) = &action {
                window.dispatch_action(action.boxed_clone(), cx);
            }
        })
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_that_hides_every_row_turns_the_count_amber() {
        assert_eq!(
            FilterField::new("f", "Filter").counts(0, 4).count_tone(),
            Tone::Warning
        );
        assert_eq!(
            FilterField::new("f", "Filter").counts(2, 4).count_tone(),
            Tone::Muted
        );
        assert_eq!(
            FilterField::new("f", "Filter").counts(0, 0).count_tone(),
            Tone::Muted
        );
    }

    #[test]
    fn an_empty_query_is_no_query() {
        let field = FilterField::new("f", "Filter").query("");
        assert!(field.query.is_none());
        assert!(!field.is_editing());
    }
}
