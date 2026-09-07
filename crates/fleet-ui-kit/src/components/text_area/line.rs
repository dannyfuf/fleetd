use gpui::{Div, ScrollHandle, SharedString, div, prelude::*};

use crate::{text::TextRole, theme::Theme};

/// One wrap chunk of a line: a word plus the spaces that follow it, so a flex-wrapped row
/// breaks where a text renderer would.
pub(super) fn wrap_chunks(line: &str) -> impl Iterator<Item = &str> {
    line.split_inclusive(' ')
}

/// A chunk of a line. `flex_none` + `whitespace_nowrap` is what makes the parent's `flex_wrap`
/// behave like word wrapping instead of shrinking every word.
pub(super) fn chunk(text: &str) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .child(SharedString::from(text.to_string()))
}

/// Scrolls `handle` the minimum amount that puts the caret inside the visible box.
///
/// A logical line that fits the viewport is handed to gpui, which measures the row it actually
/// laid out — the only thing that knows how many visual rows the wrapping made of it. A line
/// taller than the whole box has no row gpui can reveal, so the caret's position inside it is
/// estimated from how far along the line the caret sits, which is exact for the uniformly
/// wrapped paragraph that produces this case.
pub(super) fn reveal_caret(
    handle: &ScrollHandle,
    row: usize,
    progress: f32,
    line_height: gpui::Pixels,
) {
    let container = handle.bounds();
    let Some(child) = handle.bounds_for_item(row) else {
        handle.scroll_to_item(row);
        return;
    };
    if child.size.height <= container.size.height {
        handle.scroll_to_item(row);
        return;
    }
    let mut offset = handle.offset();
    let top = child.top() + child.size.height * progress;
    let bottom = top + line_height;
    if top + offset.y < container.top() {
        offset.y = container.top() - top;
    } else if bottom + offset.y > container.bottom() {
        offset.y = container.bottom() - bottom;
    }
    handle.set_offset(offset);
}

/// The 2 px accent bar that marks the insertion point — the same caret
/// [`crate::components::TextField`]
/// draws, at the line height of the role.
pub(super) fn caret_bar(theme: &Theme, role: TextRole) -> Div {
    div()
        .flex_none()
        .w(theme.metrics.focus_ring_w)
        .h(role.style(theme).line_height)
        .bg(theme.colors.accent)
}

/// One rendered line: its chunks, with the caret spliced in when it lands on this line.
///
/// `min_h` is the *floor* for an empty line, not the height: an explicit minimum replaces the
/// content-derived automatic minimum a column flex item would otherwise get, so a row that wrapped
/// to three visual rows would let the column shrink it back to one and paint its tail over the next
/// line. `flex_shrink_0` is what keeps a wrapped line as tall as the wrapping made it; the box
/// scrolls or clips instead of squeezing its lines.
pub(super) fn line_row(theme: &Theme, role: TextRole, line: &str, caret: Option<usize>) -> Div {
    let line_height = role.style(theme).line_height;
    let row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .flex_shrink_0()
        .items_center()
        .w_full()
        .min_h(line_height);
    match caret {
        Some(offset) => {
            let (head, tail) = line.split_at(offset);
            row.children(wrap_chunks(head).map(chunk))
                .child(caret_bar(theme, role))
                .children(wrap_chunks(tail).map(chunk))
        }
        None => row.children(wrap_chunks(line).map(chunk)),
    }
}
