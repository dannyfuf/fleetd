use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gpui::{
    ClipboardItem, Context, Entity, EntityInputHandler, FocusHandle, Focusable, KeyBinding,
    Modifiers, MouseButton, MouseDownEvent, Subscription, VisualTestContext, Window, actions, div,
    point, prelude::*, px,
};

use super::{InputMode, TEXT_INPUT_KEY_CONTEXT, TYPING_GROUP_WINDOW, TextInput, TextInputEvent};
use crate::{Theme, text_input};

actions!(text_input_test, [ParentEnter]);

#[derive(Clone, Copy, Default)]
struct Propagated {
    up: usize,
    down: usize,
    enter: usize,
}

struct InputHost {
    input: Entity<TextInput>,
    focus: FocusHandle,
    propagated: Rc<Cell<Propagated>>,
    _event_subscription: Subscription,
}

impl InputHost {
    fn parent_up(&mut self, _: &text_input::MoveUp, _: &mut Window, _: &mut Context<Self>) {
        let mut propagated = self.propagated.get();
        propagated.up += 1;
        self.propagated.set(propagated);
    }

    fn parent_down(&mut self, _: &text_input::MoveDown, _: &mut Window, _: &mut Context<Self>) {
        let mut propagated = self.propagated.get();
        propagated.down += 1;
        self.propagated.set(propagated);
    }

    fn parent_enter(&mut self, _: &ParentEnter, _: &mut Window, _: &mut Context<Self>) {
        let mut propagated = self.propagated.get();
        propagated.enter += 1;
        self.propagated.set(propagated);
    }
}

impl Focusable for InputHost {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for InputHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("TextInputTestHost")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::parent_up))
            .on_action(cx.listener(Self::parent_down))
            .on_action(cx.listener(Self::parent_enter))
            .w(px(360.0))
            .child(self.input.clone())
    }
}

pub(crate) fn test_bindings() -> Vec<KeyBinding> {
    let context = Some(TEXT_INPUT_KEY_CONTEXT);
    vec![
        KeyBinding::new("left", text_input::MoveLeft, context),
        KeyBinding::new("right", text_input::MoveRight, context),
        KeyBinding::new("alt-left", text_input::MoveWordLeft, context),
        KeyBinding::new("alt-right", text_input::MoveWordRight, context),
        KeyBinding::new("home", text_input::MoveToRowStart, context),
        KeyBinding::new("end", text_input::MoveToRowEnd, context),
        KeyBinding::new("cmd-left", text_input::MoveToLineStart, context),
        KeyBinding::new("cmd-right", text_input::MoveToLineEnd, context),
        KeyBinding::new("up", text_input::MoveUp, context),
        KeyBinding::new("down", text_input::MoveDown, context),
        KeyBinding::new("cmd-up", text_input::MoveToStart, context),
        KeyBinding::new("cmd-down", text_input::MoveToEnd, context),
        KeyBinding::new("shift-left", text_input::SelectLeft, context),
        KeyBinding::new("shift-right", text_input::SelectRight, context),
        KeyBinding::new("alt-shift-left", text_input::SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", text_input::SelectWordRight, context),
        KeyBinding::new("shift-home", text_input::SelectToRowStart, context),
        KeyBinding::new("shift-end", text_input::SelectToRowEnd, context),
        KeyBinding::new("cmd-shift-left", text_input::SelectToLineStart, context),
        KeyBinding::new("cmd-shift-right", text_input::SelectToLineEnd, context),
        KeyBinding::new("shift-up", text_input::SelectUp, context),
        KeyBinding::new("shift-down", text_input::SelectDown, context),
        KeyBinding::new("cmd-shift-up", text_input::SelectToStart, context),
        KeyBinding::new("cmd-shift-down", text_input::SelectToEnd, context),
        KeyBinding::new("ctrl-a", text_input::MoveToLineStart, context),
        KeyBinding::new("ctrl-e", text_input::MoveToLineEnd, context),
        KeyBinding::new("ctrl-shift-a", text_input::SelectToLineStart, context),
        KeyBinding::new("ctrl-shift-e", text_input::SelectToLineEnd, context),
        KeyBinding::new("ctrl-b", text_input::MoveLeft, context),
        KeyBinding::new("ctrl-f", text_input::MoveRight, context),
        KeyBinding::new("backspace", text_input::Backspace, context),
        KeyBinding::new("delete", text_input::Delete, context),
        KeyBinding::new("alt-backspace", text_input::DeleteWordBackward, context),
        KeyBinding::new("alt-delete", text_input::DeleteWordForward, context),
        KeyBinding::new("cmd-backspace", text_input::DeleteToLineStart, context),
        KeyBinding::new("cmd-delete", text_input::DeleteToLineEnd, context),
        KeyBinding::new("ctrl-w", text_input::DeleteWordBackward, context),
        KeyBinding::new("ctrl-u", text_input::DeleteToLineStart, context),
        KeyBinding::new("ctrl-k", text_input::DeleteToLineEnd, context),
        KeyBinding::new("ctrl-h", text_input::Backspace, context),
        KeyBinding::new("ctrl-d", text_input::Delete, context),
        KeyBinding::new("cmd-a", text_input::SelectAll, context),
        KeyBinding::new("cmd-c", text_input::Copy, context),
        KeyBinding::new("cmd-x", text_input::Cut, context),
        KeyBinding::new("cmd-v", text_input::Paste, context),
        KeyBinding::new("cmd-z", text_input::Undo, context),
        KeyBinding::new("cmd-shift-z", text_input::Redo, context),
        KeyBinding::new(
            "enter",
            text_input::Newline,
            Some("FleetTextInput && mode == multiline && enter == newline"),
        ),
        KeyBinding::new(
            "shift-enter",
            text_input::Newline,
            Some("FleetTextInput && mode == multiline"),
        ),
        KeyBinding::new("enter", ParentEnter, Some("TextInputTestHost")),
    ]
}

type HostedInput = (
    VisualTestContext,
    Entity<TextInput>,
    Rc<RefCell<Vec<TextInputEvent>>>,
    Rc<Cell<Propagated>>,
    FocusHandle,
);

fn hosted(
    cx: &mut gpui::TestAppContext,
    mode: InputMode,
    text: &str,
    configure: impl FnOnce(&mut TextInput, &mut Context<TextInput>) + 'static,
) -> HostedInput {
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        cx.bind_keys(test_bindings());
    });
    let text = text.to_owned();
    let events = Rc::new(RefCell::new(Vec::new()));
    let propagated = Rc::new(Cell::new(Propagated::default()));
    let window = cx.update(|cx| {
        let events_for_host = events.clone();
        let propagated_for_host = propagated.clone();
        cx.open_window(Default::default(), move |_, cx| {
            cx.new(|cx| {
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(mode, cx);
                    if !text.is_empty() {
                        input.set_text(text, cx);
                    }
                    configure(&mut input, cx);
                    input
                });
                let subscription = cx.subscribe(&input, move |_host, _input, event, _cx| {
                    events_for_host.borrow_mut().push(*event);
                });
                InputHost {
                    input,
                    focus: cx.focus_handle(),
                    propagated: propagated_for_host,
                    _event_subscription: subscription,
                }
            })
        })
        .expect("text input test window")
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let host = window.root(&mut visual).expect("text input host");
    let input = host.read_with(&visual, |host, _cx| host.input.clone());
    let parent_focus = host.read_with(&visual, |host, _cx| host.focus.clone());
    visual.update(|window, _cx| window.activate_window());
    visual.run_until_parked();
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.focus(window, cx);
        });
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    (visual, input, events, propagated, parent_focus)
}

#[gpui::test]
fn typing_emits_changed_and_selection_typing_replaces(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, events, _, _) = hosted(cx, InputMode::SingleLine, "", |_, _| {});
    visual.simulate_input("hello");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "hello"));
    assert!(events.borrow().contains(&TextInputEvent::Changed));

    visual.simulate_keystrokes("shift-left shift-left");
    visual.simulate_input("X");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "helX"));
}

#[gpui::test]
fn word_and_line_deletion_use_the_shared_actions(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "alpha beta", |_, _| {});
    visual.simulate_keystrokes("alt-backspace");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "alpha "));
    visual.simulate_keystrokes("ctrl-u");
    input.read_with(&visual, |input, _| assert!(input.text().is_empty()));
}

#[gpui::test]
fn single_line_vertical_motion_and_enter_propagate(cx: &mut gpui::TestAppContext) {
    let (mut visual, _input, _, propagated, _) =
        hosted(cx, InputMode::SingleLine, "value", |_, _| {});
    visual.simulate_keystrokes("up down enter");
    let propagated = propagated.get();
    assert_eq!(propagated.up, 1);
    assert_eq!(propagated.down, 1);
    assert_eq!(propagated.enter, 1);
}

#[gpui::test]
fn multiline_enter_inserts_a_newline(cx: &mut gpui::TestAppContext) {
    let mode = InputMode::Multiline {
        min_rows: 2,
        max_rows: 4,
    };
    let (mut visual, input, _, _, _) = hosted(cx, mode, "first", |_, _| {});
    visual.simulate_keystrokes("enter");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "first\n"));
}

#[gpui::test]
fn multiline_enter_with_owner_policy_propagates(cx: &mut gpui::TestAppContext) {
    let mode = InputMode::Multiline {
        min_rows: 1,
        max_rows: 4,
    };
    let (mut visual, input, _, propagated, _) = hosted(cx, mode, "first", |input, cx| {
        input.set_enter_inserts_newline(false, cx);
    });
    visual.simulate_keystrokes("enter");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "first"));
    assert_eq!(propagated.get().enter, 1);
}

#[gpui::test]
fn visual_up_moves_below_the_first_row_and_propagates_at_the_top(cx: &mut gpui::TestAppContext) {
    let mode = InputMode::Multiline {
        min_rows: 1,
        max_rows: 4,
    };
    let paragraph = "wrapping words across a deliberately narrow editor ".repeat(12);
    let (mut visual, input, _, propagated, _) = hosted(cx, mode, &paragraph, |_, _| {});
    let end = input.read_with(&visual, |input, _| input.buffer.caret());
    visual.simulate_keystrokes("up");
    input.read_with(&visual, |input, _| assert!(input.buffer.caret() < end));
    assert_eq!(propagated.get().up, 0);

    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.buffer.set_caret(0);
            input.reveal_caret = true;
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    visual.simulate_keystrokes("up");
    assert_eq!(propagated.get().up, 1);
}

#[gpui::test]
fn multiline_wraps_visual_rows_grows_to_the_cap_and_reveals_the_caret(
    cx: &mut gpui::TestAppContext,
) {
    let mode = InputMode::Multiline {
        min_rows: 2,
        max_rows: 3,
    };
    let paragraph = "wrapping words across a deliberately narrow editor ".repeat(20);
    let (visual, input, _, _, _) = hosted(cx, mode, &paragraph, |_, _| {});
    input.read_with(&visual, |input, _| {
        let bounds = input.last_bounds.expect("input bounds");
        assert!(input.line_cache.visual_rows() > 3);
        assert_eq!(bounds.size.height, input.line_height * 3.0);
        assert!(input.scroll_row > 0);
    });
}

#[gpui::test]
fn wrapped_hit_testing_bounds_and_selection_share_visual_geometry(cx: &mut gpui::TestAppContext) {
    let mode = InputMode::Multiline {
        min_rows: 2,
        max_rows: 4,
    };
    let paragraph = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu ".repeat(4);
    let (mut visual, input, _, _, _) = hosted(cx, mode, &paragraph, |input, _cx| {
        input.buffer.set_caret(0);
    });
    let second_row_point = input.read_with(&visual, |input, _| {
        let bounds = input.last_bounds.expect("input bounds");
        point(
            bounds.left() + input.line_height,
            bounds.top() + input.line_height * 1.5,
        )
    });
    visual.simulate_mouse_down(second_row_point, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(second_row_point, MouseButton::Left, Modifiers::none());
    let (offset, bounds, line_height) = input.read_with(&visual, |input, _| {
        let offset = input.buffer.caret();
        assert!(offset > 0);
        assert!(offset < input.text().len());
        (
            offset,
            input.last_bounds.expect("input bounds"),
            input.line_height,
        )
    });
    let range_bounds = visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.bounds_for_range(offset..offset + 1, bounds, window, cx)
        })
    });
    let range_bounds = range_bounds.expect("wrapped range bounds");
    assert!(range_bounds.top() >= bounds.top() + line_height);
    assert!(range_bounds.top() < bounds.top() + line_height * 2.0);

    visual.update(|window, cx| {
        input.update(cx, |input, _cx| {
            input.buffer.set_selected_range(0..offset + 1);
            input.reveal_caret = true;
        });
        window.draw(cx).clear(cx);
    });
    input.read_with(&visual, |input, _| {
        assert_eq!(input.last_selection_quad_count, 2)
    });
}

#[gpui::test]
fn single_line_remains_unwrapped_and_scrolls_horizontally(cx: &mut gpui::TestAppContext) {
    let text = "single-line-content-".repeat(40);
    let (visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, &text, |_, _| {});
    input.read_with(&visual, |input, _| {
        assert_eq!(input.line_cache.visual_rows(), 1);
        assert_eq!(input.line_cache.logical_lines(), 1);
        assert!(input.horizontal_scroll > gpui::Pixels::ZERO);
    });
}

#[gpui::test]
fn clipboard_round_trip_and_single_line_newline_sanitising(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "alpha beta", |_, _| {});
    visual.simulate_keystrokes("cmd-a cmd-c cmd-x");
    input.read_with(&visual, |input, _| assert!(input.text().is_empty()));
    visual.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("alpha beta".to_owned())
        );
        cx.write_to_clipboard(ClipboardItem::new_string("line one\nline two".to_owned()));
    });
    visual.simulate_keystrokes("cmd-v");
    input.read_with(&visual, |input, _| {
        assert_eq!(input.text(), "line one line two")
    });
}

#[gpui::test]
fn typing_bursts_group_by_the_test_clock_and_redo_restores_them(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "", |_, _| {});
    visual.simulate_input("a");
    visual.simulate_input("b");
    visual.simulate_keystrokes("cmd-z");
    input.read_with(&visual, |input, _| assert!(input.text().is_empty()));
    visual.simulate_keystrokes("cmd-shift-z");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "ab"));

    visual.update(|_, cx| input.update(cx, |input, cx| input.set_text("", cx)));
    visual.simulate_input("a");
    visual
        .executor()
        .advance_clock(TYPING_GROUP_WINDOW + Duration::from_millis(1));
    visual.simulate_input("b");
    visual.simulate_keystrokes("cmd-z");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "a"));
    visual.simulate_keystrokes("cmd-z");
    input.read_with(&visual, |input, _| assert!(input.text().is_empty()));
}

#[gpui::test]
fn ime_composition_is_one_undo_step(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "", |_, _| {});
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "h", None, window, cx);
            input.replace_and_mark_text_in_range(None, "漢", None, window, cx);
            input.replace_text_in_range(None, "漢字", window, cx);
        });
    });
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "漢字"));
    visual.simulate_keystrokes("cmd-z");
    input.read_with(&visual, |input, _| assert!(input.text().is_empty()));
}

#[gpui::test]
fn clearing_during_composition_closes_its_undo_group(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "", |_, _| {});
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "x", None, window, cx);
            input.clear(cx);
        });
    });
    visual.simulate_input("a");
    visual.simulate_keystrokes("cmd-z cmd-z");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "x"));
    visual.simulate_keystrokes("cmd-z");
    input.read_with(&visual, |input, _| assert!(input.text().is_empty()));
}

#[gpui::test]
fn read_only_ignores_platform_and_action_edits(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "locked", |input, cx| {
        input.set_read_only(true, cx);
    });
    visual.simulate_keystrokes("backspace cmd-x cmd-v");
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_text_in_range(None, "x", window, cx)
        });
    });
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "locked"));
}

#[gpui::test]
fn character_filter_drops_rejected_input(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "", |input, cx| {
        input.set_filter(Some(|character| character.is_ascii_digit()), cx);
    });
    visual.simulate_input("a1b2c3");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "123"));
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("letters");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "123"));
    visual.simulate_keystrokes("right");
    visual.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("x4y5".to_owned()));
    });
    visual.simulate_keystrokes("cmd-v");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "12345"));

    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_text("", cx);
            input.replace_and_mark_text_in_range(None, "1", None, window, cx);
            input.replace_text_in_range(None, "letter", window, cx);
        });
    });
    input.read_with(&visual, |input, _| {
        assert!(input.text().is_empty());
        assert!(!input.is_composing());
    });
}

#[track_caller]
fn point_for_offset(input: &TextInput, offset: usize) -> gpui::Point<gpui::Pixels> {
    let bounds = input.last_bounds.expect("input bounds");
    let position = input
        .position_for_offset(offset)
        .expect("shaped offset position");
    point(
        bounds.left() + position.x - input.horizontal_scroll,
        bounds.top() + position.y - input.line_height * input.scroll_row as f32
            + input.line_height / 2.0,
    )
}

#[gpui::test]
fn pointer_drag_and_double_click_select_text(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, InputMode::SingleLine, "alpha beta", |_, _| {});
    let (start, end, beta) = input.read_with(&visual, |input, _| {
        (
            point_for_offset(input, 0),
            point_for_offset(input, input.text().len()),
            point_for_offset(input, "alpha be".len()),
        )
    });
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(
        point(end.x + px(20.0), end.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    input.read_with(&visual, |input, _| {
        assert_eq!(input.buffer().selected_text(), "alpha beta")
    });

    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: beta,
        modifiers: Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    input.read_with(&visual, |input, _| {
        assert_eq!(input.buffer().selected_text(), "beta")
    });
}

#[gpui::test]
fn losing_focus_emits_blurred(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, events, _, _) = hosted(cx, InputMode::SingleLine, "value", |_, _| {});
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, " marked", None, window, cx);
        });
    });
    visual.update(|window, cx| {
        window.blur();
        window.draw(cx).clear(cx);
    });
    visual.run_until_parked();
    input.read_with(&visual, |input, _| assert!(!input.is_composing()));
    assert!(events.borrow().contains(&TextInputEvent::Blurred));
}
