use super::*;
use gpui::{AppContext, TestAppContext};
use std::time::Instant;

#[test]
fn clear_removes_mouse_and_keyboard_selections_together() {
    let point = AbsoluteCellPoint::new(0, 0);
    let mut surface = TerminalSurface::<()> {
        anchor: Some(0),
        anchor_history_epoch: Some(1),
        anchor_cols: Some(80),
        anchor_alt_screen: Some(false),
        mouse_selection: Some(MouseSelection {
            anchor: point,
            head: point,
            initial: AbsoluteCellSelection::new(point, point),
            initiating: point,
            granularity: SelectionGranularity::Cell,
            history_epoch: 1,
            cols: 80,
            alt_screen: false,
            dragging: true,
            selected: true,
        }),
        ..TerminalSurface::default()
    };
    assert!(surface.clear_selections());
    assert!(surface.mouse_selection.is_none());
    assert!(surface.anchor.is_none());
    assert!(surface.anchor_history_epoch.is_none());
    assert!(surface.anchor_cols.is_none());
    assert!(surface.anchor_alt_screen.is_none());
    assert!(!surface.clear_selections());
}

#[gpui::test]
fn prefix_hints_are_cancelled_on_exit_and_restart_their_delay(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::for_mode(fleet_ui_kit::ThemeMode::Dark)));
    let state = cx.new(|_| AppState::new("/tmp/fleet-prefix-test", Instant::now()));
    let mut hint = PrefixHintState::default();
    cx.update(|cx| hint.reconcile(true, &state, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.update(|cx| hint.reconcile(false, &state, cx));
    cx.update(|cx| hint.reconcile(true, &state, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(!hint.visible());
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(hint.visible());
    cx.update(|cx| hint.reconcile(false, &state, cx));
    assert!(!hint.visible());
}

#[gpui::test]
fn dropping_prefix_owner_cancels_the_pending_reveal(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::for_mode(fleet_ui_kit::ThemeMode::Dark)));
    let state = cx.new(|_| AppState::new("/tmp/fleet-prefix-test", Instant::now()));
    let mut hint = PrefixHintState::default();
    let visible = Rc::clone(&hint.visible);
    cx.update(|cx| hint.reconcile(true, &state, cx));
    cx.run_until_parked();
    drop(hint);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!visible.get());
}

#[test]
fn pointer_motion_inside_the_same_cell_does_not_change_selection() {
    let point = AbsoluteCellPoint::new(0, 0);
    let initial = AbsoluteCellSelection::new(point, point);
    let mut selection = MouseSelection {
        anchor: point,
        head: point,
        initial,
        initiating: point,
        granularity: SelectionGranularity::Cell,
        history_epoch: 0,
        cols: 80,
        alt_screen: false,
        dragging: true,
        selected: false,
    };
    assert!(!selection.extend(initial, point));
    let end = AbsoluteCellPoint::new(0, 1);
    let extended = AbsoluteCellSelection::new(point, end);
    assert!(selection.extend(extended, end));
    assert!(!selection.extend(extended, end));
    assert!(selection.selected);
}
