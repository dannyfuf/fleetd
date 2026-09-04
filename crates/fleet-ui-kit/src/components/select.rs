//! `Select` — a labelled value that opens a [`super::FuzzyList`] of options.
//!
//! Fleet prefers a [`super::Cycler`] for two-to-five options and a [`super::FuzzyList`] under a
//! [`super::TextField`] for a searchable set; `Select` is the closed-list case in between (the
//! Create dialog's base list, the Assign dialog's context list).
//!
//! The caller owns `open`, the cursor and the option data. The open list is a **popover on the
//! same surface**, not a floating menu: it pushes the dialog's own content down rather than
//! covering it, because a dialog that reflows under a menu is how a user loses the field they
//! were editing.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{
    components::{FocusRing, KeyHint},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A closed-list chooser.
#[derive(IntoElement)]
pub struct Select {
    label: Option<SharedString>,
    value: SharedString,
    placeholder: Option<SharedString>,
    open: bool,
    focused: bool,
    disabled: bool,
    invalid: Option<SharedString>,
    hint: Option<SharedString>,
    options: Option<AnyElement>,
}

impl Select {
    /// A select showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            label: None,
            value: value.into(),
            placeholder: None,
            open: false,
            focused: false,
            disabled: false,
            invalid: None,
            hint: None,
            options: None,
        }
    }

    /// Set the label above the control.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Text shown when nothing is chosen.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Whether the option list is showing.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Whether the control has focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether the choice can be changed here.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The exact failing rule, e.g. `that base no longer exists on origin`.
    pub fn invalid(mut self, message: impl Into<SharedString>) -> Self {
        self.invalid = Some(message.into());
        self
    }

    /// The key that opens the list, shown on the right while the control is focused and closed.
    pub fn hint(mut self, keys: impl Into<SharedString>) -> Self {
        self.hint = Some(keys.into());
        self
    }

    /// The option list, normally a [`super::FuzzyList`].
    pub fn options(mut self, options: impl IntoElement) -> Self {
        self.options = Some(options.into_any_element());
        self
    }

    /// Whether the control is currently rejecting its value.
    pub fn is_invalid(&self) -> bool {
        self.invalid.is_some()
    }
}

impl RenderOnce for Select {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let empty = self.value.is_empty();
        let disabled = self.disabled;
        let focused = self.focused && !disabled;
        let shown = if empty {
            self.placeholder.clone().unwrap_or_default()
        } else {
            self.value.clone()
        };
        let border = if self.invalid.is_some() {
            theme.colors.danger
        } else if focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(self.label.map(Text::label))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space.sm)
                    .h(theme.metrics.row_h)
                    .w_full()
                    .px(theme.space.md)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border_1()
                    .border_color(border)
                    .when(disabled, |el| el.opacity(0.4))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .child(if empty {
                                Text::ui(shown).faint().ellipsize()
                            } else {
                                Text::ui(shown).tone(Tone::Default).ellipsize()
                            }),
                    )
                    .children(
                        self.hint
                            .filter(|_| focused && !self.open)
                            .map(KeyHint::new),
                    )
                    .child(
                        // One glyph says which way the list will grow: closed points along the
                        // row, open points up at the value it belongs to.
                        if self.open {
                            Icon::ChevronsUp
                        } else {
                            Icon::ChevronRight
                        }
                        .el()
                        .size(IconSize::Small)
                        .color(if focused {
                            theme.colors.text_secondary
                        } else {
                            theme.colors.text_muted
                        }),
                    ),
            )
            .when(self.open && !disabled, |el| {
                el.child(
                    div()
                        .w_full()
                        .rounded(theme.radii.sm)
                        .bg(theme.colors.elevated)
                        .border_1()
                        .border_color(theme.colors.border_strong)
                        .shadow(theme.sheet_shadow())
                        .overflow_hidden()
                        .child(
                            FocusRing::pane(focused)
                                .child(div().flex().flex_col().w_full().children(self.options)),
                        ),
                )
            })
            .children(self.invalid.map(|message| {
                div()
                    .flex()
                    .items_center()
                    .h(theme.metrics.field_status_h)
                    .child(Text::hint(message).tone(Tone::Danger).ellipsize())
            }))
    }
}
