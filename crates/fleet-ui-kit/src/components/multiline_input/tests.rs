use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, EntityInputHandler, VisualTestContext, prelude::*};

use super::{HISTORY_LIMIT, MultilineBuffer, MultilineInput, MultilineInputEvent, PromptHistory};

use crate::theme::Theme;

/// A buffer holding `text` with the caret parked `chars` characters in.
fn buffer(text: &str, caret_chars: usize) -> MultilineBuffer {
    let mut buffer = MultilineBuffer::from_text(text);
    let offset = text
        .char_indices()
        .nth(caret_chars)
        .map_or(text.len(), |(index, _)| index);
    buffer.set_cursor(offset);
    buffer
}

/// The caret as a character index, which is what the assertions read.
fn caret(buffer: &MultilineBuffer) -> usize {
    buffer.text()[..buffer.cursor()].chars().count()
}

#[test]
fn insert_keeps_newlines_and_normalises_the_rest() {
    let mut b = MultilineBuffer::new();
    b.insert("one\r\ntwo\rthree\tfour\u{7}");
    assert_eq!(b.text(), "one\ntwo\nthree four");
    assert_eq!(b.cursor(), b.text().len());
}

#[test]
fn insert_newline_splits_the_line_at_the_caret() {
    let mut b = buffer("abcd", 2);
    b.insert_newline();
    assert_eq!(b.text(), "ab\ncd");
    assert_eq!(caret(&b), 3);
    assert!(!b.on_first_line());
    assert!(b.on_last_line());
}

#[test]
fn backspace_walks_grapheme_boundaries() {
    let mut b = buffer("héllo", 2);
    assert!(b.backspace());
    assert_eq!(b.text(), "hllo");
    assert_eq!(caret(&b), 1);
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let mut emoji = MultilineBuffer::from_text(format!("a{family}b"));
    emoji.set_cursor(1);
    assert!(emoji.delete_forward());
    assert_eq!(emoji.text(), "ab");
}

#[test]
fn backspace_at_a_line_start_joins_the_lines() {
    let mut b = MultilineBuffer::from_text("first\nsecond");
    b.set_cursor(6);
    assert!(b.backspace());
    assert_eq!(b.text(), "firstsecond");
    assert!(b.on_first_line());
}

#[test]
fn backspace_at_the_start_of_the_buffer_is_a_no_op() {
    let mut b = buffer("x", 0);
    assert!(!b.backspace());
    assert_eq!(b.text(), "x");
}

#[test]
fn word_deletion_stops_at_the_line_start() {
    let mut b = MultilineBuffer::from_text("origin/main feature ");
    assert!(b.delete_word_before());
    assert_eq!(b.text(), "origin/main ");
    assert!(b.delete_word_before());
    assert_eq!(b.text(), "");
    assert!(!b.delete_word_before());

    let mut lines = MultilineBuffer::from_text("first\nsecond");
    lines.set_cursor(lines.text().len());
    assert!(lines.delete_word_before());
    assert_eq!(lines.text(), "first\n");
    // The word walk stops at the line start; the next one falls back to joining the lines.
    assert!(lines.delete_word_before());
    assert_eq!(lines.text(), "first");
}

#[test]
fn forward_word_deletion_stops_at_the_line_end() {
    let mut b = MultilineBuffer::from_text("alpha beta\ngamma");
    b.set_cursor(0);
    assert!(b.delete_word_after());
    assert_eq!(b.text(), " beta\ngamma");
    b.move_to_line_end(false);
    assert!(b.delete_word_after());
    assert_eq!(b.text(), " betagamma");
}

#[test]
fn line_cuts_only_touch_the_current_line() {
    let mut b = MultilineBuffer::from_text("first\nsecond line");
    b.set_cursor(12);
    assert!(b.delete_to_line_end());
    assert_eq!(b.text(), "first\nsecond");
    assert!(b.delete_to_line_start());
    assert_eq!(b.text(), "first\n");
    assert!(!b.delete_to_line_start());
}

#[test]
fn shift_selection_extends_from_the_anchor_and_typing_replaces_it() {
    let mut b = buffer("abcdef", 1);
    assert!(b.move_right(true));
    assert!(b.move_right(true));
    assert_eq!(b.selected_range(), 1..3);
    assert_eq!(b.selected_text(), "bc");
    b.insert("X");
    assert_eq!(b.text(), "aXdef");
    assert!(b.selected_text().is_empty());
}

#[test]
fn a_plain_arrow_collapses_the_selection_to_its_edge() {
    let mut b = buffer("abcdef", 1);
    b.move_right(true);
    b.move_right(true);
    assert!(b.move_left(false));
    assert_eq!(caret(&b), 1);
    assert!(b.selected_text().is_empty());
}

#[test]
fn select_all_then_backspace_empties_the_buffer() {
    let mut b = MultilineBuffer::from_text("one\ntwo");
    assert!(b.select_all());
    assert_eq!(b.selected_range(), 0..7);
    assert!(b.backspace());
    assert!(b.is_empty());
    assert!(!b.select_all());
}

#[test]
fn word_motion_crosses_a_line_break_one_step_at_a_time() {
    let mut b = MultilineBuffer::from_text("alpha beta\ngamma");
    b.set_cursor(0);
    assert!(b.move_word_right(false));
    assert_eq!(caret(&b), 5);
    assert!(b.move_word_right(false));
    assert_eq!(caret(&b), 10);
    // At the line end the word walk has nowhere to go, so it steps over the newline.
    assert!(b.move_word_right(false));
    assert_eq!(caret(&b), 11);
    assert!(b.move_word_left(false));
    assert_eq!(caret(&b), 10);
}

#[test]
fn vertical_motion_keeps_the_goal_column() {
    let mut b = MultilineBuffer::from_text("longest line\nab\nanother line");
    b.set_cursor(10);
    assert!(b.move_down(false));
    assert_eq!(b.text()[..b.cursor()].lines().count(), 2);
    assert_eq!(caret(&b), 15); // clamped to the end of the short line
    assert!(b.move_down(false));
    assert_eq!(caret(&b), 26); // the goal column is restored on the long line
    assert!(b.move_up(false));
    assert!(b.move_up(false));
    assert_eq!(caret(&b), 10);
}

#[test]
fn vertical_motion_stops_at_the_first_and_last_line() {
    let mut b = MultilineBuffer::from_text("one\ntwo");
    b.set_cursor(0);
    assert!(!b.move_up(false));
    b.move_to_end(false);
    assert!(!b.move_down(false));
    assert!(b.on_last_line());
    assert!(!b.on_first_line());
}

/// `spec-B` §B5.8: `@` and `$` fire wherever a token starts; `/` fires at line start only,
/// because a harness expands a slash command only when it opens the whole message.
#[test]
fn triggers_fire_where_each_surface_can_honour_them() {
    let b = MultilineBuffer::from_text("look at ");
    assert_eq!(symbol(b.trigger_for(b.cursor(), "@")), Some('@'));
    assert_eq!(symbol(b.trigger_for(b.cursor(), "$")), Some('$'));
    // A token start mid-line is not a line start, so `/` stays prose.
    assert_eq!(b.trigger_for(b.cursor(), "/"), None);
    assert_eq!(symbol(b.trigger_for(0, "@")), Some('@'));
    assert_eq!(b.trigger_for(b.cursor(), "x"), None);
    assert_eq!(b.trigger_for(b.cursor(), "@f"), None);

    let mid = MultilineBuffer::from_text("feature");
    assert_eq!(mid.trigger_for(mid.cursor(), "/"), None);
    assert_eq!(mid.trigger_for(mid.cursor(), "@"), None);
    assert_eq!(mid.trigger_for(mid.cursor(), "$"), None);

    let newline = MultilineBuffer::from_text("first\n");
    assert_eq!(
        symbol(newline.trigger_for(newline.cursor(), "/")),
        Some('/')
    );

    let empty = MultilineBuffer::new();
    assert_eq!(symbol(empty.trigger_for(0, "/")), Some('/'));
}

/// The picker filters on what follows, so the trigger keeps reporting as the token grows — and
/// stops the moment the token gains whitespace.
#[test]
fn the_active_trigger_carries_the_query_the_picker_filters_on() {
    let b = MultilineBuffer::from_text("read @src/li");
    let trigger = b.active_trigger().expect("inside an @ token");
    assert_eq!(trigger.symbol, '@');
    assert_eq!(trigger.query, "src/li");
    assert_eq!(trigger.at, "read ".len());

    let b = MultilineBuffer::from_text("read @src/lib.rs and");
    assert!(b.active_trigger().is_none(), "the caret left the token");

    let b = MultilineBuffer::from_text("$rev");
    let trigger = b.active_trigger().expect("inside a $ token");
    assert_eq!(trigger.symbol, '$');
    assert_eq!(trigger.query, "rev");

    let b = MultilineBuffer::from_text("/mod");
    assert_eq!(
        b.active_trigger().map(|trigger| trigger.query),
        Some(gpui::SharedString::from("mod"))
    );
    // Mid-line, a slash token is not a command.
    let b = MultilineBuffer::from_text("cd /mod");
    assert!(b.active_trigger().is_none());

    assert!(
        MultilineBuffer::from_text("plain")
            .active_trigger()
            .is_none()
    );
}

/// The symbol of a reported trigger, for the assertions above.
fn symbol(trigger: Option<super::Trigger>) -> Option<char> {
    trigger.map(|trigger| trigger.symbol)
}

#[test]
fn utf16_offsets_round_trip_across_the_newline() {
    let b = MultilineBuffer::from_text("a😀\né");
    assert_eq!(b.len_utf16(), 5);
    let range = b.range_to_utf16(&(0..b.text().len()));
    assert_eq!(range, 0..5);
    assert_eq!(b.range_from_utf16(&range), 0..b.text().len());
}

#[test]
fn history_drops_blanks_and_adjacent_duplicates() {
    let mut history = PromptHistory::new();
    history.push("fix the rounding");
    history.push("fix the rounding");
    history.push("   ");
    history.push("");
    history.push("now the tz shifts");
    assert_eq!(history.entries(), ["fix the rounding", "now the tz shifts"]);
    assert!(!history.is_active());
}

#[test]
fn history_keeps_the_last_hundred_prompts() {
    let mut history = PromptHistory::new();
    for index in 0..HISTORY_LIMIT + 5 {
        history.push(format!("prompt {index}"));
    }
    assert_eq!(history.entries().len(), HISTORY_LIMIT);
    assert_eq!(history.entries()[0], "prompt 5");
    assert_eq!(history.entries()[HISTORY_LIMIT - 1], "prompt 104");
}

#[test]
fn history_walks_back_then_restores_the_draft() {
    let mut history = PromptHistory::new();
    history.push("first");
    history.push("second");

    assert_eq!(history.previous("draft").as_deref(), Some("second"));
    assert!(history.is_active());
    assert_eq!(history.previous("ignored").as_deref(), Some("first"));
    assert_eq!(history.previous("ignored"), None);
    assert_eq!(history.newer().as_deref(), Some("second"));
    assert_eq!(history.newer().as_deref(), Some("draft"));
    assert!(!history.is_active());
    assert_eq!(history.newer(), None);
}

#[test]
fn history_walk_on_an_empty_list_does_nothing() {
    let mut history = PromptHistory::new();
    assert_eq!(history.previous("draft"), None);
    assert!(!history.is_active());
}

/// A focused composer in a test window, plus the events it emitted.
type Composed = (
    Entity<MultilineInput>,
    Rc<RefCell<Vec<MultilineInputEvent>>>,
);

fn composer(cx: &mut gpui::TestAppContext) -> (VisualTestContext, Composed) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| MultilineInput::new(cx, "Message claude…".into()))
        })
        .expect("test window")
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let input = window.root(&mut cx).expect("composer");
    let events: Rc<RefCell<Vec<MultilineInputEvent>>> = Rc::default();
    let sink = events.clone();
    cx.update(|window, cx| {
        cx.subscribe(&input, move |_, event: &MultilineInputEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
        .detach();
        input.update(cx, |input, cx| window.focus(input.focus_handle(), cx));
    });
    (cx, (input, events))
}

/// The reported events with the per-keystroke [`MultilineInputEvent::Changed`] noise removed.
///
/// A picker subscribes to `Changed`; a test about submit or escape cares about the intent.
fn intents(events: &Rc<RefCell<Vec<MultilineInputEvent>>>) -> Vec<MultilineInputEvent> {
    events
        .borrow()
        .iter()
        .filter(|event| !matches!(event, MultilineInputEvent::Changed))
        .cloned()
        .collect()
}

#[gpui::test]
fn enter_submits_and_clears_while_shift_enter_breaks_the_line(cx: &mut gpui::TestAppContext) {
    let (mut cx, (input, events)) = composer(cx);
    cx.simulate_input("ship it");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("now");
    input.read_with(&cx, |input, _| assert_eq!(input.text(), "ship it\nnow"));

    cx.simulate_keystrokes("enter");
    input.read_with(&cx, |input, _| {
        assert!(input.text().is_empty());
        assert_eq!(input.history().entries(), ["ship it\nnow"]);
    });
    assert_eq!(
        intents(&events),
        [MultilineInputEvent::Submit("ship it\nnow".into())]
    );
}

#[gpui::test]
fn a_blank_composer_neither_submits_nor_clears(cx: &mut gpui::TestAppContext) {
    let (mut cx, (input, events)) = composer(cx);
    cx.simulate_input("  ");
    cx.simulate_keystrokes("enter");
    input.read_with(&cx, |input, _| assert_eq!(input.text(), "  "));
    assert!(intents(&events).is_empty());
}

#[gpui::test]
fn escape_reports_intent_without_touching_the_text(cx: &mut gpui::TestAppContext) {
    let (mut cx, (input, events)) = composer(cx);
    cx.simulate_input("half a thought");
    cx.simulate_keystrokes("escape");
    input.read_with(&cx, |input, _| assert_eq!(input.text(), "half a thought"));
    assert_eq!(intents(&events), [MultilineInputEvent::Escape]);
}

/// The trigger characters are **reported, never consumed**: all three stay in the buffer, so
/// they remain typable and a picker filters on what follows.
#[gpui::test]
fn a_trigger_character_is_inserted_and_reported(cx: &mut gpui::TestAppContext) {
    let (mut cx, (input, events)) = composer(cx);
    cx.simulate_input("read @");
    input.read_with(&cx, |input, _| assert_eq!(input.text(), "read @"));
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        MultilineInputEvent::Trigger(trigger) if trigger.symbol == '@' && trigger.query.is_empty()
    )));

    events.borrow_mut().clear();
    cx.simulate_input("src/lib.rs");
    input.read_with(&cx, |input, _| {
        assert_eq!(input.text(), "read @src/lib.rs");
        // Every keystroke reports a change, and the query is where the picker filters from.
        let trigger = input.active_trigger().expect("still inside the token");
        assert_eq!(trigger.query, "src/lib.rs");
    });
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| matches!(event, MultilineInputEvent::Changed)),
        "typing into a token must not re-open the picker"
    );
    assert!(!events.borrow().is_empty(), "a change is always reported");
}

#[gpui::test]
fn the_up_arrow_walks_history_and_restores_the_draft(cx: &mut gpui::TestAppContext) {
    let (mut cx, (input, _events)) = composer(cx);
    cx.simulate_input("first prompt");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("second prompt");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("draft");

    cx.simulate_keystrokes("up");
    input.read_with(&cx, |input, _| assert_eq!(input.text(), "draft"));

    cx.update(|_, cx| input.update(cx, |input, cx| input.clear(cx)));
    cx.simulate_keystrokes("up");
    input.read_with(&cx, |input, _| assert_eq!(input.text(), "second prompt"));
    cx.simulate_keystrokes("up");
    input.read_with(&cx, |input, _| assert_eq!(input.text(), "first prompt"));
    cx.simulate_keystrokes("down down");
    input.read_with(&cx, |input, _| assert!(input.text().is_empty()));
}

#[gpui::test]
fn typing_over_a_recalled_prompt_ends_the_history_walk(cx: &mut gpui::TestAppContext) {
    let (mut cx, (input, _events)) = composer(cx);
    cx.simulate_input("recall me");
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("up");
    input.read_with(&cx, |input, _| {
        assert_eq!(input.text(), "recall me");
        assert!(input.history().is_active());
    });
    cx.simulate_input("!");
    cx.simulate_keystrokes("home up");
    input.read_with(&cx, |input, _| {
        assert_eq!(input.text(), "recall me!");
        assert!(!input.history().is_active());
    });
}

#[gpui::test]
fn ime_composition_marks_and_commits_like_the_single_line_field(cx: &mut gpui::TestAppContext) {
    let (mut cx, (input, _events)) = composer(cx);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_text("é/", cx);
            input.replace_and_mark_text_in_range(None, "漢字", Some(0..1), window, cx);
            assert_eq!(input.text(), "é/漢字");
            assert_eq!(input.buffer().marked_range(), Some(3..9));
            assert_eq!(input.buffer().selected_range(), 3..6);

            // Committing lands where the platform parked the selection, inside the composed
            // run — the same rule the single-line field follows.
            input.unmark_text(window, cx);
            input.replace_text_in_range(None, "\n", window, cx);
            assert_eq!(input.text(), "é/漢\n字");
            assert!(input.buffer().marked_range().is_none());
        });
    });
}

#[gpui::test]
fn typed_text_is_the_primary_token_whatever_the_inherited_style_says(
    cx: &mut gpui::TestAppContext,
) {
    let (mut cx, (input, _events)) = composer(cx);
    cx.simulate_input("abcXYZ");
    // A measured layout runs outside the parent `div`'s `with_text_style` scope, so the
    // inherited style there is the window default: opaque black on Fleet's dark ground. The
    // composer therefore names its own tokens (`r2-12`: typed glyphs were invisible).
    let inherited = gpui::TextStyle {
        color: gpui::black(),
        ..Default::default()
    };
    cx.update(|_window, cx| {
        let theme = Theme::dark();
        let (display, runs) = super::element::content(input.read(cx), &theme, &inherited);
        assert_eq!(display.as_ref(), "abcXYZ");
        assert_eq!(runs.first().map(|run| run.color), Some(theme.colors.text));
        assert_ne!(theme.colors.text, gpui::black());

        input.update(cx, |input, cx| input.set_text("", cx));
        let (display, runs) = super::element::content(input.read(cx), &theme, &inherited);
        assert_eq!(display.as_ref(), "Message claude…");
        assert_eq!(
            runs.first().map(|run| run.color),
            Some(theme.colors.text_muted)
        );
    });
}
