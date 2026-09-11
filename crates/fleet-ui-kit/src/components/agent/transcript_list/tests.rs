//! The wiring the pure modules cannot cover: what `set_rows` does to `ListState`, what a thread
//! switch resets, what the row-focus keys report, and the two timers.

use std::time::{Duration, Instant};

use gpui::{Entity, SharedString, TestAppContext};

use super::*;
use crate::components::agent::rows::{
    NoticeRow, ReasoningRow, TranscriptRow, TranscriptRowId, TranscriptRowKind, UserRow,
    WorkingPhase, WorkingRow,
};
use crate::components::agent::scroll::Gesture;
use crate::components::agent::tool_row::ToolRow;
use crate::theme::Theme;

fn user(text: &str) -> TranscriptRow {
    TranscriptRow::ordinal(0, TranscriptRowKind::User(UserRow::new(text)))
}

fn notice(text: &str) -> TranscriptRow {
    TranscriptRow::ordinal(
        1,
        TranscriptRowKind::Notice(NoticeRow { text: text.into() }),
    )
}

fn expandable(id: &str) -> TranscriptRow {
    TranscriptRow::new(
        TranscriptRowId::Item(id.into()),
        TranscriptRowKind::Work(ToolRow::new(id, "bash", "cargo test").body("exit 0")),
    )
}

fn transcript(cx: &mut TestAppContext, rows: Vec<TranscriptRow>) -> Entity<TranscriptList> {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let list = cx.update(|cx| cx.new(TranscriptList::new));
    cx.update(|cx| {
        list.update(cx, |list, cx| list.set_rows(rows, cx));
    });
    list
}

#[gpui::test]
fn set_rows_keeps_the_list_state_in_step_with_the_projection(cx: &mut TestAppContext) {
    let list = transcript(cx, vec![user("a"), expandable("t1")]);
    list.read_with(cx, |list, _| {
        assert_eq!(list.rows().len(), 2);
        assert_eq!(list.state.item_count(), 2);
    });
    cx.update(|cx| {
        list.update(cx, |list, cx| list.set_rows(vec![user("a")], cx));
    });
    list.read_with(cx, |list, _| {
        assert_eq!(list.rows().len(), 1);
        assert_eq!(list.state.item_count(), 1);
        assert_eq!(list.focused_row(), None);
    });
}

/// A thread switch is the only moment every measured height is worthless, so it is the only
/// caller of `reset` — and it puts the scroll machine back at the live edge.
#[gpui::test]
fn switching_threads_resets_the_measured_heights_and_the_scroll_machine(cx: &mut TestAppContext) {
    let list = transcript(cx, vec![user("a"), user("b")]);
    cx.update(|cx| {
        list.update(cx, |list, cx| {
            list.scroll_mode(true, cx);
            list.focus_row(Some(1), cx);
        });
    });
    list.read_with(cx, |list, _| {
        assert!(list.is_scroll_mode());
        assert_eq!(list.focused_row(), Some(1));
        assert!(!list.is_following());
    });

    cx.update(|cx| {
        list.update(cx, |list, cx| list.set_thread(vec![user("c")], cx));
    });
    list.read_with(cx, |list, _| {
        assert_eq!(list.state.item_count(), 1);
        assert!(!list.is_scroll_mode());
        assert_eq!(list.focused_row(), None);
        assert!(list.is_following());
        assert!(list.state.is_following_tail());
    });
}

/// `G` is documented as "newest", not as a way out of scroll mode: the status bar's `SCROLL`
/// word stays on until `q` / `i` / `esc`, and so does the frozen tail.
#[gpui::test]
fn jumping_to_the_newest_row_does_not_thaw_the_frozen_tail(cx: &mut TestAppContext) {
    let list = transcript(cx, vec![user("a"), user("b")]);
    cx.update(|cx| {
        list.update(cx, |list, cx| list.scroll_mode(true, cx));
    });
    list.read_with(cx, |list, _| {
        assert!(list.is_scroll_mode());
        assert!(!list.state.is_following_tail());
        assert!(!list.is_following());
    });

    cx.update(|cx| {
        list.update(cx, |list, cx| list.scroll_to_end(cx));
    });
    list.read_with(cx, |list, _| {
        assert!(list.is_scroll_mode());
        assert!(
            !list.state.is_following_tail(),
            "`G` re-armed the tail while the status bar still read SCROLL"
        );
        assert!(!list.is_following());
    });

    // Leaving the mode is what resumes the follow.
    cx.update(|cx| {
        list.update(cx, |list, cx| list.scroll_mode(false, cx));
    });
    list.read_with(cx, |list, _| {
        assert!(!list.is_scroll_mode());
        assert!(list.is_following());
        assert!(list.state.is_following_tail());
    });
}

/// Row focus lives inside scroll mode, which is what finally makes `⏎` / `u` / `o` fire.
#[gpui::test]
fn the_row_focus_keys_report_against_the_focused_row(cx: &mut TestAppContext) {
    let list = transcript(cx, vec![user("a"), expandable("t1")]);
    list.read_with(cx, |list, _| {
        assert_eq!(list.event_for_key("enter"), None, "no focus, no key");
    });

    cx.update(|cx| {
        list.update(cx, |list, cx| list.focus_row(Some(0), cx));
    });
    list.read_with(cx, |list, _| {
        // A plain user message has nothing to show, so `⏎` is not claimed.
        assert_eq!(list.event_for_key("enter"), None);
        assert_eq!(
            list.event_for_key("y"),
            Some(TranscriptEvent::RowAction {
                row: SharedString::from("row-0"),
                action: RowAction::Copy
            })
        );
        assert_eq!(list.event_for_key("z"), None);
    });

    cx.update(|cx| {
        list.update(cx, |list, cx| list.focus_row(Some(1), cx));
    });
    list.read_with(cx, |list, _| {
        assert_eq!(
            list.event_for_key("enter"),
            Some(TranscriptEvent::Toggle(SharedString::from("t1")))
        );
        for (key, action) in [
            ("u", RowAction::Revert),
            ("o", RowAction::Open),
            ("d", RowAction::Diff),
        ] {
            assert_eq!(
                list.event_for_key(key),
                Some(TranscriptEvent::RowAction {
                    row: SharedString::from("t1"),
                    action
                })
            );
        }
    });
}

#[gpui::test]
fn the_row_focus_is_clamped_to_the_transcript(cx: &mut TestAppContext) {
    let list = transcript(cx, vec![user("a"), user("b"), notice("c")]);
    cx.update(|cx| {
        list.update(cx, |list, cx| {
            list.focus_row(Some(9), cx);
            assert_eq!(list.focused_row(), None);
            list.move_row_focus(-1, cx);
        });
    });
    // With no focus, `k` starts from the newest row.
    list.read_with(cx, |list, _| assert_eq!(list.focused_row(), Some(1)));
    cx.update(|cx| {
        list.update(cx, |list, cx| {
            list.move_row_focus(-5, cx);
            assert_eq!(list.focused_row(), Some(0));
            list.move_row_focus(9, cx);
            assert_eq!(list.focused_row(), Some(2));
        });
    });
}

/// Debounced on show only: a thread switch fires scroll events with `is_at_end = false` while
/// the initial position settles, and an undebounced chip flashes through that window.
#[gpui::test]
fn the_jump_chip_is_debounced_on_show_and_immediate_on_hide(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    // Enough rows that the content really overflows: a gesture may only break follow when it
    // can actually move the viewport away from the live edge.
    let rows: Vec<_> = (0..80)
        .map(|index| user(&format!("message {index}")))
        .collect();
    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut list = TranscriptList::new(cx);
                    list.set_rows(rows, cx);
                    list
                })
            })
        })
        .expect("test window");
    let list = window.root(cx).expect("transcript");
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();

    visual.update(|_window, cx| {
        list.update(cx, |list, cx| {
            assert!(list.gesture(Gesture::WheelUp, cx), "the wheel broke follow");
            assert!(!list.is_following());
            assert!(
                !list.shows_jump_to_latest(),
                "the chip appeared immediately"
            );
        });
    });

    visual.executor().advance_clock(Duration::from_millis(200));
    visual.run_until_parked();
    list.read_with(visual, |list, _| assert!(list.shows_jump_to_latest()));

    visual.update(|_window, cx| {
        list.update(cx, |list, cx| {
            list.scroll_to_latest(cx);
            assert!(!list.shows_jump_to_latest(), "hiding is never debounced");
        });
    });
}

/// A wheel gesture on content that fits cannot move the viewport, so it must not break follow —
/// a spurious break produces no scroll event, never re-arms, and streaming silently stops
/// following.
#[gpui::test]
fn a_gesture_on_content_that_fits_never_breaks_follow(cx: &mut TestAppContext) {
    let list = transcript(cx, vec![user("a")]);
    cx.update(|cx| {
        list.update(cx, |list, cx| {
            assert!(!list.gesture(Gesture::WheelUp, cx));
            assert!(list.is_following());
            assert!(!list.shows_jump_to_latest());
        });
    });
}

/// The clock is not in the model: the row carries `started_at` and the list writes one string.
#[gpui::test]
fn the_working_clock_ticks_only_while_a_working_row_exists(cx: &mut TestAppContext) {
    let working = TranscriptRow::new(
        TranscriptRowId::LiveActivity,
        TranscriptRowKind::Working(WorkingRow {
            phase: WorkingPhase::Working,
            started_at: Some(Instant::now()),
            harness: "claude".into(),
            detail: None,
        }),
    );
    let list = transcript(cx, vec![user("a"), working]);
    list.read_with(cx, |list, _| {
        assert!(
            list.working_label
                .as_ref()
                .is_some_and(|label| label.starts_with("working ")),
            "the clock label is prepared outside render"
        );
        assert!(list._working_tick.is_some());
    });

    cx.update(|cx| {
        list.update(cx, |list, cx| list.set_rows(vec![user("a")], cx));
    });
    list.read_with(cx, |list, _| {
        assert!(list.working_label.is_none());
        assert!(list._working_tick.is_none(), "the tick outlived its row");
    });
}

/// A reasoning row that is still streaming shares the live id, and its label comes from the
/// row rather than from a clock the list owns.
#[gpui::test]
fn a_streaming_reasoning_row_needs_no_clock(cx: &mut TestAppContext) {
    let reasoning = TranscriptRow::new(
        TranscriptRowId::LiveActivity,
        TranscriptRowKind::Reasoning(ReasoningRow {
            text: SharedString::default(),
            duration_ms: None,
            expanded: false,
        }),
    );
    let list = transcript(cx, vec![reasoning]);
    list.read_with(cx, |list, _| {
        assert!(list.working_label.is_none());
        assert!(list._working_tick.is_none());
    });
}

#[gpui::test]
fn the_row_body_renderer_sees_every_row(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorder = std::rc::Rc::clone(&seen);
    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut list = TranscriptList::new(cx);
                    list.set_rows(vec![user("a"), expandable("t1")], cx);
                    list.set_row_body(
                        move |row, _cx| {
                            recorder.borrow_mut().push(row.id.key());
                            // Declining leaves the row's own body text in place.
                            None
                        },
                        cx,
                    );
                    list
                })
            })
        })
        .expect("test window");
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    // A bottom-aligned list measures from the newest row upwards, so the renderer sees the
    // tail first. Both rows reach it, which is the property that matters.
    let mut seen = seen.borrow().clone();
    seen.sort();
    assert_eq!(seen, vec!["row-0", "t1"]);
}

#[test]
fn the_thumb_disappears_when_the_transcript_fits() {
    use gpui::px;
    assert_eq!(scroll_thumb(px(0.0), px(0.0), px(400.0), px(24.0)), None);
    assert_eq!(scroll_thumb(px(0.0), px(100.0), px(0.0), px(24.0)), None);
    let (top, height) =
        scroll_thumb(px(100.0), px(100.0), px(300.0), px(24.0)).expect("a scrollable transcript");
    assert!((height - 0.75).abs() < f32::EPSILON);
    assert!((top - 0.25).abs() < f32::EPSILON);
}

/// A very long transcript still has a grabbable thumb, and the thumb never runs off the track.
#[test]
fn the_thumb_keeps_a_minimum_height_and_stays_in_the_track() {
    use gpui::px;
    let (top, height) = scroll_thumb(px(9_900.0), px(9_900.0), px(100.0), px(24.0))
        .expect("a scrollable transcript");
    assert!(height >= 24.0 / 10_000.0);
    assert!(top + height <= 1.0 + f32::EPSILON);
}

/// Reaching the top of the rows in hand asks the owner for the page behind them, **once**.
///
/// The transcript cannot know whether older history exists, so it reports the gesture and the
/// owner decides. Firing repeatedly while the reader sits at the top would be a request per
/// scroll event.
#[gpui::test]
fn reaching_the_top_asks_for_older_history_once_per_row_set(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    // Addressable ids, so the row diff can tell a prepended page from a rewritten tail.
    let rows: Vec<_> = (0..80)
        .map(|index| expandable(&format!("row-{index}")))
        .collect();
    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut list = TranscriptList::new(cx);
                    list.set_rows(rows, cx);
                    list
                })
            })
        })
        .expect("test window");
    let list = window.root(cx).expect("transcript");
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();

    let asked = std::rc::Rc::new(std::cell::Cell::new(0_usize));
    let seen = std::rc::Rc::clone(&asked);
    visual.update(|_window, cx| {
        cx.subscribe(&list, move |_, event, _| {
            if matches!(event, TranscriptEvent::ReachedOldest) {
                seen.set(seen.get() + 1);
            }
        })
        .detach();
    });

    visual.update(|_window, cx| list.update(cx, |list, cx| list.scroll_to_top(cx)));
    visual.run_until_parked();
    assert_eq!(asked.get(), 1, "the top was reached once");

    // Still at the top: reaching it again is not another question.
    visual.update(|_window, cx| list.update(cx, |list, cx| list.scroll_to_top(cx)));
    visual.run_until_parked();
    assert_eq!(asked.get(), 1, "sitting at the top asks nothing more");

    // The owner answers by prepending history, which is a new first row — and re-arms it.
    visual.update(|_window, cx| {
        list.update(cx, |list, cx| {
            let mut rows = vec![expandable("older")];
            rows.extend(list.rows().to_vec());
            list.set_rows(rows, cx);
            list.scroll_to_top(cx);
        });
    });
    visual.run_until_parked();
    assert_eq!(asked.get(), 2, "a prepended page re-arms the question");
}
