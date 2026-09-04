//! `Select` — a labelled value that opens a [`super::FuzzyList`] of options.
//!
//! Fleet prefers a [`super::Cycler`] for two-to-five options and a [`super::FuzzyList`] for a
//! searchable set; `Select` is the closed-list case in between (the Create dialog's base list,
//! the Assign dialog's context list).
//!
//! **Minimal render.** The open state renders the option list inline; the caller owns
//! `open`, the cursor and the option data.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
};

/// A closed-list chooser.
#[derive(IntoElement)]
pub struct Select {
    label: Option<SharedString>,
    value: SharedString,
    placeholder: Option<SharedString>,
    open: bool,
    focused: bool,
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

    /// The option list, normally a [`super::FuzzyList`].
    pub fn options(mut self, options: impl IntoElement) -> Self {
        self.options = Some(options.into_any_element());
        self
    }
}

impl RenderOnce for Select {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let empty = self.value.is_empty();
        let shown = if empty {
            self.placeholder.clone().unwrap_or_default()
        } else {
            self.value.clone()
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
                    .h(theme.metrics.row_h)
                    .px(theme.space.md)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border_1()
                    .border_color(if self.focused {
                        theme.colors.focus_ring
                    } else {
                        theme.colors.border
                    })
                    .child(if empty {
                        Text::ui(shown).faint()
                    } else {
                        Text::ui(shown)
                    })
                    .child(
                        Icon::ChevronRight
                            .el()
                            .size(IconSize::Small)
                            .color(theme.colors.text_muted),
                    ),
            )
            .when(self.open, |el| {
                el.child(
                    div()
                        .w_full()
                        .rounded(theme.radii.sm)
                        .border_1()
                        .border_color(theme.colors.border)
                        .overflow_hidden()
                        .children(self.options),
                )
            })
    }
}
