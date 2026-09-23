//! `PrefixMenu` — the ⌃S command menu: every command the held prefix reaches here, grouped into
//! columns, each one a clickable row led by the key that runs it.
//!
//! The prefix is one-shot: the expert types the second key in under 200 ms and never sees this
//! menu; the person who hesitates gets it exactly then. **The delay is the caller's timer**
//! (`theme.motion.prefix_hint_delay`): a component cannot own a one-shot timer without owning
//! state, and the prefix already lives in the app's mode machine.
//!
//! The menu floats bottom-centre over its container, which must be `relative`, at most
//! `metrics.prefix_menu_w` wide. It never takes focus: the held prefix keeps the keyboard, and a
//! click on a row does what pressing its key would. A row shows only the *second* key, because
//! the prefix is already held (DESIGN-SYSTEM §4).
//!
//! Use a `Menu` instead for a list of commands opened by a click, and a [`super::Tooltip`] to
//! name one control.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, MouseButton, SharedString, Window, div, prelude::*,
};

use super::{
    control,
    kbd::{Kbd, KbdSize, KbdTone},
};
use crate::{harness::HarnessTargetExt, text::Text, theme::ActiveTheme};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The ⌃S command menu.
#[derive(IntoElement)]
pub struct PrefixMenu {
    id: ElementId,
    title: SharedString,
    prefix: Option<Kbd>,
    note: Option<AnyElement>,
    close: Option<AnyElement>,
    columns: Vec<PrefixMenuColumn>,
    target: Option<&'static str>,
}

impl PrefixMenu {
    /// A menu headed `title` ("Fleet commands").
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            prefix: None,
            note: None,
            close: None,
            columns: Vec::new(),
            target: None,
        }
    }

    /// The held prefix, drawn before the title as an amber chip.
    pub fn prefix(mut self, prefix: Kbd) -> Self {
        self.prefix = Some(prefix);
        self
    }

    /// The line after the title: how to use the menu, and what pressing the prefix again does.
    pub fn note(mut self, note: impl IntoElement) -> Self {
        self.note = Some(note.into_any_element());
        self
    }

    /// The control at the header's right edge that cancels the prefix, normally a compact ghost
    /// [`super::Button`] showing `esc`.
    pub fn close(mut self, close: impl IntoElement) -> Self {
        self.close = Some(close.into_any_element());
        self
    }

    /// One column of commands, in the order the columns read left to right.
    pub fn column(mut self, column: PrefixMenuColumn) -> Self {
        self.columns.push(column);
        self
    }

    /// Record the panel's painted bounds under a harness target name.
    pub fn harness(mut self, target: &'static str) -> Self {
        self.target = Some(target);
        self
    }
}

impl RenderOnce for PrefixMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let header = div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .px(theme.space.xs)
            .children(self.prefix.map(|kbd| kbd.tone(KbdTone::Warning)))
            .child(Text::ui_strong(self.title).flex_none())
            .children(self.note)
            .child(div().flex_1())
            .children(self.close);
        let columns = div()
            .flex()
            .gap(theme.space.md)
            .children(self.columns.into_iter().map(|column| {
                // Equal columns, as the headings are peers; a label too long for its share
                // ellipsizes rather than pushing its neighbours around.
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .px(theme.space.sm)
                            .pb(theme.space.xs)
                            .child(Text::sentence_label(column.title).muted()),
                    )
                    .children(column.items)
            }));
        let panel = div()
            .id(self.id)
            // The menu floats over live terminal output and the pane under it: the pointer must
            // reach its rows and nothing beneath them, and a press on it must not start a
            // terminal selection in the area that holds it.
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .gap(theme.space.md)
            .w_full()
            .max_w(theme.metrics.prefix_menu_w)
            .p(theme.space.md)
            .rounded(theme.radii.popover)
            .bg(theme.colors.elevated)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border_strong)
            .shadow(theme.popover_shadow())
            .child(header)
            .child(columns)
            .harness_target_named(self.target);
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(theme.space.md)
            .flex()
            .justify_center()
            .px(theme.space.md)
            .child(panel)
    }
}

/// One heading of the menu and its rows.
pub struct PrefixMenuColumn {
    title: SharedString,
    items: Vec<AnyElement>,
}

impl PrefixMenuColumn {
    /// A column headed `title` ("Tabs").
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
        }
    }

    /// Append a row, normally a [`PrefixMenuItem`].
    pub fn item(mut self, item: impl IntoElement) -> Self {
        self.items.push(item.into_any_element());
        self
    }
}

/// One command of the menu: its key chip, then its label. Clicking it runs the command.
#[derive(IntoElement)]
pub struct PrefixMenuItem {
    id: ElementId,
    kbd: Kbd,
    range_end: Option<Kbd>,
    label: SharedString,
    on_click: Option<ClickHandler>,
}

impl PrefixMenuItem {
    /// A row reading `label` after the chip `kbd` — the key after the prefix, never the prefix.
    pub fn new(id: impl Into<ElementId>, kbd: Kbd, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            kbd,
            range_end: None,
            label: label.into(),
            on_click: None,
        }
    }

    /// Draw the chip as a range, `kbd`–`last`, for one row standing for numbered keys (`1`–`9`).
    pub fn range_end(mut self, last: Kbd) -> Self {
        self.range_end = Some(last);
        self
    }

    /// Run the command when the row is clicked. A row without a handler is drawn but inert.
    pub fn on_click(
        mut self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(on_click));
        self
    }
}

impl RenderOnce for PrefixMenuItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let chip = |kbd: Kbd| kbd.size(KbdSize::Small);
        let keys = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xxs)
            .child(chip(self.kbd))
            .children(self.range_end.map(|last| {
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.xxs)
                    .child(Text::hint("–").faint())
                    .child(chip(last))
            }));
        let hover = theme.colors.control_hover;
        let active = theme.colors.control_active;
        let row = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .w_full()
            .h(theme.metrics.button_h_compact)
            .px(theme.space.sm)
            .rounded(theme.radii.control)
            .child(keys)
            .child(Text::ui(self.label.clone()).ellipsize());
        match self.on_click {
            Some(on_click) => control::on_click_named(
                row.hover(move |style| style.bg(hover))
                    .active(move |style| style.bg(active)),
                self.label,
                on_click,
            ),
            None => row,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_keeps_its_columns_and_a_column_its_rows_in_order() {
        let chip = |key: &str| Kbd::parse(key).unwrap_or_else(|error| panic!("{error}"));
        let menu = PrefixMenu::new("menu", "Fleet commands")
            .column(
                PrefixMenuColumn::new("Tabs")
                    .item(PrefixMenuItem::new("a", chip("c"), "New terminal"))
                    .item(PrefixMenuItem::new("b", chip("x"), "Close tab")),
            )
            .column(PrefixMenuColumn::new("Session"));
        let titles: Vec<_> = menu
            .columns
            .iter()
            .map(|column| column.title.clone())
            .collect();
        assert_eq!(titles, ["Tabs", "Session"]);
        assert_eq!(menu.columns[0].items.len(), 2);
        assert!(menu.columns[1].items.is_empty());
    }
}
