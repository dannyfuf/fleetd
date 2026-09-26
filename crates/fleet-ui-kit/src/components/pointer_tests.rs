//! The pointer contract of the list primitives (ADR 0023, UX-SPEC §5.1), driven through real
//! mouse events in a test window: a press selects, a double-click opens, a right click opens
//! the menu, a fuzzy row runs on a press, and a fuzzy list scrolls past its old cap. And of the
//! form controls: a segment, a switch, a value box and a dropdown-drawn cycler's option (in its
//! row chrome and inline) each report a click with the value it asks for.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AppContext as _, Context, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    Styled as _, TestAppContext, TouchPhase, VisualTestContext, Window, div, point, px,
};

use crate::{
    components::{
        Cycler, FuzzyItem, FuzzyList, InputMode, ListPointer, ListView, NavItem, Row, RowColumn,
        Segment, SegmentedControl, SettingsRow, Sidebar, SidebarSection, Switch, TextInput,
        ValueBox, menu_key_bindings,
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

/// Settings rows whose handlers only record what they were asked: a cursor row with a hover
/// action, a row whose box holds an open editor, and two disabled rows with a hover action —
/// one the cursor, one not.
struct SlotRows {
    editor: Option<gpui::Entity<TextInput>>,
    log: Log,
}

impl Render for SlotRows {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        let editor = self
            .editor
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let mut input = TextInput::new(InputMode::SingleLine, cx);
                    input.set_embedded(true, cx);
                    input
                })
            })
            .clone();
        let row = |ix: usize, log: &Log| {
            let (select, open) = (log.clone(), log.clone());
            SettingsRow::new(("slot-row", ix))
                .label("row")
                .on_click(move |_, _, _| select.borrow_mut().push(Asked::Select(ix)))
                .on_double_click(move |_, _, _| open.borrow_mut().push(Asked::Open(ix)))
        };
        let action = |ix: usize| {
            div()
                .w(px(40.0))
                .h(px(10.0))
                .harness_target_indexed("test.action", ix)
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(row(0, &self.log).cursor(true).hover_actions(action(0)))
            .child(
                row(1, &self.log).control(
                    ValueBox::new("slot-box", "")
                        .editor(editor)
                        .harness_target("test.editor_box"),
                ),
            )
            .child(
                row(2, &self.log)
                    .cursor(true)
                    .disabled(true)
                    .hover_actions(action(2)),
            )
            .child(row(3, &self.log).disabled(true).hover_actions(action(3)))
    }
}

fn slot_rows(cx: &mut TestAppContext) -> (VisualTestContext, Log) {
    let log = Log::default();
    let recorded = log.clone();
    let cx = open(cx, move || SlotRows { editor: None, log });
    (cx, recorded)
}

/// Regression (ADR 0023): the second press of a quick double click on a hover action — *Move
/// up* twice — reached the row and opened it; after the first *Move up* that row was already
/// another column. A press on a slot lands the cursor as any press does and never opens.
#[gpui::test]
fn a_double_click_on_a_hover_action_lands_the_cursor_and_never_opens_the_row(
    cx: &mut TestAppContext,
) {
    let (mut cx, log) = slot_rows(cx);
    let at = centre(&mut cx, "test.action[0]");
    press(&mut cx, at, MouseButton::Left, 1);
    press(&mut cx, at, MouseButton::Left, 2);
    assert_eq!(*log.borrow(), vec![Asked::Select(0)]);
}

/// Regression: a box drawn with an editor and no `on_click` let the second press of a double
/// click through to the row, whose open handler closed the editor it was meant to select a word
/// in. A press inside a box is the box's whether it opens the editor or places the caret.
#[gpui::test]
fn a_double_click_inside_an_open_editor_never_reaches_the_row(cx: &mut TestAppContext) {
    let (mut cx, log) = slot_rows(cx);
    let at = centre(&mut cx, "test.editor_box");
    press(&mut cx, at, MouseButton::Left, 1);
    press(&mut cx, at, MouseButton::Left, 2);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

/// A disabled row cannot be hovered, so its hover actions show on the cursor row or nowhere: a
/// disabled schedule's *Run now* is still `r`'s pointer twin.
#[gpui::test]
fn a_disabled_cursor_row_still_paints_its_hover_actions(cx: &mut TestAppContext) {
    let (mut cx, _) = slot_rows(cx);
    assert_eq!(
        painted_actions(&mut cx),
        vec!["test.action[0]", "test.action[2]"],
        "the cursor rows show theirs, disabled or not; the disabled row off the cursor hides its own"
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
        let (segment, switch, value, cycler, inline) = (
            self.log.clone(),
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
                div().flex().child(
                    ValueBox::new("value", "250")
                        .unit("ms")
                        .on_click(move |_, _| value.borrow_mut().push("value box".to_owned()))
                        .harness_target("test.value_box"),
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
            .child(
                div().flex().child(
                    Cycler::new("c")
                        .id("inline-steps")
                        .options(["a", "b", "c", "d", "e"])
                        .inline(true)
                        .harness("test.inline_segment", "test.inline_dropdown")
                        .on_select(move |ix, _, _| {
                            inline.borrow_mut().push(format!("inline {ix}"))
                        }),
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
    assert_eq!(*log.borrow(), vec!["switch true".to_owned()]);
}

#[gpui::test]
fn a_click_on_a_value_box_asks_to_open_its_editor(cx: &mut TestAppContext) {
    let (mut cx, log) = open_controls(cx, false);
    let at = centre(&mut cx, "test.value_box");
    press(&mut cx, at, MouseButton::Left, 1);
    assert_eq!(*log.borrow(), vec!["value box".to_owned()]);
}

#[gpui::test]
fn an_inline_cycler_draws_the_dropdown_alone_and_a_click_picks_an_option(cx: &mut TestAppContext) {
    let (mut cx, log) = open_controls(cx, false);
    let field = centre(&mut cx, "test.inline_dropdown");
    press(&mut cx, field, MouseButton::Left, 1);
    let option = centre(&mut cx, "menu.item[1]");
    press(&mut cx, option, MouseButton::Left, 1);
    assert_eq!(*log.borrow(), vec!["inline 1".to_owned()]);
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

/// Three sidebar items routed through one `ListPointer`, the second with a hover action, in a
/// sidebar whose edge reports every width a drag asks for.
struct PointerSidebar {
    log: Log,
    widths: Rc<RefCell<Vec<Pixels>>>,
}

impl Render for PointerSidebar {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        harness::begin_frame(window);
        let (select, open, menu) = (self.log.clone(), self.log.clone(), self.log.clone());
        let pointer = ListPointer::new()
            .on_select(move |ix, _, _| select.borrow_mut().push(Asked::Select(ix)))
            .on_open(move |ix, _, _| open.borrow_mut().push(Asked::Open(ix)))
            .on_menu(move |ix, _, _, _| menu.borrow_mut().push(Asked::Menu(ix)));
        let widths = self.widths.clone();
        div().flex().size_full().child(
            Sidebar::new("pointer-sidebar")
                .section(
                    SidebarSection::new("Items").body(div().flex().flex_col().children(
                        (0..3).map(|ix| {
                            let item = NavItem::new(("nav", ix), "item").pointer(&pointer, ix);
                            let item = if ix == 1 {
                                item.hover_action(
                                    div()
                                        .w(px(20.0))
                                        .h(px(10.0))
                                        .harness_target_indexed("test.nav_action", ix),
                                )
                            } else {
                                item
                            };
                            item.harness_target_indexed("test.nav", ix)
                        }),
                    )),
                )
                .on_resize(move |width, _, _| widths.borrow_mut().push(width))
                .handle_target("test.edge"),
        )
    }
}

#[gpui::test]
fn a_nav_item_selects_on_a_click_opens_on_a_double_click_and_asks_for_its_menu(
    cx: &mut TestAppContext,
) {
    let log = Log::default();
    let recorded = log.clone();
    let mut cx = open(cx, move || PointerSidebar {
        log,
        widths: Rc::default(),
    });

    let second = centre(&mut cx, "test.nav[2]");
    press(&mut cx, second, MouseButton::Left, 1);
    press(&mut cx, second, MouseButton::Left, 2);
    let first = centre(&mut cx, "test.nav[0]");
    press(&mut cx, first, MouseButton::Right, 1);

    assert_eq!(
        *recorded.borrow(),
        vec![
            Asked::Select(2),
            Asked::Select(2),
            Asked::Open(2),
            Asked::Select(0),
            Asked::Menu(0),
        ]
    );
}

#[gpui::test]
fn a_nav_items_hover_action_is_painted_only_while_the_pointer_is_on_it(cx: &mut TestAppContext) {
    let mut cx = open(cx, || PointerSidebar {
        log: Log::default(),
        widths: Rc::default(),
    });
    let painted = |cx: &mut VisualTestContext| {
        cx.update(|window, _| harness::painted(window))
            .into_iter()
            .any(|target| target.name.as_ref() == "test.nav_action[1]")
    };

    let away = centre(&mut cx, "test.nav[0]");
    cx.simulate_mouse_move(away, None, Modifiers::none());
    cx.run_until_parked();
    assert!(!painted(&mut cx), "hidden while another item is hovered");

    let over = centre(&mut cx, "test.nav[1]");
    cx.simulate_mouse_move(over, None, Modifiers::none());
    cx.run_until_parked();
    assert!(painted(&mut cx), "shown on the hovered item");
}

#[gpui::test]
fn dragging_the_sidebar_edge_reports_widths_clamped_to_the_drag_range(cx: &mut TestAppContext) {
    let widths = Rc::new(RefCell::new(Vec::new()));
    let reported = widths.clone();
    let mut cx = open(cx, move || PointerSidebar {
        log: Log::default(),
        widths,
    });
    let metrics = Theme::dark().metrics;

    let edge = centre(&mut cx, "test.edge");
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: edge,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    for x in [260.0, 300.0, 900.0, 20.0] {
        cx.simulate_mouse_move(point(px(x), edge.y), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: point(px(20.0), edge.y),
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    cx.run_until_parked();

    assert_eq!(
        reported.borrow().last().copied(),
        Some(metrics.sidebar_min_w),
        "a drag past the leading side stops at the minimum"
    );
    assert!(
        reported.borrow().contains(&px(300.0)),
        "the width follows the pointer: {:?}",
        reported.borrow()
    );
    assert!(
        reported.borrow().contains(&metrics.sidebar_max_w),
        "a drag far past the edge stops at the maximum: {:?}",
        reported.borrow()
    );
}
