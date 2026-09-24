//! The pointer half of the dialog frame: the scrim, the close ✕ and the footer buttons each
//! reach the same handler or action a key would.

use std::{cell::Cell, rc::Rc};

use gpui::{
    AppContext, Context, FocusHandle, Modifiers, Point, Render, VisualTestContext, WindowHandle,
    point, px,
};

use super::*;
use crate::{
    components::ButtonStyle,
    harness::{self, RecordedTarget},
};

gpui::actions!(dialog_test, [Create]);

struct Host {
    focus: FocusHandle,
    dismissed: Rc<Cell<usize>>,
    created: usize,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        let dismissed = self.dismissed.clone();
        let cancelled = self.dismissed.clone();
        div()
            .key_context("DialogTest")
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(|host, _: &Create, _, _| host.created += 1))
            .child(
                Dialog::new("New worktree")
                    .on_dismiss(move |_, _| dismissed.set(dismissed.get() + 1))
                    .body(div().h(px(40.0)))
                    .actions(vec![
                        Button::new("cancel", "Cancel")
                            .on_click(move |_, _, _| cancelled.set(cancelled.get() + 1)),
                        Button::new("create", "Create")
                            .style(ButtonStyle::Primary)
                            .action(Box::new(Create)),
                    ]),
            )
    }
}

fn host(cx: &mut gpui::TestAppContext) -> (WindowHandle<Host>, VisualTestContext) {
    harness::set_recording(true);
    cx.update(|cx| cx.set_global(crate::Theme::dark()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                Host {
                    focus,
                    dismissed: Rc::new(Cell::new(0)),
                    created: 0,
                }
            })
        })
        .expect("test window")
    });
    let visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    (window, visual)
}

#[track_caller]
fn center_of(cx: &mut VisualTestContext, name: &str) -> Point<gpui::Pixels> {
    let targets: Vec<RecordedTarget> = cx.update(|window, _| harness::painted(window));
    let target = targets
        .iter()
        .rev()
        .find(|target| target.name == name)
        .unwrap_or_else(|| panic!("{name} is not painted: {targets:?}"));
    point(
        px(target.rect.x + target.rect.w / 2.0),
        px(target.rect.y + target.rect.h / 2.0),
    )
}

fn click(cx: &mut VisualTestContext, at: Point<gpui::Pixels>) {
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn a_click_on_the_scrim_dismisses_and_a_click_on_the_card_does_not(cx: &mut gpui::TestAppContext) {
    let (window, mut cx) = host(cx);
    let view = window.root(&mut cx).expect("test host");

    let body = center_of(&mut cx, "dialog.button[0]");
    let above_the_footer = point(body.x, body.y - px(60.0));
    click(&mut cx, above_the_footer);
    view.read_with(&cx, |host, _| {
        assert_eq!(
            host.dismissed.get(),
            0,
            "a click inside the card stays inside"
        )
    });

    click(&mut cx, point(px(4.0), px(4.0)));
    view.read_with(&cx, |host, _| {
        assert_eq!(host.dismissed.get(), 1, "a click on the scrim dismisses")
    });

    let close = center_of(&mut cx, "dialog.close");
    click(&mut cx, close);
    view.read_with(&cx, |host, _| {
        assert_eq!(host.dismissed.get(), 2, "the ✕ runs the same dismiss")
    });
    harness::set_recording(false);
}

#[gpui::test]
fn footer_buttons_paint_left_to_right_and_the_primary_runs_its_action(
    cx: &mut gpui::TestAppContext,
) {
    let (window, mut cx) = host(cx);
    let view = window.root(&mut cx).expect("test host");

    let cancel = center_of(&mut cx, "dialog.button[0]");
    let create = center_of(&mut cx, "dialog.button[1]");
    assert!(cancel.x < create.x, "0 is the leftmost button");

    click(&mut cx, create);
    view.read_with(&cx, |host, _| {
        assert_eq!(
            host.created, 1,
            "dialog.button[1] dispatches the primary action"
        );
        assert_eq!(host.dismissed.get(), 0, "and does not dismiss");
    });

    click(&mut cx, cancel);
    view.read_with(&cx, |host, _| {
        assert_eq!(host.created, 1);
        assert_eq!(host.dismissed.get(), 1, "Cancel ran its own handler");
    });
    harness::set_recording(false);
}
