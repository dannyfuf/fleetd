//! Mouse and wheel input for [`TextInput`]: click places, drag extends, the wheel scrolls.

use gpui::{Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, Window};

use super::{InputMode, TextInput};

impl TextInput {
    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let Some(offset) = self.offset_for_point(event.position) else {
            return;
        };
        self.finish_composition();
        if event.click_count >= 3 {
            self.buffer.select_line_at(offset);
            self.drag_anchor = None;
        } else if event.click_count == 2 {
            self.buffer.select_word_at(offset);
            self.drag_anchor = None;
        } else if event.modifiers.shift {
            self.buffer.move_to(offset, true);
            self.drag_anchor = Some(self.buffer.anchor());
        } else {
            self.buffer.set_caret(offset);
            self.drag_anchor = Some(offset);
        }
        self.view_changed(cx);
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }
        let (Some(anchor), Some(offset)) =
            (self.drag_anchor, self.offset_for_point(event.position))
        else {
            return;
        };
        let before = self.buffer.selected_range();
        self.buffer.set_selected_range(anchor..offset);
        if self.buffer.selected_range() != before {
            self.view_changed(cx);
        }
    }

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag_anchor = None;
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InputMode::Multiline { max_rows, .. } = self.mode() else {
            return;
        };
        let visual_rows = self.line_cache.visual_rows().max(1);
        let visible = visual_rows.min(max_rows.max(1));
        let max_scroll = visual_rows.saturating_sub(visible);
        if max_scroll == 0 {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height()).y;
        let rows = (f32::from(delta.abs()) / f32::from(window.line_height())).ceil() as usize;
        let next = if delta > gpui::Pixels::ZERO {
            self.scroll_row.saturating_sub(rows.max(1))
        } else {
            self.scroll_row.saturating_add(rows.max(1)).min(max_scroll)
        };
        if next != self.scroll_row {
            self.scroll_row = next;
            self.reveal_caret = false;
            cx.notify();
        }
        cx.stop_propagation();
    }
}
