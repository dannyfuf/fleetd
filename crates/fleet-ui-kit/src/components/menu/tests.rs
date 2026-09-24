use std::{cell::Cell, rc::Rc};

use gpui::{
    AppContext, Context, FocusHandle, Modifiers, MouseButton, Pixels, Point, Render,
    VisualTestContext, WindowHandle, point, px,
};

use super::*;
use crate::components::Button;

gpui::actions!(menu_test, [First, Second, Third, Unhandled]);

/// The trigger sits at the window's top-left corner; the context area below it.
const TRIGGER: Point<Pixels> = point(px(12.0), px(12.0));
const AREA: Point<Pixels> = point(px(40.0), px(300.0));
const OUTSIDE: Point<Pixels> = point(px(600.0), px(500.0));

fn verbs(menu: Menu) -> Menu {
    menu.item(MenuItem::new("First").action(Box::new(First)))
        .item(MenuItem::new("Second").action(Box::new(Second)))
        .separator()
        .item(
            MenuItem::new("Third")
                .action(Box::new(Third))
                .destructive(true),
        )
        // Nothing handles this action, so the menu leaves it out.
        .item(MenuItem::new("Not here").action(Box::new(Unhandled)))
}

struct Host {
    focus: FocusHandle,
    ran: Vec<&'static str>,
    chosen: Rc<Cell<usize>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.chosen.clone();
        div()
            .key_context("MenuTest")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .on_action(cx.listener(|host, _: &First, _, _| host.ran.push("first")))
            .on_action(cx.listener(|host, _: &Second, _, _| host.ran.push("second")))
            .on_action(cx.listener(|host, _: &Third, _, _| host.ran.push("third")))
            .child(
                PopoverMenu::new("popover")
                    .trigger(Button::new("open", "Open"))
                    .menu(|menu, _, _| verbs(menu)),
            )
            .child(
                div()
                    .mt(px(200.0))
                    .child(Dropdown::new("dropdown", "Option").menu(move |menu, _, _| {
                        (0..3).fold(menu, |menu, ix| {
                            let chosen = chosen.clone();
                            menu.item(
                                MenuItem::new(format!("Option {ix}"))
                                    .checked(chosen.get() == ix)
                                    .on_select(move |_, _| chosen.set(ix)),
                            )
                        })
                    })),
            )
            .child(
                ContextMenu::new("area", div().w(px(300.0)).h(px(100.0)))
                    .menu(|menu, _, _| verbs(menu)),
            )
    }
}

fn host(cx: &mut gpui::TestAppContext) -> (WindowHandle<Host>, VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(crate::Theme::dark());
        cx.bind_keys(menu_key_bindings());
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                Host {
                    focus,
                    ran: Vec::new(),
                    chosen: Rc::new(Cell::new(0)),
                }
            })
        })
        .expect("test window")
    });
    let visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    (window, visual)
}

/// Whether a menu is on screen holding the focus, or gone with the focus back on the host.
#[track_caller]
fn assert_menu_open(window: WindowHandle<Host>, cx: &mut VisualTestContext, open: bool) {
    let view = window.root(cx).expect("test host");
    let focus = view.read_with(cx, |host, _| host.focus.clone());
    let (host_focused, menu_focused) =
        cx.update(|window, cx| (focus.is_focused(window), menu_holds_focus(window, cx)));
    assert_eq!(menu_focused, open, "menu_holds_focus: {menu_focused}");
    let painted = cx.debug_bounds("fleet-menu").is_some();
    assert_eq!(painted, open, "a menu is painted: {painted}");
    assert_eq!(
        !host_focused, open,
        "the host has the focus: {host_focused}"
    );
}

fn ran(window: WindowHandle<Host>, cx: &mut VisualTestContext) -> Vec<&'static str> {
    let view = window.root(cx).expect("test host");
    view.read_with(cx, |host, _| host.ran.clone())
}

fn click(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn clicking_the_trigger_opens_and_down_down_enter_runs_the_third_item(
    cx: &mut gpui::TestAppContext,
) {
    let (window, mut cx) = host(cx);
    click(&mut cx, TRIGGER);
    assert_menu_open(window, &mut cx, true);
    // Hung from the trigger's bottom-left corner, one `xs` below it.
    let theme = cx.update(|_, cx| cx.theme().clone());
    let menu = cx.debug_bounds("fleet-menu").expect("menu painted");
    assert_eq!(
        menu.origin,
        point(px(0.0), theme.metrics.button_h + theme.space.xs)
    );

    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    assert_eq!(ran(window, &mut cx), ["third"]);
    assert_menu_open(window, &mut cx, false);
}

#[gpui::test]
fn the_highlight_stops_at_the_last_item_and_skips_what_was_left_out(cx: &mut gpui::TestAppContext) {
    let (window, mut cx) = host(cx);
    click(&mut cx, TRIGGER);
    // Four downs from the first item: past the separator, and "Not here" is not there to land on.
    cx.simulate_keystrokes("ctrl-n down down down enter");
    cx.run_until_parked();
    assert_eq!(ran(window, &mut cx), ["third"]);

    click(&mut cx, TRIGGER);
    cx.simulate_keystrokes("down up ctrl-p enter");
    cx.run_until_parked();
    assert_eq!(ran(window, &mut cx), ["third", "first"]);
}

#[gpui::test]
fn escape_closes_and_gives_the_focus_back(cx: &mut gpui::TestAppContext) {
    let (window, mut cx) = host(cx);
    click(&mut cx, TRIGGER);
    assert_menu_open(window, &mut cx, true);

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_menu_open(window, &mut cx, false);

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        ran(window, &mut cx).is_empty(),
        "a closed menu runs nothing"
    );
}

#[gpui::test]
fn a_click_outside_closes_without_running_anything(cx: &mut gpui::TestAppContext) {
    let (window, mut cx) = host(cx);
    click(&mut cx, TRIGGER);
    assert_menu_open(window, &mut cx, true);

    click(&mut cx, OUTSIDE);
    assert_menu_open(window, &mut cx, false);
    assert!(ran(window, &mut cx).is_empty());
}

#[gpui::test]
fn clicking_the_trigger_of_an_open_menu_closes_it(cx: &mut gpui::TestAppContext) {
    let (window, mut cx) = host(cx);
    click(&mut cx, TRIGGER);
    assert_menu_open(window, &mut cx, true);

    click(&mut cx, TRIGGER);
    assert_menu_open(window, &mut cx, false);

    click(&mut cx, TRIGGER);
    assert_menu_open(window, &mut cx, true);
}

#[gpui::test]
fn a_right_click_opens_at_the_pointer_and_a_hovered_item_runs_on_click(
    cx: &mut gpui::TestAppContext,
) {
    let (window, mut cx) = host(cx);
    cx.simulate_mouse_move(AREA, None, Modifiers::none());
    cx.simulate_mouse_down(AREA, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(AREA, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert_menu_open(window, &mut cx, true);
    let menu = cx.debug_bounds("fleet-menu").expect("menu painted");
    assert_eq!(menu.origin, AREA, "the menu hangs from the pointer");

    // Its second item is one row below the first.
    let theme = cx.update(|_, cx| cx.theme().clone());
    let inset = theme.space.xs + theme.metrics.hairline;
    let second = point(
        AREA.x + px(24.0),
        AREA.y + inset + theme.metrics.row_h * 1.5,
    );
    cx.simulate_mouse_move(second, None, Modifiers::none());
    cx.run_until_parked();
    // Hover moved the highlight, so the key runs the same item the pointer is on.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(ran(window, &mut cx), ["second"]);

    cx.simulate_mouse_down(AREA, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(AREA, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    click(&mut cx, second);
    assert_eq!(ran(window, &mut cx), ["second", "second"]);
    assert_menu_open(window, &mut cx, false);
}

#[gpui::test]
fn a_dropdown_opens_on_its_chosen_option_and_selecting_another_runs_its_handler(
    cx: &mut gpui::TestAppContext,
) {
    let (window, mut cx) = host(cx);
    let view = window.root(&mut cx).expect("test host");
    let chosen = view.read_with(&cx, |host, _| host.chosen.clone());
    chosen.set(1);
    let field = point(px(40.0), px(230.0) + px(18.0));
    click(&mut cx, field);
    assert_menu_open(window, &mut cx, true);

    // Opened on "Option 1"; one down is "Option 2".
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(chosen.get(), 2);
    assert_menu_open(window, &mut cx, false);
}

#[gpui::test]
fn build_leaves_out_unavailable_items_and_the_breaks_around_them(cx: &mut gpui::TestAppContext) {
    let (_window, mut cx) = host(cx);
    let menu = cx.update(|window, cx| {
        Menu::build(window, cx, |menu, _, _| {
            menu.header("Nothing here")
                .item(MenuItem::new("Not here").action(Box::new(Unhandled)))
                .separator()
                .separator()
                .header("Run")
                .item(MenuItem::new("First").action(Box::new(First)))
                .separator()
                .item(MenuItem::new("Pick").on_select(|_, _| {}))
                .separator()
        })
    });
    let shape = cx.update(|_, cx| {
        menu.read(cx)
            .entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(item) => item.label.to_string(),
                MenuEntry::Separator => "--".to_owned(),
                MenuEntry::Header(label) => format!("# {label}"),
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(shape, ["# Run", "First", "--", "Pick"]);
    let (labels, highlighted) = cx.update(|_, cx| {
        let menu = menu.read(cx);
        (menu.item_labels(), menu.highlighted())
    });
    assert_eq!(labels, ["First", "Pick"]);
    assert_eq!(highlighted, Some(0));
}
