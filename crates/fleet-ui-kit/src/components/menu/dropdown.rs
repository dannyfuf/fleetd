//! `Dropdown` — a field that shows the current value and a chevron, and opens a [`Menu`] of the
//! options with a check on the chosen one.
//!
//! It replaces the `Cycler`'s `‹ value ›` in Settings, Create worktree and the card detail when a
//! set is long enough that stepping through it costs more than looking at it. Use a segmented
//! control for two or three options that fit side by side, and a `FuzzyList` under a text field
//! for a set too long to read.

use gpui::{App, Context, ElementId, SharedString, Window, div, prelude::*};

use std::rc::Rc;

use super::{Menu, MenuBuilder, PopoverMenu};
use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
};

/// A value field that opens a list of options. The caller builds the options as
/// [`super::MenuItem`]s with `.checked(..)` and an `.on_select(..)` (or an action) each.
#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    value: SharedString,
    label: Option<SharedString>,
    menu: Option<MenuBuilder>,
    full_width: bool,
}

impl Dropdown {
    /// A dropdown showing `value`, the current choice's label.
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            label: None,
            menu: None,
            full_width: false,
        }
    }

    /// The field's name, drawn above it and read as its accessible name.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Build the options. Runs each time the list opens.
    pub fn menu(
        mut self,
        builder: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(builder));
        self
    }

    /// Stretch across the container.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
}

impl RenderOnce for Dropdown {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let value = self.value;
        let name = self.label.clone().unwrap_or_else(|| value.clone());
        let full_width = self.full_width;
        let field = move |open: bool, _: &mut Window, cx: &mut App| {
            let theme = cx.theme();
            let colors = &theme.colors;
            let hover = colors.border_strong;
            div()
                .id("dropdown-field")
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .h(theme.metrics.text_field_h)
                .px(theme.space.md)
                .when(full_width, |el| el.w_full())
                .rounded(theme.radii.control)
                .bg(colors.bg)
                .border(theme.metrics.hairline)
                .border_color(if open {
                    colors.focus_ring
                } else {
                    colors.border
                })
                .cursor_pointer()
                .when(!open, |el| el.hover(move |style| style.border_color(hover)))
                .role(gpui::Role::ComboBox)
                .aria_label(name)
                .aria_expanded(open)
                .child(div().flex_1().min_w_0().child(Text::ui(value).ellipsize()))
                .child(
                    Icon::ChevronDown
                        .el()
                        .size(IconSize::Small)
                        .color(colors.text_secondary),
                )
        };
        let popover = PopoverMenu::new(self.id)
            .trigger_with(field)
            .when(full_width, PopoverMenu::full_width);
        let popover = match self.menu {
            Some(builder) => popover.menu_builder(builder),
            None => popover,
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.space.xxs)
            .when(full_width, |el| el.w_full())
            .children(self.label.map(Text::sentence_label))
            .child(popover)
    }
}
