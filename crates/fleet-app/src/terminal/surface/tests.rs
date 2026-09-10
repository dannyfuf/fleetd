use super::*;
use fleet_proto::error::{ErrorKind, ProtoError};
use gpui::{AppContext, TestAppContext};
use std::time::Instant;

fn pending_key(character: char) -> PendingInput {
    PendingInput::Key(KeyEvent {
        key: Key::Char(character),
        mods: Modifiers::empty(),
        text: Some(character.to_string()),
        action: KeyAction::Press,
    })
}

#[gpui::test]
fn preframe_input_is_bounded_and_ordered(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/fleet-input-bound-test", Instant::now()));
    let mut surface = TerminalSurface::<()>::default();
    for _ in 0..PENDING_INPUT_EVENT_CAP - 1 {
        assert!(surface.queue_pending(pending_key('a')));
    }
    assert!(surface.queue_pending(pending_key('b')));
    let accepted = surface.queue_pending(pending_key('c'));
    assert!(!accepted);
    cx.update(|cx| report_input_delivery(accepted, &state, cx));
    assert_eq!(surface.pending.len(), PENDING_INPUT_EVENT_CAP);
    assert!(
        matches!(surface.pending.first(), Some(PendingInput::Key(key)) if key.key == Key::Char('a'))
    );
    assert!(
        matches!(surface.pending.last(), Some(PendingInput::Key(key)) if key.key == Key::Char('b'))
    );

    surface.pending.clear();
    assert!(surface.queue_pending(PendingInput::Paste("x".repeat(PENDING_INPUT_BYTE_CAP))));
    let accepted = surface.queue_pending(pending_key('z'));
    assert!(!accepted);
    cx.update(|cx| report_input_delivery(accepted, &state, cx));
    assert_eq!(surface.pending.len(), 1);
    state.read_with(cx, |app, _| {
        assert_eq!(app.toasts.len(), 1, "overflow feedback is throttled");
        assert_eq!(app.toasts[0].toast.text.as_ref(), INPUT_REJECTION_NOTICE);
        assert_eq!(app.toasts[0].toast.icon, Some(Icon::Info));
    });
}

#[gpui::test]
fn failed_attachment_is_cleared_and_schedules_reconciliation(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/fleet-attach-test", Instant::now()));
    let terminal = TerminalId(7);
    let surface = Rc::new(RefCell::new(TerminalSurface::<()> {
        attached: Some(terminal),
        attached_generation: 3,
        attachment_attempt: 9,
        ..TerminalSurface::default()
    }));
    let (reply, answer) = async_channel::bounded(1);
    cx.update(|cx| {
        monitor_attachment(&surface, &state, terminal, 3, 9, answer, cx);
    });
    reply
        .try_send(Err(ProtoError {
            kind: ErrorKind::Unknown,
            message: "attach refused".to_owned(),
        }))
        .expect("deliver refusal");

    cx.run_until_parked();
    assert!(surface.borrow().attached.is_none());
    assert_eq!(surface.borrow().attached_generation, 0);
    assert!(surface.borrow().attach_retry_at.is_some());
    state.read_with(cx, |app, _| {
        assert!(app.sticky_error.as_ref().is_some_and(|error| {
            error.text.contains("attach refused") && error.text.contains("retrying")
        }));
    });

    cx.executor().advance_clock(ATTACH_RETRY_DELAY);
    cx.run_until_parked();
    assert!(surface.borrow().attach_retry_at.is_none());
}

#[gpui::test]
fn a_successful_retry_retires_the_attach_error(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/fleet-attach-retry", Instant::now()));
    let terminal = TerminalId(9);
    let surface = Rc::new(RefCell::new(TerminalSurface::<()> {
        attached: Some(terminal),
        attached_generation: 5,
        attachment_attempt: 11,
        ..TerminalSurface::default()
    }));
    let (reply, answer) = async_channel::bounded(1);
    cx.update(|cx| {
        monitor_attachment(&surface, &state, terminal, 5, 11, answer, cx);
    });
    reply
        .try_send(Err(ProtoError {
            kind: ErrorKind::Unknown,
            message: "attach refused".to_owned(),
        }))
        .expect("deliver refusal");
    cx.run_until_parked();
    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error
                .as_ref()
                .is_some_and(|error| error.text.starts_with("could not attach terminal 9: "))
        );
    });
    cx.executor().advance_clock(ATTACH_RETRY_DELAY);
    cx.run_until_parked();

    // The retry reconciles the surface and this time the daemon acknowledges.
    {
        let mut local = surface.borrow_mut();
        local.attached = Some(terminal);
        local.attached_generation = 5;
        local.attachment_attempt = 12;
    }
    let (reply, answer) = async_channel::bounded(1);
    cx.update(|cx| {
        monitor_attachment(&surface, &state, terminal, 5, 12, answer, cx);
    });
    reply
        .try_send(Ok(fleet_proto::response::ResponseBody::Ack))
        .expect("deliver acknowledgement");
    cx.run_until_parked();

    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error.is_none(),
            "a live terminal keeps no `retrying` notice"
        );
    });
}

#[gpui::test]
fn attachment_timeout_does_not_leave_the_surface_attached(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/fleet-attach-timeout", Instant::now()));
    let terminal = TerminalId(8);
    let surface = Rc::new(RefCell::new(TerminalSurface::<()> {
        attached: Some(terminal),
        attached_generation: 4,
        attachment_attempt: 10,
        ..TerminalSurface::default()
    }));
    let (_reply, answer) = async_channel::bounded(1);
    cx.update(|cx| {
        monitor_attachment(&surface, &state, terminal, 4, 10, answer, cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(ATTACH_TIMEOUT);
    cx.run_until_parked();

    assert!(surface.borrow().attached.is_none());
    assert_eq!(surface.borrow().attached_generation, 0);
    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error
                .as_ref()
                .is_some_and(|error| { error.text.contains("request timed out") })
        );
    });
}

#[test]
fn long_selection_stops_growing_history() {
    let mut surface = TerminalSurface::<()> {
        anchor: Some(0),
        ..TerminalSurface::default()
    };
    let mut grid = MirrorGrid::new(1, 1);

    for line in 0..MOUSE_ROW_CACHE_CAP + 10 {
        grid.seq = line as u64;
        grid.viewport.scrollback_len = line;
        surface.track_selection(&grid, false);
    }

    assert_eq!(surface.history.len(), MOUSE_ROW_CACHE_CAP);
    assert!(
        surface
            .history
            .contains_key(&(MOUSE_ROW_CACHE_CAP as u64 - 1))
    );
    assert!(!surface.history.contains_key(&(MOUSE_ROW_CACHE_CAP as u64)));
}

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
