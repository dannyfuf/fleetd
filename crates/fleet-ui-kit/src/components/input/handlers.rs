//! The action handlers of [`TextInput`]: one `text_input::*` action in, one buffer call out.
//!
//! Every handler here is bound by the keymap and does nothing but name the buffer operation it
//! stands for, so the editing vocabulary can be read off one screen. A handler that needs more
//! than that — `newline`'s propagation, the clipboard pair, undo's composition flush — says why
//! inline.

use gpui::{ClipboardItem, Context, Window};

use super::actions::*;
use super::{InputBuffer, InputMode, TextInput};

impl TextInput {
    pub(super) fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_left(false));
    }

    pub(super) fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_right(false));
    }

    pub(super) fn move_word_left(
        &mut self,
        _: &MoveWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_word_left(false));
    }

    pub(super) fn move_word_right(
        &mut self,
        _: &MoveWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_word_right(false));
    }

    pub(super) fn move_line_start(
        &mut self,
        _: &MoveToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_to_line_start(false));
    }

    pub(super) fn move_line_end(
        &mut self,
        _: &MoveToLineEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_to_line_end(false));
    }

    pub(super) fn move_row_start(
        &mut self,
        _: &MoveToRowStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to_visual_row_boundary(false, false, cx);
    }

    pub(super) fn move_row_end(
        &mut self,
        _: &MoveToRowEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to_visual_row_boundary(true, false, cx);
    }

    pub(super) fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if !self.move_vertical(false, false, cx) {
            cx.propagate();
        }
    }

    pub(super) fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        if !self.move_vertical(true, false, cx) {
            cx.propagate();
        }
    }

    pub(super) fn move_start(&mut self, _: &MoveToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_document_start(false));
    }

    pub(super) fn move_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_document_end(false));
    }

    pub(super) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_left(true));
    }

    pub(super) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_right(true));
    }

    pub(super) fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_word_left(true));
    }

    pub(super) fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_word_right(true));
    }

    pub(super) fn select_line_start(
        &mut self,
        _: &SelectToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_to_line_start(true));
    }

    pub(super) fn select_line_end(
        &mut self,
        _: &SelectToLineEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_to_line_end(true));
    }

    pub(super) fn select_row_start(
        &mut self,
        _: &SelectToRowStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to_visual_row_boundary(false, true, cx);
    }

    pub(super) fn select_row_end(
        &mut self,
        _: &SelectToRowEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to_visual_row_boundary(true, true, cx);
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(false, true, cx);
    }

    pub(super) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(true, true, cx);
    }

    pub(super) fn select_start(
        &mut self,
        _: &SelectToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(cx, |buffer| buffer.move_to_document_start(true));
    }

    pub(super) fn select_end(&mut self, _: &SelectToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_document_end(true));
    }

    pub(super) fn select_all_action(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_all(cx);
    }

    pub(super) fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, InputBuffer::backspace);
    }

    pub(super) fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, InputBuffer::delete_forward);
    }

    pub(super) fn delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, InputBuffer::delete_word_backward);
    }

    pub(super) fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, InputBuffer::delete_word_forward);
    }

    pub(super) fn delete_line_start(
        &mut self,
        _: &DeleteToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, InputBuffer::delete_to_line_start);
    }

    pub(super) fn delete_line_end(
        &mut self,
        _: &DeleteToLineEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, InputBuffer::delete_to_line_end);
    }

    pub(super) fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode(), InputMode::SingleLine) {
            cx.propagate();
            return;
        }
        let accepted = self.filter.is_none_or(|filter| filter('\n'));
        if accepted {
            self.edit(cx, InputBuffer::insert_newline);
        }
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    pub(super) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || !self.copy_selection(cx) {
            return;
        }
        self.edit(cx, |buffer, now| buffer.insert("", now));
    }

    pub(super) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let text = self.filtered(&text);
        if !text.is_empty() {
            self.edit(cx, |buffer, now| {
                buffer.replace_range(buffer.selected_range(), &text, now)
            });
        }
    }

    pub(super) fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.finish_composition();
        if self.buffer.undo() {
            self.changed(cx);
        }
    }

    pub(super) fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.finish_composition();
        if self.buffer.redo() {
            self.changed(cx);
        }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        let selected = self.buffer.selected_text();
        if selected.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
        true
    }
}
