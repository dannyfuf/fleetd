use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{
    ClipboardItem, Context, Entity, EntityInputHandler, FocusHandle, KeyBinding, Modifiers,
    MouseButton, MouseDownEvent, ScrollDelta, ScrollWheelEvent, Subscription, VisualTestContext,
    Window, actions, div, point, prelude::*, px,
};

use super::{MultilineInput, MultilineInputEvent};
use crate::{Theme, theme::ActiveTheme};

actions!(composer_test, [OwnerUp]);

#[derive(Clone, Copy, Default)]
struct Propagated {
    up: usize,
}

struct ComposerHost {
    input: Entity<MultilineInput>,
    focus: FocusHandle,
    width: gpui::Pixels,
    bubbled_wheels: Rc<Cell<usize>>,
    propagated: Rc<Cell<Propagated>>,
    _subscription: Subscription,
}

impl ComposerHost {
    fn owner_up(&mut self, _: &OwnerUp, _: &mut Window, cx: &mut Context<Self>) {
        let mut propagated = self.propagated.get();
        propagated.up += 1;
        self.propagated.set(propagated);
        self.input.update(cx, |input, cx| input.caret_up(cx));
    }
}

impl Render for ComposerHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bubbled_wheels = self.bubbled_wheels.clone();
        div()
            .key_context("ComposerTestHost")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::owner_up))
            .size_full()
            .on_scroll_wheel(move |_, _, _| bubbled_wheels.set(bubbled_wheels.get() + 1))
            .child(div().w(self.width).child(self.input.clone()))
    }
}

type Hosted = (
    VisualTestContext,
    Entity<MultilineInput>,
    Rc<RefCell<Vec<MultilineInputEvent>>>,
    Rc<Cell<usize>>,
    Rc<Cell<Propagated>>,
);

fn hosted(cx: &mut gpui::TestAppContext, text: &str, width: gpui::Pixels) -> Hosted {
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        cx.bind_keys(crate::components::input::live_tests::test_bindings());
        cx.bind_keys([KeyBinding::new("up", OwnerUp, Some("ComposerTestHost"))]);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let bubbled_wheels = Rc::new(Cell::new(0));
    let propagated = Rc::new(Cell::new(Propagated::default()));
    let text = text.to_owned();
    let events_for_window = events.clone();
    let wheels_for_window = bubbled_wheels.clone();
    let propagated_for_window = propagated.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), move |_, cx| {
            cx.new(|cx| {
                let input = cx.new(|cx| {
                    let mut input = MultilineInput::new(cx, "Message claude…".into());
                    if !text.is_empty() {
                        input.set_text(text, cx);
                    }
                    input
                });
                let events = events_for_window.clone();
                let subscription =
                    cx.subscribe(&input, move |_, _, event: &MultilineInputEvent, _| {
                        events.borrow_mut().push(event.clone());
                    });
                ComposerHost {
                    input,
                    focus: cx.focus_handle(),
                    width,
                    bubbled_wheels: wheels_for_window,
                    propagated: propagated_for_window,
                    _subscription: subscription,
                }
            })
        })
        .expect("composer test window")
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let host = window.root(&mut visual).expect("composer host");
    let input = host.read_with(&visual, |host, _| host.input.clone());
    visual.update(|window, _| window.activate_window());
    visual.update(|window, cx| {
        let focus = input.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    (visual, input, events, bubbled_wheels, propagated)
}

fn composer(cx: &mut gpui::TestAppContext) -> Hosted {
    hosted(cx, "", px(360.0))
}

fn inner(input: &Entity<MultilineInput>, cx: &VisualTestContext) -> Entity<crate::TextInput> {
    input.read_with(cx, |input, _| input.inner())
}

#[track_caller]
fn point_for_offset(
    input: &Entity<MultilineInput>,
    offset: usize,
    cx: &VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    let inner = inner(input, cx);
    inner.read_with(cx, |inner, _| {
        let bounds = inner.test_bounds().expect("inner input bounds");
        let local = inner
            .test_position_for_offset(offset)
            .expect("offset has painted geometry");
        point(
            bounds.left() + local.x,
            bounds.top() + local.y - inner.test_line_height() * inner.test_scroll_row() as f32
                + inner.test_line_height() / 2.0,
        )
    })
}

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
    let (mut visual, input, events, _, _) = composer(cx);
    visual.simulate_input("ship it");
    visual.simulate_keystrokes("shift-enter");
    visual.simulate_input("now");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "ship it\nnow")
    });

    visual.simulate_keystrokes("enter");
    input.read_with(&visual, |input, cx| {
        assert!(input.text(cx).is_empty());
        assert_eq!(input.history().entries(), ["ship it\nnow"]);
    });
    assert_eq!(
        intents(&events),
        [MultilineInputEvent::Submit("ship it\nnow".into())]
    );
}

#[gpui::test]
fn enter_with_owner_policy_reaches_the_wrapper_fallback(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, events, _, _) = composer(cx);
    visual.simulate_input("send me");
    visual.simulate_keystrokes("enter");
    input.read_with(&visual, |input, cx| assert!(input.text(cx).is_empty()));
    assert_eq!(
        intents(&events),
        [MultilineInputEvent::Submit("send me".into())]
    );
}

#[gpui::test]
fn a_blank_composer_neither_submits_nor_clears(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, events, _, _) = composer(cx);
    visual.simulate_input("  ");
    visual.simulate_keystrokes("enter");
    input.read_with(&visual, |input, cx| assert_eq!(input.text(cx), "  "));
    assert!(intents(&events).is_empty());
}

#[gpui::test]
fn paste_preserves_tabs_and_drops_other_controls(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = composer(cx);
    visual.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("\tname\tvalue\u{7}".to_owned()))
    });
    visual.simulate_keystrokes("cmd-v");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "\tname\tvalue")
    });
}

#[gpui::test]
fn escape_reports_intent_without_touching_the_text(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, events, _, _) = composer(cx);
    visual.simulate_input("half a thought");
    visual.simulate_keystrokes("escape");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "half a thought")
    });
    assert_eq!(intents(&events), [MultilineInputEvent::Escape]);
}

#[gpui::test]
fn a_trigger_character_is_inserted_and_reported(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, events, _, _) = composer(cx);
    visual.simulate_input("read @");
    input.read_with(&visual, |input, cx| assert_eq!(input.text(cx), "read @"));
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        MultilineInputEvent::Trigger(trigger) if trigger.symbol == '@' && trigger.query.is_empty()
    )));

    events.borrow_mut().clear();
    visual.simulate_input("src/lib.rs");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "read @src/lib.rs");
        assert_eq!(
            input.active_trigger(cx).map(|trigger| trigger.query),
            Some("src/lib.rs".into())
        );
    });
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| matches!(event, MultilineInputEvent::Changed))
    );
}

#[gpui::test]
fn the_up_arrow_walks_history_and_restores_the_draft(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = composer(cx);
    visual.simulate_input("first prompt");
    visual.simulate_keystrokes("enter");
    visual.simulate_input("second prompt");
    visual.simulate_keystrokes("enter");
    visual.simulate_input("draft");

    visual.simulate_keystrokes("up");
    input.read_with(&visual, |input, cx| assert_eq!(input.text(cx), "draft"));

    visual.update(|_, cx| input.update(cx, MultilineInput::clear));
    visual.simulate_keystrokes("up");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "second prompt")
    });
    visual.simulate_keystrokes("up");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "first prompt")
    });
    visual.simulate_keystrokes("down down");
    input.read_with(&visual, |input, cx| assert!(input.text(cx).is_empty()));
}

#[gpui::test]
fn typing_over_a_recalled_prompt_ends_the_history_walk(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = composer(cx);
    visual.simulate_input("recall me");
    visual.simulate_keystrokes("enter up");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "recall me");
        assert!(input.history().is_active());
    });
    visual.simulate_input("!");
    visual.simulate_keystrokes("home up");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "recall me!");
        assert!(!input.history().is_active());
    });
}

#[gpui::test]
fn ime_composition_marks_and_commits_like_the_shared_input(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = composer(cx);
    let inner = inner(&input, &visual);
    visual.update(|window, cx| {
        input.update(cx, |input, cx| input.set_text("é/", cx));
        inner.update(cx, |inner, cx| {
            inner.replace_and_mark_text_in_range(None, "漢字", Some(0..1), window, cx);
            assert_eq!(inner.text(), "é/漢字");
            assert_eq!(inner.buffer().marked_range(), Some(3..9));
            assert_eq!(inner.buffer().selected_range(), 3..6);
            inner.unmark_text(window, cx);
            inner.replace_text_in_range(None, "\n", window, cx);
            assert_eq!(inner.text(), "é/漢\n字");
            assert!(inner.buffer().marked_range().is_none());
        });
    });
}

#[gpui::test]
fn enter_does_not_submit_an_open_ime_composition(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, events, _, _) = composer(cx);
    let inner = inner(&input, &visual);
    visual.update(|window, cx| {
        inner.update(cx, |inner, cx| {
            inner.replace_and_mark_text_in_range(None, "漢", Some(1..1), window, cx);
        });
    });
    visual.simulate_keystrokes("enter");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.text(cx), "漢");
        assert!(input.is_composing(cx));
    });
    assert!(intents(&events).is_empty());
}

#[gpui::test]
fn an_overlong_token_wraps_inside_the_composer(cx: &mut gpui::TestAppContext) {
    let (visual, input, _, _, _) = hosted(cx, &"x".repeat(300), px(140.0));
    let inner = inner(&input, &visual);
    inner.read_with(&visual, |inner, _| {
        assert!(inner.test_visual_rows() > 1);
        assert!(inner.test_bounds().is_some());
    });
}

#[gpui::test]
fn shaping_cache_invalidates_only_the_edited_logical_line(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, "first\nsecond\nthird", px(240.0));
    visual.update(|_, cx| {
        let inner = input.read(cx).inner();
        inner.update(cx, |inner, _| inner.test_reset_shape_probe());
        input.update(cx, |input, cx| input.set_text("first\nsecond!\nthird", cx));
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let inner = inner(&input, &visual);
    inner.read_with(&visual, |inner, _| {
        assert_eq!(inner.test_visual_rows(), 3);
        assert!(
            inner
                .test_position_for_offset("first\nsecond!".len())
                .is_some()
        );
        let counts = inner.test_shape_miss_counts();
        assert!(
            counts.contains(&1),
            "the edited line was not reshaped: {counts:?}"
        );
        assert!(
            counts.iter().all(|misses| matches!(*misses, 0 | 1 | 3)),
            "a stable-width pass reshaped an unchanged logical line: {counts:?}"
        );
    });
}

#[gpui::test]
fn home_uses_the_visual_row_and_stale_layout_falls_back_to_logical_motion(
    cx: &mut gpui::TestAppContext,
) {
    let (mut visual, input, _, _, _) = hosted(cx, &"x".repeat(300), px(140.0));
    let inner = inner(&input, &visual);
    visual.update(|window, cx| {
        inner.update(cx, |inner, cx| {
            inner.set_selected_text_range(150..150, window, cx)
        });
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("home");
    input.read_with(&visual, |input, cx| {
        assert!(input.buffer(cx).caret() > 0);
        assert!(input.buffer(cx).caret() < 150);
    });

    visual.update(|_, cx| {
        input.update(cx, |input, cx| {
            input.set_text("a\nb", cx);
            assert!(input.caret_up(cx));
            assert_eq!(input.buffer(cx).caret(), 1);
        });
    });
}

#[gpui::test]
fn modified_home_and_end_keep_logical_line_semantics(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, &"x".repeat(300), px(140.0));
    let inner = inner(&input, &visual);
    visual.update(|window, cx| {
        inner.update(cx, |inner, cx| {
            inner.set_selected_text_range(150..150, window, cx)
        });
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-left");
    input.read_with(&visual, |input, cx| assert_eq!(input.buffer(cx).caret(), 0));
    visual.simulate_keystrokes("cmd-right");
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.buffer(cx).caret(), input.text(cx).len())
    });
}

#[gpui::test]
fn pointer_drag_selects_and_double_click_selects_a_word(cx: &mut gpui::TestAppContext) {
    let (mut visual, input, _, _, _) = hosted(cx, "alpha beta", px(240.0));
    let start = point_for_offset(&input, 0, &visual);
    let end = point_for_offset(&input, "alpha beta".len(), &visual);
    let beta = point_for_offset(&input, "alpha be".len(), &visual);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.buffer(cx).selected_text(), "alpha beta")
    });

    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: beta,
        modifiers: Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    input.read_with(&visual, |input, cx| {
        assert_eq!(input.buffer(cx).selected_text(), "beta")
    });
}

#[gpui::test]
fn wheel_scrolls_an_overflowing_draft_without_bubbling(cx: &mut gpui::TestAppContext) {
    let text = (0..20)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut visual, input, _, bubbled_wheels, _) = hosted(cx, &text, px(240.0));
    let inner = inner(&input, &visual);
    let center = inner.read_with(&visual, |inner, _| {
        inner.test_bounds().expect("input bounds").center()
    });
    visual.simulate_event(ScrollWheelEvent {
        position: center,
        delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        ..Default::default()
    });
    inner.read_with(&visual, |inner, _| assert!(inner.test_scroll_row() > 0));
    assert_eq!(bubbled_wheels.get(), 0);
}

#[gpui::test]
fn typed_text_is_the_primary_token_whatever_the_inherited_style_says(
    cx: &mut gpui::TestAppContext,
) {
    let (mut visual, input, _, _, _) = composer(cx);
    visual.simulate_input("abcXYZ");
    let inner = inner(&input, &visual);
    inner.read_with(&visual, |inner, cx| {
        assert_eq!(inner.test_value_color(cx), cx.theme().colors.text)
    });
    visual.update(|_, cx| input.update(cx, MultilineInput::clear));
    inner.read_with(&visual, |inner, cx| {
        assert_eq!(inner.test_value_color(cx), cx.theme().colors.text_muted)
    });
}

#[gpui::test]
fn up_at_the_first_visual_row_propagates_while_lower_rows_move(cx: &mut gpui::TestAppContext) {
    let text = "wrapping words across a deliberately narrow composer ".repeat(10);
    let (mut visual, input, _, _, propagated) = hosted(cx, &text, px(160.0));
    let before = input.read_with(&visual, |input, cx| input.buffer(cx).caret());
    visual.simulate_keystrokes("up");
    input.read_with(&visual, |input, cx| {
        assert!(input.buffer(cx).caret() < before)
    });
    assert_eq!(propagated.get().up, 0);

    let inner = inner(&input, &visual);
    visual.update(|window, cx| {
        inner.update(cx, |inner, cx| {
            inner.set_selected_text_range(0..0, window, cx)
        });
        window.draw(cx).clear(cx);
    });
    visual.simulate_keystrokes("up");
    assert_eq!(propagated.get().up, 1);
}
