use gpui::{Keystroke, prelude::*};

use super::{EditEffect, TextFieldState, TextInput, char_offset};

use crate::theme::Theme;

fn state(text: &str, caret_chars: usize) -> TextFieldState {
    let mut state = TextFieldState::from_text(text);
    let offset = text
        .char_indices()
        .nth(caret_chars)
        .map_or(text.len(), |(index, _)| index);
    state.set_cursor(offset);
    state
}

#[test]
fn insert_puts_the_caret_after_the_insertion() {
    let mut s = TextFieldState::new();
    s.insert("feat/");
    s.insert("rut");
    assert_eq!(s.text(), "feat/rut");
    assert_eq!(s.caret_chars(), 8);
}

#[test]
fn insert_strips_newlines() {
    let mut s = TextFieldState::new();
    s.insert("one\ntwo\r\tthree");
    assert_eq!(s.text(), "onetwothree");
}

#[test]
fn backspace_walks_char_boundaries() {
    let mut s = state("héllo", 2);
    assert!(s.backspace());
    assert_eq!(s.text(), "hllo");
    assert_eq!(s.caret_chars(), 1);
}

#[test]
fn backspace_at_the_start_is_a_no_op() {
    let mut s = state("x", 0);
    assert!(!s.backspace());
    assert_eq!(s.text(), "x");
}

#[test]
fn delete_forward_removes_the_char_after_the_caret() {
    let mut s = state("abc", 1);
    assert!(s.delete_forward());
    assert_eq!(s.text(), "ac");
    assert_eq!(s.caret_chars(), 1);
}

#[test]
fn ctrl_w_deletes_one_word_and_its_trailing_space() {
    let mut s = TextFieldState::from_text("origin/main feature ");
    assert!(s.delete_word_before());
    assert_eq!(s.text(), "origin/main ");
    assert!(s.delete_word_before());
    assert_eq!(s.text(), "");
    assert!(!s.delete_word_before());
}

#[test]
fn ctrl_u_and_ctrl_k_cut_around_the_caret() {
    let mut s = state("abcdef", 3);
    assert!(s.delete_to_end());
    assert_eq!(s.text(), "abc");
    assert!(s.delete_to_start());
    assert_eq!(s.text(), "");
}

#[test]
fn ctrl_a_and_ctrl_e_park_the_caret() {
    let mut s = state("abc", 1);
    assert!(s.move_to_start());
    assert_eq!(s.caret_chars(), 0);
    assert!(!s.move_to_start());
    assert!(s.move_to_end());
    assert_eq!(s.caret_chars(), 3);
}

#[test]
fn arrows_stop_at_the_ends() {
    let mut s = state("ab", 0);
    assert!(!s.move_left());
    assert!(s.move_right());
    assert!(s.move_right());
    assert!(!s.move_right());
    assert_eq!(s.caret_chars(), 2);
}

#[test]
fn set_cursor_snaps_to_a_char_boundary() {
    let mut s = TextFieldState::from_text("é");
    s.set_cursor(1);
    assert_eq!(s.cursor(), 0);
    s.set_cursor(99);
    assert_eq!(s.cursor(), 2);
}

#[test]
fn utf16_offsets_round_trip() {
    let s = TextFieldState::from_text("a😀b");
    assert_eq!(s.len_utf16(), 4);
    assert_eq!(s.offset_to_utf16(s.text().len()), 4);
    assert_eq!(s.offset_from_utf16(3), 5);
}

#[test]
fn replace_and_mark_tracks_the_composing_range() {
    let mut s = TextFieldState::from_text("ab");
    s.replace_and_mark(2..2, "か");
    assert_eq!(s.text(), "abか");
    assert_eq!(s.marked_range(), Some(2..5));
    s.replace_range(2..5, "か");
    assert_eq!(s.marked_range(), None);
}

#[test]
fn edit_keystrokes_ignore_printable_keys() {
    let mut s = TextFieldState::from_text("ab");
    let printable = Keystroke {
        modifiers: Default::default(),
        key: "c".into(),
        key_char: Some("c".into()),
    };
    assert_eq!(s.edit_keystroke(&printable), EditEffect::Ignored);
    assert!(s.handle_keystroke(&printable));
    assert_eq!(s.text(), "abc");
}

#[test]
fn char_offset_saturates() {
    assert_eq!(char_offset("abc", 1), 1);
    assert_eq!(char_offset("abc", 9), 3);
    assert_eq!(char_offset("é!", 1), 2);
}

#[test]
fn editing_effects_distinguish_boundaries_caret_and_text() {
    let mut state = TextFieldState::from_text("é");
    let key = |key: &str| Keystroke {
        modifiers: Default::default(),
        key: key.into(),
        key_char: None,
    };
    assert_eq!(state.edit_keystroke(&key("right")), EditEffect::Unchanged);
    assert_eq!(state.edit_keystroke(&key("left")), EditEffect::CaretMoved);
    assert_eq!(
        state.edit_keystroke(&key("backspace")),
        EditEffect::Unchanged
    );
    assert_eq!(state.edit_keystroke(&key("delete")), EditEffect::Changed);
    assert_eq!(state.text(), "");
    assert_eq!(state.edit_keystroke(&key("a")), EditEffect::Ignored);
}

#[gpui::test]
fn native_input_keeps_display_and_caret_in_sync(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(TextInput::new))
            .expect("test window")
    });
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
    let input = window.root(&mut cx).expect("text input");
    cx.update(|window, cx| {
        input.update(cx, |input, cx| window.focus(&input.focus_handle, cx));
    });
    cx.simulate_input("héllo");
    input.read_with(&cx, |input, _| {
        assert_eq!(input.text(), "héllo");
        assert_eq!(input.display_text, "héllo");
        assert_eq!(input.state.cursor(), "héllo".len());
        assert!(input.last_layout.is_some());
    });
    cx.simulate_keystrokes("left backspace");
    input.read_with(&cx, |input, _| {
        assert_eq!(input.text(), "hélo");
        assert_eq!(input.display_text, "hélo");
        assert_eq!(input.state.caret_chars(), 3);
    });
}
