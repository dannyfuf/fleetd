use std::time::{Duration, Instant};

use super::{HISTORY_CAP, InputBuffer, InputMode, TYPING_GROUP_WINDOW};

const MULTILINE: InputMode = InputMode::Multiline {
    min_rows: 2,
    max_rows: 8,
};

#[derive(Debug, PartialEq, Eq)]
struct MarkedText {
    text: String,
    caret: usize,
    anchor: usize,
}

#[track_caller]
fn parse_marked(source: &str) -> MarkedText {
    let mut text = String::new();
    let mut caret = None;
    let mut anchor = None;
    for character in source.chars() {
        match character {
            '|' => {
                assert!(
                    caret.is_none() && anchor.is_none(),
                    "duplicate caret marker"
                );
                caret = Some(text.len());
                anchor = Some(text.len());
            }
            '«' => {
                assert!(anchor.is_none(), "duplicate selection start");
                anchor = Some(text.len());
            }
            '»' => {
                assert!(caret.is_none(), "duplicate selection end");
                caret = Some(text.len());
            }
            other => text.push(other),
        }
    }
    MarkedText {
        text,
        caret: caret.expect("marked text needs | or »"),
        anchor: anchor.expect("marked text needs | or «"),
    }
}

#[track_caller]
fn buffer(mode: InputMode, marked: &str) -> InputBuffer {
    let parsed = parse_marked(marked);
    let mut buffer = InputBuffer::from_text(mode, parsed.text);
    buffer.set_selected_range(parsed.anchor..parsed.caret);
    buffer
}

#[track_caller]
fn assert_marked(buffer: &InputBuffer, expected: &str) {
    let expected = parse_marked(expected);
    assert_eq!(buffer.text(), expected.text);
    assert_eq!(buffer.caret(), expected.caret, "caret differs");
    assert_eq!(buffer.anchor(), expected.anchor, "anchor differs");
}

fn at(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}

// Seed parity: these preserve every generic editing contract that the composer's own removed
// buffer covered with pure unit tests. Composer-only triggers and prompt history remain with the
// composer and are intentionally not generic input behavior.

#[test]
fn parity_insert_keeps_newlines_tabs_and_drops_other_controls() {
    let now = Instant::now();
    let mut input = buffer(MULTILINE, "|");
    assert!(input.insert("one\r\ntwo\rthree\tfour\u{7}", now));
    assert_marked(&input, "one\ntwo\nthree\tfour|");
}

#[test]
fn parity_word_selection_uses_unicode_classes() {
    let mut input = buffer(MULTILINE, "alpha ca|fé, gamma");
    assert!(input.select_word_at("alpha ca".len()));
    assert_eq!(input.selected_text(), "café");
    assert!(input.select_word_at("alpha café".len()));
    assert_eq!(input.selected_text(), ",");
}

#[test]
fn parity_insert_newline_splits_the_line_at_the_caret() {
    let mut input = buffer(MULTILINE, "ab|cd");
    assert!(input.insert_newline(Instant::now()));
    assert_marked(&input, "ab\n|cd");
    assert!(!input.on_first_line());
    assert!(input.on_last_line());
}

#[test]
fn parity_deletion_walks_grapheme_boundaries() {
    let now = Instant::now();
    let mut combining = buffer(MULTILINE, "x e\u{301}| y");
    assert!(combining.backspace(now));
    assert_marked(&combining, "x | y");

    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let mut emoji = buffer(MULTILINE, &format!("a|{family}b"));
    assert!(emoji.delete_forward(now));
    assert_marked(&emoji, "a|b");
}

#[test]
fn parity_backspace_at_a_line_start_joins_lines() {
    let mut input = buffer(MULTILINE, "first\n|second");
    assert!(input.backspace(Instant::now()));
    assert_marked(&input, "first|second");
    assert!(input.on_first_line());
}

#[test]
fn parity_backspace_at_document_start_is_a_no_op() {
    let mut input = buffer(MULTILINE, "|x");
    assert!(!input.backspace(Instant::now()));
    assert_marked(&input, "|x");
}

#[test]
fn parity_backward_word_deletion_stops_at_line_start() {
    let now = Instant::now();
    let mut input = buffer(MULTILINE, "alpha beta |");
    assert!(input.delete_word_backward(now));
    assert_marked(&input, "alpha |");
    assert!(input.delete_word_backward(now));
    assert_marked(&input, "|");

    let mut lines = buffer(MULTILINE, "first\nsecond|");
    assert!(lines.delete_word_backward(now));
    assert_marked(&lines, "first\n|");
    assert!(lines.delete_word_backward(now));
    assert_marked(&lines, "first|");
}

#[test]
fn parity_forward_word_deletion_stops_at_line_end() {
    let now = Instant::now();
    let mut input = buffer(MULTILINE, "|alpha beta\ngamma");
    assert!(input.delete_word_forward(now));
    assert_marked(&input, "| beta\ngamma");
    input.move_to_line_end(false);
    assert!(input.delete_word_forward(now));
    assert_marked(&input, " beta|gamma");
}

#[test]
fn parity_line_cuts_only_touch_the_current_line() {
    let now = Instant::now();
    let mut input = buffer(MULTILINE, "first\nsecond| line");
    assert!(input.delete_to_line_end(now));
    assert_marked(&input, "first\nsecond|");
    assert!(input.delete_to_line_start(now));
    assert_marked(&input, "first\n|");
    assert!(!input.delete_to_line_start(now));
}

#[test]
fn parity_shift_selection_extends_and_typing_replaces_it() {
    let mut input = buffer(MULTILINE, "a|bcdef");
    assert!(input.move_right(true));
    assert!(input.move_right(true));
    assert_marked(&input, "a«bc»def");
    assert!(input.insert("X", Instant::now()));
    assert_marked(&input, "aX|def");
}

#[test]
fn parity_plain_arrow_collapses_selection_to_its_edge() {
    let mut input = buffer(MULTILINE, "a«bc»def");
    assert!(input.move_left(false));
    assert_marked(&input, "a|bcdef");
}

#[test]
fn parity_select_all_then_backspace_empties_the_buffer() {
    let mut input = buffer(MULTILINE, "one\ntwo|");
    assert!(input.select_all());
    assert_marked(&input, "«one\ntwo»");
    assert!(input.backspace(Instant::now()));
    assert_marked(&input, "|");
    assert!(!input.select_all());
}

#[test]
fn parity_word_motion_crosses_line_break_one_grapheme_at_a_time() {
    let mut input = buffer(MULTILINE, "|alpha beta\ngamma");
    assert!(input.move_word_right(false));
    assert_marked(&input, "alpha| beta\ngamma");
    assert!(input.move_word_right(false));
    assert_marked(&input, "alpha beta|\ngamma");
    assert!(input.move_word_right(false));
    assert_marked(&input, "alpha beta\n|gamma");
    assert!(input.move_word_left(false));
    assert_marked(&input, "alpha beta|\ngamma");
}

#[test]
fn parity_vertical_motion_keeps_the_goal_column() {
    let mut input = buffer(MULTILINE, "longest l|ine\nab\nanother line");
    assert!(input.move_down(false));
    assert_marked(&input, "longest line\nab|\nanother line");
    assert!(input.move_down(false));
    assert_marked(&input, "longest line\nab\nanother l|ine");
    assert!(input.move_up(false));
    assert!(input.move_up(false));
    assert_marked(&input, "longest l|ine\nab\nanother line");
}

#[test]
fn parity_vertical_motion_stops_at_first_and_last_lines() {
    let mut input = buffer(MULTILINE, "|one\ntwo");
    assert!(!input.move_up(false));
    input.move_to_document_end(false);
    assert!(!input.move_down(false));
    assert!(input.on_last_line());
    assert!(!input.on_first_line());
}

#[test]
fn parity_utf16_offsets_round_trip_across_newline_and_non_bmp_text() {
    let input = buffer(MULTILINE, "a😀\né|");
    assert_eq!(input.len_utf16(), 5);
    let range = input.range_to_utf16(&(0..input.text().len()));
    assert_eq!(range, 0..5);
    assert_eq!(input.range_from_utf16(&range), 0..input.text().len());
}

// New unified-engine coverage.

#[test]
fn single_line_sanitizes_line_endings_and_tabs_without_merging_boundaries() {
    let mut input = buffer(InputMode::SingleLine, "|");
    assert!(input.insert("a\r\nb\nc\rd\te", Instant::now()));
    assert_marked(&input, "a b c d e|");
    assert_eq!(input.line_count(), 1);
}

#[test]
fn word_motion_observes_word_whitespace_and_punctuation_classes() {
    let mut input = buffer(MULTILINE, "alpha|  ... beta");
    assert!(input.move_word_right(false));
    assert_marked(&input, "alpha  ...| beta");
    assert!(input.move_word_right(false));
    assert_marked(&input, "alpha  ... beta|");
    assert!(input.move_word_left(false));
    assert_marked(&input, "alpha  ... |beta");
    assert!(input.move_word_left(false));
    assert_marked(&input, "alpha  |... beta");
}

#[test]
fn word_deletion_stops_after_a_long_whitespace_run() {
    let now = Instant::now();
    let mut backward = buffer(MULTILINE, "alpha   |beta");
    assert!(backward.delete_word_backward(now));
    assert_marked(&backward, "alpha|beta");

    let mut forward = buffer(MULTILINE, "alpha|   beta");
    assert!(forward.delete_word_forward(now));
    assert_marked(&forward, "alpha|beta");

    let mut one_space = buffer(MULTILINE, "alpha |beta");
    assert!(one_space.delete_word_backward(now));
    assert_marked(&one_space, "|beta");

    let mut punctuation = buffer(MULTILINE, "alpha...| beta");
    assert!(punctuation.delete_word_backward(now));
    assert_marked(&punctuation, "alpha| beta");
}

#[test]
fn tab_insertion_obeys_each_modes_documented_storage_rule() {
    let now = Instant::now();
    let mut single = buffer(InputMode::SingleLine, "a|b");
    assert!(single.insert_tab(now));
    assert_marked(&single, "a |b");

    let mut multiline = buffer(MULTILINE, "a|b");
    assert!(multiline.insert_tab(now));
    assert_marked(&multiline, "a\t|b");
}

#[test]
fn single_line_line_rules_cover_the_whole_document_and_vertical_motion_propagates() {
    let mut input = buffer(InputMode::SingleLine, "alpha |beta");
    assert!(!input.move_up(false));
    assert!(!input.move_down(false));
    assert!(input.move_to_line_start(false));
    assert_marked(&input, "|alpha beta");
    assert!(input.move_to_line_end(true));
    assert_marked(&input, "«alpha beta»");
}

#[test]
fn document_motion_is_distinct_from_multiline_line_motion() {
    let mut input = buffer(MULTILINE, "one\nt|wo\nthree");
    assert!(input.move_to_line_start(false));
    assert_marked(&input, "one\n|two\nthree");
    assert!(input.move_to_document_start(true));
    assert_marked(&input, "»one\n«two\nthree");
    assert!(input.move_to_document_end(false));
    assert_marked(&input, "one\ntwo\nthree|");
}

#[test]
fn vertical_motion_can_extend_a_selection_from_its_original_anchor() {
    let mut input = buffer(MULTILINE, "one t|wo\nshort\nlast two");
    assert!(input.move_down(true));
    assert!(input.move_down(true));
    assert_marked(&input, "one t«wo\nshort\nlast »two");
}

#[test]
fn line_selection_includes_the_logical_line_delimiter() {
    let mut input = buffer(MULTILINE, "one\nt|wo\nthree");
    assert!(input.select_line_at(5));
    assert_marked(&input, "one\n«two\n»three");
}

#[test]
fn every_deletion_replaces_a_selection_first() {
    let now = Instant::now();
    for delete in [
        InputBuffer::delete_word_backward,
        InputBuffer::delete_word_forward,
        InputBuffer::delete_to_line_start,
        InputBuffer::delete_to_line_end,
    ] {
        let mut input = buffer(MULTILINE, "a«bc»d");
        assert!(delete(&mut input, now));
        assert_marked(&input, "a|d");
    }
}

#[test]
fn revision_changes_only_when_text_changes() {
    let now = Instant::now();
    let mut input = buffer(MULTILINE, "a|");
    let initial = input.revision();
    assert!(!input.insert("", now));
    assert_eq!(input.revision(), initial);
    input.move_to_document_start(false);
    assert_eq!(input.revision(), initial);
    assert!(input.insert("b", now));
    assert_eq!(input.revision(), initial + 1);
    assert!(input.undo());
    assert_eq!(input.revision(), initial + 2);
}

#[test]
fn ime_replace_mark_selection_and_unmark_use_byte_offsets() {
    let base = Instant::now();
    let mut input = buffer(MULTILINE, "é/|");
    assert!(input.replace_and_mark(3..3, "漢😀", base));
    assert_eq!(input.marked_range(), Some(3..10));
    assert_eq!(input.range_to_utf16(&(3..10)), 2..5);
    input.set_selected_range(3..6);
    assert_eq!(input.selected_text(), "漢");
    input.unmark();
    assert!(input.marked_range().is_none());
    assert!(!input.has_selection());
    assert_marked(&input, "é/漢|😀");
}

#[test]
fn utf16_conversion_snaps_inside_surrogates_and_combining_clusters() {
    let input = buffer(MULTILINE, "a😀e\u{301}|");
    assert_eq!(input.offset_from_utf16(2), "a😀".len());
    assert_eq!(input.offset_from_utf16(4), "a😀".len());
    assert_eq!(input.offset_to_utf16("a😀e".len()), 3);
}

#[test]
fn typing_inside_the_window_is_one_undo_step_and_a_pause_splits_it() {
    let base = Instant::now();
    let mut input = buffer(MULTILINE, "|");
    assert!(input.insert("a", at(base, 0)));
    assert!(input.insert("b", at(base, 200)));
    assert!(input.insert("c", at(base, 501)));
    assert_marked(&input, "abc|");
    assert!(input.undo());
    assert_marked(&input, "ab|");
    assert!(input.undo());
    assert_marked(&input, "|");
    assert!(!input.undo());
    assert_eq!(TYPING_GROUP_WINDOW, Duration::from_millis(300));
}

#[test]
fn caret_motion_ends_a_timed_typing_group() {
    let base = Instant::now();
    let mut input = buffer(MULTILINE, "|");
    input.insert("a", base);
    input.move_left(false);
    input.move_right(false);
    input.insert("b", at(base, 10));
    assert!(input.undo());
    assert_marked(&input, "a|");
    assert!(input.undo());
    assert_marked(&input, "|");
}

#[test]
fn explicit_composition_group_collapses_multiple_replacements() {
    let base = Instant::now();
    let mut input = buffer(MULTILINE, "|");
    input.begin_history_group();
    assert!(input.replace_and_mark(0..0, "k", base));
    assert!(input.replace_and_mark(0..1, "漢", at(base, 500)));
    input.unmark();
    input.end_history_group();
    assert_marked(&input, "漢|");
    assert!(input.undo());
    assert_marked(&input, "|");
    assert!(!input.undo());
}

#[test]
fn undo_restores_the_selection_replaced_by_an_insert() {
    let mut input = buffer(MULTILINE, "a»bc«d");
    assert!(input.insert("X", Instant::now()));
    assert_marked(&input, "aX|d");
    assert!(input.undo());
    assert_marked(&input, "a»bc«d");
}

#[test]
fn a_new_edit_after_undo_clears_redo() {
    let base = Instant::now();
    let mut input = buffer(MULTILINE, "|");
    input.insert("a", base);
    input.insert("b", at(base, 500));
    assert!(input.undo());
    assert!(input.can_redo());
    input.insert("x", at(base, 1_000));
    assert!(!input.can_redo());
    assert!(!input.redo());
    assert_marked(&input, "ax|");
}

#[test]
fn redo_restores_the_selection_after_undo() {
    let mut input = buffer(MULTILINE, "a«bc»d");
    input.insert("X", Instant::now());
    assert!(input.undo());
    assert!(input.redo());
    assert_marked(&input, "aX|d");
}

#[test]
fn history_cap_discards_the_oldest_steps() {
    let base = Instant::now();
    let mut input = buffer(MULTILINE, "|");
    for index in 0..HISTORY_CAP + 5 {
        let end = input.text().len();
        assert!(input.replace_range(end..end, "x", at(base, index as u64)));
    }
    let mut undos = 0;
    while input.undo() {
        undos += 1;
    }
    assert_eq!(undos, HISTORY_CAP);
    assert_eq!(input.text(), "xxxxx");
}

#[test]
fn programmatic_set_text_resets_history_and_sanitizes_for_the_mode() {
    let mut input = buffer(InputMode::SingleLine, "a|");
    input.insert("b", Instant::now());
    assert!(input.can_undo());
    input.set_text("x\ny");
    assert_marked(&input, "x y|");
    assert!(!input.can_undo());
    assert!(!input.can_redo());
}

#[test]
fn clear_is_one_undoable_edit() {
    let mut input = buffer(MULTILINE, "one\ntwo|");
    assert!(input.clear(Instant::now()));
    assert_marked(&input, "|");
    assert!(input.undo());
    assert_marked(&input, "one\ntwo|");
}
