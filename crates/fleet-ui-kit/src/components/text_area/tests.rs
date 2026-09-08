use gpui::Keystroke;

use super::TextAreaState;
use super::line::wrap_chunks;

fn state(text: &str, cursor: usize) -> TextAreaState {
    let mut state = TextAreaState::from_text(text);
    state.set_cursor(cursor);
    state
}

fn key(key: &str) -> Keystroke {
    Keystroke {
        modifiers: Default::default(),
        key: key.into(),
        key_char: None,
    }
}

fn ctrl(key: &str) -> Keystroke {
    Keystroke {
        modifiers: gpui::Modifiers {
            control: true,
            ..Default::default()
        },
        key: key.into(),
        key_char: None,
    }
}

#[test]
fn from_text_parks_the_caret_at_the_end() {
    let state = TextAreaState::from_text("one\ntwo");
    assert_eq!(state.cursor(), 7);
    assert_eq!(state.line_col(), (1, 3));
    assert_eq!(state.line_count(), 2);
}

#[test]
fn insert_normalizes_line_breaks_and_tabs() {
    let mut state = TextAreaState::new();
    state.insert("a\r\nb\rc\td");
    assert_eq!(state.text(), "a\nb\nc  d");
    assert_eq!(state.cursor(), state.text().len());
}

#[test]
fn enter_inserts_a_newline_and_tab_inserts_two_spaces() {
    let mut state = TextAreaState::new();
    state.insert("fix");
    assert!(state.handle_keystroke(&key("enter")));
    assert!(state.handle_keystroke(&key("tab")));
    assert_eq!(state.text(), "fix\n  ");
    assert_eq!(state.line_col(), (1, 2));
}

#[test]
fn line_col_counts_characters_not_bytes() {
    let state = state("héllo\nwörld", 10);
    // Line 1 starts at byte 7 and "wö" spans three bytes: that is column 2, not 3.
    assert_eq!(state.line_col(), (1, 2));
    // An offset inside the ö snaps back to the character that owns it.
    assert_eq!(super::TextAreaState::from_text("wö").line_col(), (0, 2));
}

#[test]
fn cursor_snaps_to_a_char_boundary() {
    let mut state = TextAreaState::from_text("é\n😀");
    state.set_cursor(1);
    assert_eq!(state.cursor(), 0);
    state.set_cursor(4);
    assert_eq!(state.cursor(), 3);
    state.set_cursor(999);
    assert_eq!(state.cursor(), state.text().len());
}

#[test]
fn arrows_cross_line_breaks_by_characters() {
    let mut state = state("añ\nb", 0);
    assert!(state.move_right());
    assert!(state.move_right());
    assert_eq!(state.cursor(), 3); // past the two-byte ñ
    assert!(state.move_right()); // over the newline
    assert_eq!(state.line_col(), (1, 0));
    assert!(state.move_left());
    assert_eq!(state.line_col(), (0, 2));
    assert!(state.move_to_start());
    assert!(!state.move_left());
}

#[test]
fn up_and_down_preserve_the_preferred_column() {
    let mut state = state("abcdef\nxy\nlmnopq", 5);
    assert_eq!(state.line_col(), (0, 5));
    assert!(state.move_down());
    assert_eq!(state.line_col(), (1, 2)); // clamped to the short line
    assert!(state.move_down());
    assert_eq!(state.line_col(), (2, 5)); // …and back to column 5
    assert!(state.move_up());
    assert_eq!(state.line_col(), (1, 2));
}

#[test]
fn an_edit_forgets_the_preferred_column() {
    let mut state = state("abcdef\nxy\nlmnopq", 5);
    assert!(state.move_down());
    state.insert("!");
    assert!(state.move_down());
    assert_eq!(state.line_col(), (2, 3));
}

#[test]
fn up_at_the_first_line_and_down_at_the_last_do_nothing() {
    let mut state = state("one\ntwo", 1);
    assert!(!state.move_up());
    assert_eq!(state.line_col(), (0, 1));
    state.move_to_end();
    assert!(!state.move_down());
}

#[test]
fn up_and_down_land_on_char_boundaries_in_multibyte_lines() {
    let mut state = state("ñññññ\nabcde", 10);
    assert_eq!(state.line_col(), (0, 5));
    assert!(state.move_down());
    assert_eq!(state.cursor(), 16);
    assert!(state.move_up());
    assert_eq!(state.cursor(), 10);
}

#[test]
fn backspace_joins_lines_and_walks_char_boundaries() {
    let mut state = state("añ\nb", 3);
    assert!(state.backspace()); // deletes the whole ñ
    assert_eq!(state.text(), "a\nb");
    state.set_cursor(2);
    assert!(state.backspace()); // deletes the line break
    assert_eq!(state.text(), "ab");
    state.set_cursor(0);
    assert!(!state.backspace());
}

#[test]
fn delete_forward_removes_the_char_after_the_caret() {
    let mut state = state("a😀b", 1);
    assert!(state.delete_forward());
    assert_eq!(state.text(), "ab");
    state.move_to_end();
    assert!(!state.delete_forward());
}

#[test]
fn word_ops_cut_around_the_caret() {
    let mut state = TextAreaState::from_text("origin/main feature ");
    assert!(state.delete_word_before());
    assert_eq!(state.text(), "origin/main ");
    state.set_cursor(0);
    assert!(state.delete_word_after());
    assert_eq!(state.text(), " ");
    assert!(state.delete_word_after());
    assert_eq!(state.text(), "");
    assert!(!state.delete_word_before());
    assert!(!state.delete_word_after());
}

#[test]
fn ctrl_u_and_ctrl_k_are_line_scoped() {
    let mut state = state("one\ntwo three\nfour", 7);
    assert!(state.delete_to_line_end());
    assert_eq!(state.text(), "one\ntwo\nfour");
    assert!(state.delete_to_line_start());
    assert_eq!(state.text(), "one\n\nfour");
    // At the end of a line `ctrl-k` eats the break itself.
    assert!(state.delete_to_line_end());
    assert_eq!(state.text(), "one\nfour");
}

#[test]
fn home_and_end_stay_on_the_current_line() {
    let mut state = state("one\ntwo", 5);
    assert!(state.handle_keystroke(&key("home")));
    assert_eq!(state.cursor(), 4);
    // The keystroke is still consumed once the caret is parked; only the motion reports
    // that it had nowhere to go.
    assert!(state.handle_keystroke(&key("home")));
    assert!(!state.move_to_line_start());
    assert!(state.handle_keystroke(&ctrl("e")));
    assert_eq!(state.cursor(), 7);
    assert!(state.move_to_start());
    assert_eq!(state.cursor(), 0);
}

#[test]
fn printable_keystrokes_insert_and_control_ones_do_not() {
    let mut state = TextAreaState::new();
    let printable = Keystroke {
        modifiers: Default::default(),
        key: "ñ".into(),
        key_char: Some("ñ".into()),
    };
    assert!(!state.handle_edit_keystroke(&printable));
    assert!(state.handle_keystroke(&printable));
    assert_eq!(state.text(), "ñ");
    assert!(!state.handle_keystroke(&ctrl("s")));
    assert_eq!(state.text(), "ñ");
}

#[test]
fn reveal_cursor_scrolls_the_minimum_amount() {
    let mut state = TextAreaState::from_text("0\n1\n2\n3\n4\n5\n6\n7");
    assert!(state.reveal_cursor(3)); // caret on line 7
    assert_eq!(state.scroll_row(), 5);
    state.set_cursor(0);
    assert!(state.reveal_cursor(3));
    assert_eq!(state.scroll_row(), 0);
    assert!(!state.reveal_cursor(3));
}

#[test]
fn set_text_and_clear_reset_the_view() {
    let mut state = TextAreaState::from_text("a\nb\nc");
    state.set_scroll_row(2);
    state.set_text("x");
    assert_eq!(state.text(), "x");
    assert_eq!(state.cursor(), 1);
    assert_eq!(state.scroll_row(), 0);
    state.clear();
    assert!(state.is_empty());
    assert_eq!(state.cursor(), 0);
}

#[test]
fn lines_keeps_the_empty_line_after_a_trailing_break() {
    let state = TextAreaState::from_text("a\n");
    assert_eq!(state.lines().collect::<Vec<_>>(), vec!["a", ""]);
    assert_eq!(state.line_count(), 2);
}

#[test]
fn wrap_chunks_keep_their_trailing_space() {
    assert_eq!(
        wrap_chunks("two words here").collect::<Vec<_>>(),
        vec!["two ", "words ", "here"]
    );
    assert_eq!(wrap_chunks("").next(), None);
}
