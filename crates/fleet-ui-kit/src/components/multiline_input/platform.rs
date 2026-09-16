//! Platform text-input bridge for [`super::MultilineInput`].

use std::ops::Range;

use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point};

use super::{MultilineInput, MultilineInputEvent, buffer::offset_from_utf16};

impl EntityInputHandler for MultilineInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        self.buffer.text().get(range).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.buffer.selected_range()),
            reversed: self.buffer.cursor() < self.buffer.anchor(),
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
        self.sync_view(cx);
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
        let range = self.resolve_range(range_utf16);
        let trigger = self.buffer.trigger_for(range.start, text);
        self.buffer.replace_range(range, text);
        self.history.reset();
        self.goal_x = None;
        self.sync_text(cx);
        // The character is already in the buffer: the trigger is a report, not a consumption.
        cx.emit(MultilineInputEvent::Changed);
        if let Some(trigger) = trigger {
            cx.emit(MultilineInputEvent::Trigger(trigger));
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
        let range = self.resolve_range(range_utf16);
        self.buffer.replace_and_mark(range, new_text);
        if let Some(selected) = new_selected_range_utf16 {
            let inserted = self.buffer.marked_range().unwrap_or_else(|| {
                let cursor = self.buffer.cursor();
                cursor..cursor
            });
            let insertion = self.buffer.text().get(inserted.clone()).unwrap_or("");
            let relative_start = offset_from_utf16(insertion, selected.start);
            let relative_end = offset_from_utf16(insertion, selected.end);
            self.buffer
                .set_selected_range(inserted.start + relative_start..inserted.start + relative_end);
        }
        self.history.reset();
        self.goal_x = None;
        self.sync_text(cx);
        cx.emit(MultilineInputEvent::Changed);
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
        let origin = point(element_bounds.left(), element_bounds.top() - self.scroll);
        Some(Bounds::from_corners(
            point(origin.x + start.x, origin.y + start.y),
            point(origin.x + end.x, origin.y + end.y + self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point_in_window: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let position = point(
            point_in_window.x - bounds.left(),
            point_in_window.y - bounds.top() + self.scroll,
        );
        let offset = self.offset_for_position(position)?;
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
        self.sync_view(cx);
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.buffer.len_utf16())
    }
}
