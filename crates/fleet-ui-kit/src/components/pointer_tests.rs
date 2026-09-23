//! The pointer contract of the list primitives (ADR 0023, UX-SPEC §5.1), driven through real
//! mouse events in a test window: a press selects, a double-click opens, a right click opens
//! the menu, a fuzzy row runs on a press, and a fuzzy list scrolls past its old cap. And of the
//! form controls: a segment, a switch and a dropdown-drawn cycler's option each report a click
//! with the value it asks for.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AppContext as _, Context, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    Styled as _, TestAppContext, TouchPhase, VisualTestContext, Window, div, point, px,
};

use crate::{
    components::{
        Cycler, FuzzyItem, FuzzyList, ListView, Row, RowColumn, Segment, SegmentedControl, Switch,
        Toggle, menu_key_bindings,
    },
    harness::{self, HarnessTargetExt as _},
    text::Text,
    theme::Theme,
};

/// Everything the pointer asked the list for, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asked {
    Select(usize),
    Open(usize),
    Menu(usize),
    Run(usize),
}

type Log = Rc<RefCell<Vec<Asked>>>;

/// A ten-row `ListView` whose handlers only record what they were asked, the way a screen's
/// `on_select` / `on_open` / `on_menu` would dispatch.
struct PointerList {
    cursor: usize,
    log: Log,
}

impl Render for PointerList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        let this = cx.entity().downgrade();
        let (open, menu) = (self.log.clone(), self.log.clone());
        let select_log = self.log.clone();
        div().size_full().child(
            ListView::new("pointer-list", 10, |ix, is_cursor, _, _| {
                Row::new()
                    .selected(is_cursor)
                    .column(RowColumn::flex(Text::ui("row")))
                    .harness_target_indexed("test.row", ix)
                    .into_any_element()
            })
            .cursor(self.cursor)
            .on_select(move |ix, _, cx| {
                select_log.borrow_mut().push(Asked::Select(ix));
                if let Some(list) = this.upgrade() {
                    list.update(cx, |list, cx| {
                        list.cursor = ix;
                        cx.notify();
                    });
                }
            })
            .on_open(move |ix, _, _| open.borrow_mut().push(Asked::Open(ix)))
            .on_menu(move |ix, _, _, _| menu.borrow_mut().push(Asked::Menu(ix))),
        )
    }
}

/// A twenty-row fuzzy list, eight rows tall — the old cap — scrolling the rest.
struct LongFuzzy {
    scroll: ScrollHandle,
    log: Log,
}

/// The rows the old cap let a fuzzy list show.
const OLD_CAP: usize = 8;

impl Render for LongFuzzy {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        let log = self.log.clone();
        div().size_full().child(
            FuzzyList::new(
                "long-fuzzy",
                (0..20).map(|ix| FuzzyItem::new(format!("branch-{ix:02}"))),
            )
            .visible_rows(OLD_CAP)
            .track_scroll(&self.scroll)
            .harness_rows("test.fuzzy", 0)
            .on_click(move |ix, _, _| log.borrow_mut().push(Asked::Run(ix))),
        )
    }
}

fn open<V: Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce() -> V + 'static,
) -> VisualTestContext {
    harness::set_recording(true);
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| build()))
            .expect("test window")
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx
}

/// The centre of the painted target `name`.
#[track_caller]
fn centre(cx: &mut VisualTestContext, name: &str) -> Point<Pixels> {
    let targets = cx.update(|window, _| harness::painted(window));
    let target = targets
        .iter()
        .find(|target| target.name.as_ref() == name)
        .unwrap_or_else(|| panic!("{name} was not painted"));
    point(
        px(target.rect.x + target.rect.w / 2.0),
        px(target.rect.y + target.rect.h / 2.0),
    )
}

fn press(cx: &mut VisualTestContext, at: Point<Pixels>, button: MouseButton, click_count: usize) {
    cx.simulate_event(MouseDownEvent {
        button,
        position: at,
        modifiers: Modifiers::none(),
        click_count,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button,
        position: at,
        modifiers: Modifiers::none(),
        click_count,
    });
    cx.run_until_parked();
}

#[gpui::test]
fn a_click_on_row_three_moves_the_cursor_there_and_does_not_open(cx: &mut TestAppContext) {
    let log = Log::default();
    let recorded = log.clone();
    let mut cx = open(cx, move || PointerList { cursor: 0, log });

    let row = centre(&mut cx, "test.row[3]");
    press(&mut cx, row, MouseButton::Left, 1);

    assert_eq!(*recorded.borrow(), vec![Asked::Select(3)]);
}

#[gpui::test]
fn a_double_click_selects_then_opens(cx: &mut TestAppContext) {
    let log = Log::default();
    let recorded = log.clone();
    let mut cx = open(cx, move || PointerList { cursor: 0, log });

    let row = centre(&mut cx, "test.row[5]");
    press(&mut cx, row, MouseButton::Left, 1);
    press(&mut cx, row, MouseButton::Left, 2);

    assert_eq!(
        *recorded.borrow(),
        vec![Asked::Select(5), Asked::Select(5), Asked::Open(5)],
        "the first press selects, the second press of the pair opens the cursor row"
    );
}

#[gpui::test]
fn a_right_click_selects_then_asks_for_the_menu(cx: &mut TestAppContext) {
    let log = Log::default();
    let recorded = log.clone();
    let mut cx = open(cx, move || PointerList { cursor: 0, log });

    let row = centre(&mut cx, "test.row[2]");
    press(&mut cx, row, MouseButton::Right, 1);

    assert_eq!(*recorded.borrow(), vec![Asked::Select(2), Asked::Menu(2)]);
}

#[gpui::test]
fn a_click_on_a_fuzzy_row_runs_it(cx: &mut TestAppContext) {
    let log = Log::default();
    let recorded = log.clone();
    let mut cx = open(cx, move || LongFuzzy {
        scroll: ScrollHandle::new(),
        log,
    });

    let row = centre(&mut cx, "test.fuzzy[4]");
    press(&mut cx, row, MouseButton::Left, 1);

    assert_eq!(*recorded.borrow(), vec![Asked::Run(4)]);
}

#[gpui::test]
fn the_fuzzy_list_wheel_scrolls_past_the_old_cap(cx: &mut TestAppContext) {
    let scroll = ScrollHandle::new();
    let handle = scroll.clone();
    let mut cx = open(cx, move || LongFuzzy {
        scroll,
        log: Log::default(),
    });

    // Child bounds are laid out unscrolled; the viewport slides over them by the offset.
    let past_the_cap = |handle: &ScrollHandle| {
        let row = handle
            .bounds_for_item(OLD_CAP)
            .expect("the row past the old cap is rendered");
        let viewport_bottom = handle.bounds().bottom() - handle.offset().y;
        row.bottom() <= viewport_bottom
    };
    assert!(
        !past_the_cap(&handle),
        "the list is exactly the old cap tall, so row {OLD_CAP} starts hidden"
    );
    assert!(
        handle.max_offset().y > px(0.0),
        "the rest must be scrollable"
    );

    let over = centre(&mut cx, "test.fuzzy[2]");
    cx.simulate_event(ScrollWheelEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(0.0), -handle.bounds().size.height)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();

    assert!(
        past_the_cap(&handle),
        "the wheel must reveal row {OLD_CAP}, past the old cap (offset {:?})",
        handle.offset()
    );
}

/// Two rows, each with a named hover action: the action exists for the pointer only on the
/// row under it, or on the selected row.
struct HoverRows;

impl Render for HoverRows {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        div()
            .flex()
            .flex_col()
            .size_full()
            .children((0..3).map(|ix| {
                Row::new()
                    .selected(ix == 2)
                    .column(RowColumn::flex(Text::ui("row")))
                    .hover_actions(
                        div()
                            .w(px(40.0))
                            .h(px(10.0))
                            .harness_target_indexed("test.action", ix),
                    )
                    .harness_target_indexed("test.row", ix)
            }))
    }
}

fn painted_actions(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| harness::painted(window))
        .into_iter()
        .map(|target| target.name.to_string())
        .filter(|name| name.starts_with("test.action"))
        .collect()
}

#[gpui::test]
fn hover_actions_show_only_on_the_hovered_or_selected_row(cx: &mut TestAppContext) {
    let mut cx = open(cx, || HoverRows);

    let second = centre(&mut cx, "test.row[1]");
    cx.simulate_mouse_move(second, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        painted_actions(&mut cx),
        vec!["test.action[1]", "test.action[2]"],
        "the hovered row and the selected row show their actions; row 0 hides them"
    );

    let first = centre(&mut cx, "test.row[0]");
    cx.simulate_mouse_move(first, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        painted_actions(&mut cx),
        vec!["test.action[0]", "test.action[2]"],
        "the actions follow the pointer and never leave the selected row"
    );
}

/// One of each form control, every click recorded as the index or value it asks for.
struct Controls {
    log: Rc<RefCell<Vec<String>>>,
    disabled: bool,
}

impl Render for Controls {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        let (segment, switch, toggle, cycler) = (
            self.log.clone(),
            self.log.clone(),
            self.log.clone(),
            self.log.clone(),
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                SegmentedControl::new(
                    "segments",
                    [Segment::new("a"), Segment::new("b"), Segment::new("c")],
                )
                .disabled(self.disabled)
                .harness_segments("test.segment")
                .on_select(move |ix, _, _| segment.borrow_mut().push(format!("segment {ix}"))),
            )
            .child(
                Switch::new("switch", false)
                    .on_toggle(move |on, _, _| switch.borrow_mut().push(format!("switch {on}")))
                    .harness_target("test.switch"),
            )
            .child(
                div().w(px(300.0)).child(
                    Toggle::labeled("toggle", true)
                        .on_toggle(move |on, _, _| toggle.borrow_mut().push(format!("toggle {on}")))
                        .harness_target("test.toggle"),
                ),
            )
            .child(
                div().w(px(300.0)).child(
                    Cycler::labeled("steps", "c")
                        .options(["a", "b", "c", "d", "e"])
                        .on_select(move |ix, _, _| cycler.borrow_mut().push(format!("cycler {ix}")))
                        .harness_target("test.cycler"),
                ),
            )
    }
}

/// A point just inside the right edge of the painted target `name`, where a row's control sits.
#[track_caller]
fn right_edge(cx: &mut VisualTestContext, name: &str) -> Point<Pixels> {
    let targets = cx.update(|window, _| harness::painted(window));
    let target = targets
        .iter()
        .find(|target| target.name.as_ref() == name)
        .unwrap_or_else(|| panic!("{name} was not painted"));
    point(
        px(target.rect.x + target.rect.w - 24.0),
        px(target.rect.y + target.rect.h / 2.0),
    )
}

fn open_controls(
    cx: &mut TestAppContext,
    disabled: bool,
) -> (VisualTestContext, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let recorded = log.clone();
    cx.update(|cx| cx.bind_keys(menu_key_bindings()));
    let cx = open(cx, move || Controls { log, disabled });
    (cx, recorded)
}

/// A decision dock whose controls only record the action they report.
struct Dock {
    decision: crate::components::Decision,
    log: Rc<RefCell<Vec<crate::components::DecisionAction>>>,
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        let log = self.log.clone();
        div().size_full().child(
            crate::components::DecisionDock::new(self.decision.clone())
                .on_action(move |action, _, _| log.borrow_mut().push(action)),
        )
    }
}

fn dock(
    cx: &mut TestAppContext,
    decision: crate::components::Decision,
) -> (
    VisualTestContext,
    Rc<RefCell<Vec<crate::components::DecisionAction>>>,
) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let recorded = log.clone();
    let cx = open(cx, move || Dock { decision, log });
    (cx, recorded)
}

#[gpui::test]
fn a_click_on_a_segment_asks_for_its_index(cx: &mut TestAppContext) {
    let (mut cx, log) = open_controls(cx, false);
    let at = centre(&mut cx, "test.segment[2]");
    press(&mut cx, at, MouseButton::Left, 1);
    assert_eq!(*log.borrow(), vec!["segment 2".to_owned()]);
}

#[gpui::test]
fn a_disabled_segmented_control_ignores_the_pointer(cx: &mut TestAppContext) {
    let (mut cx, log) = open_controls(cx, true);
    let at = centre(&mut cx, "test.segment[1]");
    press(&mut cx, at, MouseButton::Left, 1);
    assert!(log.borrow().is_empty());
}

#[gpui::test]
fn a_click_on_a_switch_asks_for_the_other_value(cx: &mut TestAppContext) {
    let (mut cx, log) = open_controls(cx, false);
    let at = centre(&mut cx, "test.switch");
    press(&mut cx, at, MouseButton::Left, 1);
    let at = right_edge(&mut cx, "test.toggle");
    press(&mut cx, at, MouseButton::Left, 1);
    assert_eq!(
        *log.borrow(),
        vec!["switch true".to_owned(), "toggle false".to_owned()]
    );
}

#[gpui::test]
fn a_long_cycler_lists_its_options_and_a_click_picks_one(cx: &mut TestAppContext) {
    let (mut cx, log) = open_controls(cx, false);
    let field = right_edge(&mut cx, "test.cycler");
    press(&mut cx, field, MouseButton::Left, 1);
    let option = centre(&mut cx, "menu.item[3]");
    press(&mut cx, option, MouseButton::Left, 1);
    assert_eq!(*log.borrow(), vec!["cycler 3".to_owned()]);
}

#[gpui::test]
fn an_approval_button_reports_the_action_its_key_resolves_to(cx: &mut TestAppContext) {
    use crate::components::{ApprovalRequest, Decision, DecisionAction, DecisionKind};
    let decision = Decision::new(
        "g1",
        "codex wants to edit README.md",
        DecisionKind::Approval(ApprovalRequest::new("edit", "apply the edit to README.md")),
    );
    for (target, key) in [
        ("agents.approval.allow_once", "y"),
        ("agents.approval.allow_always", "a"),
        ("agents.approval.deny", "n"),
        ("agents.approval.deny_and_stop", "escape"),
    ] {
        let (mut cx, log) = dock(cx, decision.clone());
        let at = centre(&mut cx, target);
        press(&mut cx, at, MouseButton::Left, 1);
        let expected: Vec<DecisionAction> = decision.action_for_key(key).into_iter().collect();
        assert!(!expected.is_empty(), "`{key}` answers an approval");
        assert_eq!(
            *log.borrow(),
            expected,
            "{target} reports what `{key}` does"
        );
    }
}

#[gpui::test]
fn a_decision_in_flight_reports_nothing(cx: &mut TestAppContext) {
    use crate::components::{ApprovalRequest, Decision, DecisionKind};
    let decision = Decision::new(
        "g1",
        "codex wants to run a command",
        DecisionKind::Approval(ApprovalRequest::new("bash", "cargo test")),
    )
    .answering(true);
    let (mut cx, log) = dock(cx, decision);
    let at = centre(&mut cx, "agents.approval.allow_once");
    press(&mut cx, at, MouseButton::Left, 1);
    assert!(
        log.borrow().is_empty(),
        "an answer in flight is not sent twice"
    );
}
