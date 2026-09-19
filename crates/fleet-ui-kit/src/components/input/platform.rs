//! Platform text-input bridge for [`super::TextInput`].

use std::ops::Range;

use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point};

use super::TextInput;

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        self.buffer.text().get(range).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.buffer.selected_range()),
            reversed: self.buffer.caret() < self.buffer.anchor(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.buffer
            .marked_range()
            .map(|range| self.buffer.range_to_utf16(&range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.buffer.unmark();
        self.finish_composition();
        self.line_cache.clear();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let composing = self.composition_group_open || self.buffer.marked_range().is_some();
        let range = self.resolve_range(range_utf16.clone());
        let insertion = self.filtered(text);
        if !composing && !text.is_empty() && insertion.is_empty() {
            return;
        }
        let now = cx.background_executor().now();
        let changed = if range_utf16.is_none() && !composing {
            self.buffer.insert(&insertion, now)
        } else {
            self.buffer.replace_range(range, &insertion, now)
        };
        self.finish_composition();
        self.line_cache.clear();
        if changed {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let composing = self.composition_group_open || self.buffer.marked_range().is_some();
        let insertion = self.filtered(new_text);
        if !composing && !new_text.is_empty() && insertion.is_empty() {
            return;
        }
        if !self.composition_group_open {
            self.buffer.begin_history_group();
            self.composition_group_open = true;
        }
        let range = self.resolve_range(range_utf16);
        let now = cx.background_executor().now();
        let changed = self.buffer.replace_and_mark(range, &insertion, now);
        if let Some(selected) = new_selected_range_utf16 {
            let inserted = self.buffer.marked_range().unwrap_or_else(|| {
                let caret = self.buffer.caret();
                caret..caret
            });
            let insertion = self.buffer.text().get(inserted.clone()).unwrap_or_default();
            let relative_start = byte_offset_from_utf16(insertion, selected.start);
            let relative_end = byte_offset_from_utf16(insertion, selected.end);
            self.buffer
                .set_selected_range(inserted.start + relative_start..inserted.start + relative_end);
        }
        if self.buffer.marked_range().is_none() {
            self.finish_composition();
        }
        self.line_cache.clear();
        if changed {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        let start = self.position_for_offset(range.start)?;
        let end = self.position_for_offset(range.end)?;
        let scroll = self.horizontal_scroll;
        let row_scroll = self.line_height * self.scroll_row as f32;
        Some(Bounds::from_corners(
            point(
                element_bounds.left() + start.x - scroll,
                element_bounds.top() + start.y - row_scroll,
            ),
            point(
                element_bounds.left() + end.x - scroll,
                element_bounds.top() + end.y - row_scroll + self.line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point_in_window: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_point(point_in_window)?;
        Some(self.buffer.offset_to_utf16(offset))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.buffer.range_from_utf16(&range_utf16);
        self.buffer.set_selected_range(range);
        self.reveal_caret = true;
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.buffer.len_utf16())
    }
}

impl TextInput {
    /// Resolve an IME edit range in platform priority order: explicit, marked, selection.
    fn resolve_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        match range_utf16 {
            Some(range) => self.buffer.range_from_utf16(&range),
            None => self
                .buffer
                .marked_range()
                .unwrap_or_else(|| self.buffer.selected_range()),
        }
    }
}

fn byte_offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8 = 0usize;
    let mut utf16 = 0usize;
    for character in text.chars() {
        if utf16 >= offset {
            break;
        }
        utf16 += character.len_utf16();
        utf8 += character.len_utf8();
    }
    utf8
}
