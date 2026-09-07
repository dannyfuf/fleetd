//! The overlay stack: confirmations, prompts, option menus and the generated help.
//!
//! An overlay owns the whole key-context chain while it is open (see
//! [`crate::state::GitUiState::context_chain`]), which is lazygit's "a prompt swallows the
//! keymap" rule.

use fleet_ui_kit::prelude::*;
use fleet_ui_kit::{
    ConfirmDialog, Dialog, Fact, FactList, FuzzyItem, FuzzyList, KeyHintRow, TextField,
};
use gpui::{AnyElement, Context, div};

use crate::keymap;
use crate::root::Lazygit;
use crate::state::{Buffer, Overlay};

/// How many help rows fit on one page.
const HELP_ROWS: usize = 18;

/// The chain the panels *would* have while the help overlay owns the real one.
pub(crate) fn help_chain(view: &Lazygit) -> Vec<&'static str> {
    view.help_context
        .clone()
        .unwrap_or_else(|| view.state.context_chain())
}

/// The largest scroll offset the `?` overlay may take: the last row stays on screen.
///
/// Without this clamp `j` scrolls forever past the end of the table and leaves the dialog blank.
#[must_use]
pub(crate) fn help_last_top(view: &Lazygit) -> usize {
    keymap::bindings_for_chain(&help_chain(view))
        .len()
        .saturating_sub(HELP_ROWS)
}

/// Renders the top overlay, if any.
pub(crate) fn render(view: &Lazygit, cx: &mut Context<Lazygit>) -> Option<AnyElement> {
    match view.state.overlay()? {
        Overlay::Confirm(confirm) => {
            let facts = FactList::from_facts(confirm.facts.iter().map(|fact| {
                if confirm.danger {
                    Fact::risk(fact.clone())
                } else {
                    Fact::safe(fact.clone())
                }
            }));
            let mut dialog = ConfirmDialog::new(confirm.title.clone(), facts)
                .target(confirm.target.clone())
                .action_label(confirm.title.clone())
                .hints(KeyHintRow::new().key("y", "yes"));
            if confirm.danger {
                dialog = dialog.icon(Icon::TriangleAlert);
            } else {
                dialog = dialog.icon(Icon::CircleDot);
            }
            Some(dialog.into_any_element())
        }
        Overlay::Prompt(prompt) => {
            let body = if prompt.buffer.is_multiline() {
                multiline_body(&prompt.buffer, &view.scroll_editor, cx)
            } else if let Some(input) = &view.prompt_input {
                input.clone().into_any_element()
            } else {
                TextField::new(prompt.buffer.value().to_owned())
                    .caret(prompt.buffer.caret())
                    .focused(true)
                    .mono(true)
                    .hide_status_line(true)
                    .into_any_element()
            };
            let hints = if prompt.buffer.is_multiline() {
                KeyHintRow::new()
                    .key("\u{2318}\u{23ce}", "confirm")
                    .key("\u{23ce}", "new line")
                    .key("esc", "cancel")
            } else {
                KeyHintRow::new()
                    .key("\u{23ce}", "confirm")
                    .key("esc", "cancel")
            };
            let mut dialog = Dialog::new(prompt.title.clone())
                .icon(Icon::FilePen)
                .body(body)
                .hint_row(hints)
                .primary(if prompt.buffer.is_multiline() {
                    "\u{2318}\u{23ce} Confirm"
                } else {
                    "\u{23ce} Confirm"
                });
            if let Some(subtitle) = &prompt.subtitle {
                dialog = dialog.subtitle(subtitle.clone());
            }
            Some(dialog.into_any_element())
        }
        Overlay::Menu(menu) => {
            let visible = menu.visible();
            let items = visible
                .iter()
                .map(|(_, item)| FuzzyItem::new(item.label.clone()).key(item.key.clone()));
            let list = FuzzyList::new(items)
                .cursor(menu.cursor)
                .cap(12)
                .under_text_field(menu.filter.is_some())
                .empty(Text::ui("Nothing matches.").muted());
            let mut body = div().flex().flex_col().gap(cx.theme().space.sm);
            if let Some(filter) = &menu.filter {
                body = body.child(
                    TextField::new(filter.value().to_owned())
                        .caret(filter.caret())
                        .focused(true)
                        .placeholder("filter")
                        .hide_status_line(true),
                );
            }
            let body = body.child(list);
            let hints = if menu.filter.is_some() {
                KeyHintRow::new()
                    .key("\u{23ce}", "run")
                    .key("^n/^p", "move")
                    .key("esc", "clear filter")
            } else {
                KeyHintRow::new()
                    .key("\u{23ce}", "run")
                    .key("j/k", "move")
                    .key("/", "filter")
                    .key("esc", "cancel")
            };
            Some(
                Dialog::new(menu.title.clone())
                    .icon(Icon::Ellipsis)
                    .body(body)
                    .hint_row(hints)
                    .primary("\u{23ce} Run")
                    .into_any_element(),
            )
        }
        Overlay::Help { top } => Some(help_dialog(view, *top, cx)),
    }
}

/// A multi-line editor: one row per line, with a caret bar in the active line.
fn multiline_body(
    buffer: &Buffer,
    scroll: &gpui::UniformListScrollHandle,
    cx: &mut Context<Lazygit>,
) -> AnyElement {
    let theme = cx.theme();
    let (lines, caret_line, caret_column) = buffer.lines_with_caret();
    let list = gpui::uniform_list(
        "lazygit-prompt-lines",
        lines.len(),
        move |visible, _, cx| {
            let theme = cx.theme();
            visible
                .map(|index| {
                    let line = &lines[index];
                    let mut element = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(theme.metrics.diff_row_h);
                    if index == caret_line {
                        let at = line
                            .char_indices()
                            .nth(caret_column)
                            .map_or(line.len(), |(at, _)| at);
                        element = element
                            .child(Text::data(gpui::SharedString::new(&line[..at])).flex_none())
                            .child(
                                div()
                                    .w(theme.metrics.focus_ring_w)
                                    .h(theme.metrics.diff_caret_h)
                                    .flex_none()
                                    .bg(theme.colors.accent),
                            )
                            .child(Text::data(gpui::SharedString::new(&line[at..])).flex_none());
                    } else {
                        element = element.child(Text::data(line.clone()).flex_none());
                    }
                    element.into_any_element()
                })
                .collect()
        },
    )
    .size_full()
    .track_scroll(scroll);
    div()
        .w_full()
        .h(theme.metrics.editor_box_h)
        .p(theme.space.sm)
        .rounded(theme.radii.sm)
        .bg(theme.colors.bg)
        .border_1()
        .border_color(theme.colors.focus_ring)
        .overflow_hidden()
        .child(list)
        .into_any_element()
}

/// The `?` overlay, generated from the binding table so keys and documentation cannot drift.
fn help_dialog(view: &Lazygit, top: usize, cx: &mut Context<Lazygit>) -> AnyElement {
    let theme = cx.theme();
    let rows = keymap::bindings_for_chain(&help_chain(view));
    let total = rows.len();
    let start = top.min(total.saturating_sub(HELP_ROWS));
    let mut column = div().flex().flex_col().w_full().gap(theme.space.xxs).child(
        Text::ui(format!(
            "{} bindings active here — j/k scrolls, esc closes",
            total
        ))
        .muted(),
    );
    for spec in rows.iter().skip(start).take(HELP_ROWS) {
        column = column.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.space.md)
                .child(
                    Text::hint(keymap::pretty_keys(spec.keys))
                        .w_ch(12.0)
                        .flex_none(),
                )
                .child(Text::ui(keymap::humanize(spec.action)).flex_none())
                .child(
                    Text::hint(
                        spec.context
                            .rsplit('>')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_owned(),
                    )
                    .faint()
                    .flex_none(),
                ),
        );
    }
    Dialog::new("Keybindings")
        .icon(Icon::Command)
        .width(theme.metrics.overlay_help_w)
        .body(column)
        .hint_row(KeyHintRow::new().key("j/k", "scroll").key("esc", "close"))
        .primary("esc Close")
        .into_any_element()
}
