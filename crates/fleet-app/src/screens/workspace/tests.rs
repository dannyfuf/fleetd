use super::lifecycle::invalidate_history_epoch;
use crate::{
    state::MirrorGrid,
    terminal::{AbsoluteCellPoint, AbsoluteCellSelection, cached_grid_row, grid_size},
};
use gpui::{Bounds, Keystroke};
use std::collections::BTreeMap;

use fleet_core::ids::JobId;
use fleet_proto::job::JobKind;
use gpui::Modifiers as GpuiModifiers;

use super::*;

fn keystroke(key: &str, key_char: Option<&str>, mods: GpuiModifiers) -> Keystroke {
    Keystroke {
        modifiers: mods,
        key: key.to_owned(),
        key_char: key_char.map(str::to_owned),
    }
}

fn job(target: &str, status: JobStatus) -> JobRecord {
    JobRecord {
        id: JobId::try_from("job-1").unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::Prune,
        target: target.to_owned(),
        title: "prune".to_owned(),
        status,
        progress: None,
        log_path: "/tmp/job.log".to_owned(),
        started_at: "2026-09-04T00:00:00Z".to_owned(),
        finished_at: None,
        cancellable: false,
        retryable: false,
    }
}

fn encoded(key: &str, key_char: Option<&str>, mods: GpuiModifiers) -> KeyEvent {
    key_event(&keystroke(key, key_char, mods), false)
        .unwrap_or_else(|| panic!("`{key}` must encode"))
}

#[test]
fn watch_split_scales_and_the_terminal_uses_its_reduced_measured_area() {
    assert_eq!(watch_width(800.0), 360.0);
    assert_eq!(watch_width(1200.0), 480.0);
    assert_eq!(watch_width(2000.0), 640.0);
    let cell = gpui::size(px(10.0), px(20.0));
    let mut local = Local {
        area: Bounds::new(
            gpui::point(px(0.0), px(0.0)),
            gpui::size(px(1200.0), px(600.0)),
        ),
        ..Local::default()
    };
    let full = local.size_for(TerminalId(1), cell);
    local.area.size.width -= px(watch_width(1200.0) + 1.0);
    let split = local.size_for(TerminalId(1), cell);
    assert!(split.0 < full.0);
    assert_eq!(split.1, full.1);
    assert_eq!(
        split,
        grid_size(
            local.area.size,
            cell,
            fleet_ui_kit::theme::Spacing::default().sm
        )
    );
}

#[test]
fn a_plain_character_carries_its_composed_text() {
    let event = encoded("a", Some("a"), GpuiModifiers::default());
    assert_eq!(event.key, Key::Char('a'));
    assert_eq!(event.text.as_deref(), Some("a"));
    assert_eq!(event.mods, Modifiers::empty());
    assert_eq!(event.action, KeyAction::Press);
}

#[test]
fn the_semantic_key_stays_unshifted_while_the_text_is_shifted() {
    let mods = GpuiModifiers {
        shift: true,
        ..GpuiModifiers::default()
    };
    let event = encoded("a", Some("A"), mods);
    assert_eq!(event.key, Key::Char('a'));
    assert_eq!(event.text.as_deref(), Some("A"));
    assert!(event.mods.contains(Modifiers::SHIFT));
}

#[test]
fn control_keys_carry_no_text() {
    let mods = GpuiModifiers {
        control: true,
        ..GpuiModifiers::default()
    };
    let event = encoded("c", None, mods);
    assert_eq!(event.key, Key::Char('c'));
    assert_eq!(event.text, None);
    assert!(event.mods.contains(Modifiers::CTRL));
}

#[test]
fn named_keys_never_send_text() {
    for (name, expected) in [
        ("enter", Key::Enter),
        ("escape", Key::Escape),
        ("backspace", Key::Backspace),
        ("tab", Key::Tab),
        ("up", Key::Up),
        ("pagedown", Key::PageDown),
        ("f5", Key::F5),
    ] {
        let event = encoded(name, Some("\r"), GpuiModifiers::default());
        assert_eq!(event.key, expected, "{name}");
        assert_eq!(event.text, None, "{name}");
    }
}

#[test]
fn nvim_mode_keys_survive_the_app_translation() {
    let plain = GpuiModifiers::default();
    assert_eq!(encoded("escape", None, plain).key, Key::Escape);
    assert_eq!(encoded("i", Some("i"), plain).text.as_deref(), Some("i"));
    assert_eq!(encoded("v", Some("v"), plain).text.as_deref(), Some("v"));
    assert_eq!(encoded("up", None, plain).key, Key::Up);
    assert_eq!(encoded("down", None, plain).key, Key::Down);

    let shift = GpuiModifiers {
        shift: true,
        ..GpuiModifiers::default()
    };
    let colon = encoded(";", Some(":"), shift);
    assert_eq!(colon.key, Key::Char(';'));
    assert_eq!(colon.text.as_deref(), Some(":"));
    let shift_up = encoded("up", None, shift);
    assert_eq!(shift_up.key, Key::Up);
    assert!(shift_up.mods.contains(Modifiers::SHIFT));

    let control = GpuiModifiers {
        control: true,
        ..GpuiModifiers::default()
    };
    let ctrl_bracket = encoded("[", None, control);
    assert_eq!(ctrl_bracket.key, Key::Char('['));
    assert_eq!(ctrl_bracket.text, None);
    assert!(ctrl_bracket.mods.contains(Modifiers::CTRL));
    for key in ["c", "w"] {
        let event = encoded(key, None, control);
        assert_eq!(event.key, Key::Char(key.chars().next().unwrap_or_default()));
        assert_eq!(event.text, None);
        assert!(event.mods.contains(Modifiers::CTRL));
    }

    let alt = GpuiModifiers {
        alt: true,
        ..GpuiModifiers::default()
    };
    let alt_x = encoded("x", Some("x"), alt);
    assert_eq!(alt_x.key, Key::Char('x'));
    assert_eq!(alt_x.text.as_deref(), Some("x"));
    assert!(alt_x.mods.contains(Modifiers::ALT));
}

#[test]
fn space_is_a_character_not_a_named_key() {
    assert_eq!(
        encoded("space", Some(" "), GpuiModifiers::default()).key,
        Key::Char(' ')
    );
}

#[test]
fn a_held_key_is_a_repeat() {
    let event = key_event(&keystroke("a", Some("a"), GpuiModifiers::default()), true)
        .unwrap_or_else(|| panic!("printable keys must encode"));
    assert_eq!(event.action, KeyAction::Repeat);
}

#[test]
fn unmodelled_keys_are_dropped_rather_than_typed() {
    for name in ["f13", "back", "", "shift"] {
        assert!(
            key_event(&keystroke(name, None, GpuiModifiers::default()), false).is_none(),
            "`{name}` must not reach the pty"
        );
    }
}

#[test]
fn every_modifier_survives_the_translation() {
    let mods = GpuiModifiers {
        control: true,
        alt: true,
        shift: true,
        platform: true,
        function: true,
    };
    let event = encoded("a", None, mods);
    assert!(event.mods.contains(Modifiers::CTRL));
    assert!(event.mods.contains(Modifiers::ALT));
    assert!(event.mods.contains(Modifiers::SHIFT));
    assert!(event.mods.contains(Modifiers::SUPER));
}

#[test]
fn only_this_sessions_jobs_reach_the_header_chip() {
    let jobs = vec![
        job("acme/api#feature", JobStatus::Running),
        job(
            "acme/api#feature",
            JobStatus::Failed {
                error: "boom".to_owned(),
            },
        ),
        job("acme/api#other", JobStatus::Running),
        job("acme/api#feature", JobStatus::Succeeded),
    ];
    let targets = vec!["acme/api#feature".to_owned()];
    assert_eq!(job_counts(&jobs, &targets), (1, 1));
    assert_eq!(job_counts(&jobs, &[]), (0, 0));
}

#[test]
fn a_queued_job_already_counts_as_running() {
    let jobs = vec![job("t", JobStatus::Queued)];
    assert_eq!(job_counts(&jobs, &["t".to_owned()]), (1, 0));
}

#[test]
fn the_status_glyph_matches_the_hub_row() {
    assert_eq!(
        status_kind(SessionState::Attached, false, AgentActivity::Unknown, false,),
        StatusKind::Attached
    );
    assert_eq!(
        status_kind(SessionState::Detached, true, AgentActivity::Unknown, false,),
        StatusKind::Sleeping
    );
    assert_eq!(
        status_kind(SessionState::Detached, false, AgentActivity::Unknown, false,),
        StatusKind::DetachedAwake
    );
    assert_eq!(
        status_kind(SessionState::Unknown, false, AgentActivity::Unknown, false,),
        StatusKind::Unknown
    );
    assert_eq!(
        status_kind(SessionState::None, false, AgentActivity::Unknown, false,),
        StatusKind::NoSession
    );
    // A failed post-create hook outranks every session state (§2.5).
    assert_eq!(
        status_kind(SessionState::Attached, false, AgentActivity::Working, true,),
        StatusKind::Degraded
    );
    assert_eq!(
        status_kind(SessionState::Detached, true, AgentActivity::Idle, false,),
        StatusKind::AgentFinished
    );
}

#[test]
fn the_pr_badge_follows_the_strict_priority_order() {
    assert_eq!(
        badge_state(true, PrChecks::Fail, PrReviewDecision::Approved),
        PrBadgeState::Draft
    );
    assert_eq!(
        badge_state(false, PrChecks::Fail, PrReviewDecision::Approved),
        PrBadgeState::CiFail
    );
    assert_eq!(
        badge_state(false, PrChecks::Pass, PrReviewDecision::ChangesRequested),
        PrBadgeState::Changes
    );
    assert_eq!(
        badge_state(false, PrChecks::Pending, PrReviewDecision::None),
        PrBadgeState::CiPending
    );
    assert_eq!(
        badge_state(false, PrChecks::Pass, PrReviewDecision::Approved),
        PrBadgeState::Approved
    );
    assert_eq!(
        badge_state(false, PrChecks::None, PrReviewDecision::ReviewRequired),
        PrBadgeState::Review
    );
}

#[test]
fn the_scroll_caret_stops_at_both_edges() {
    // The caret is an absolute scrollback line: a viewport of 3 rows starting at line 900
    // confines it to 900..=902, and the edges are where `j` / `k` start scrolling instead.
    // `track_selection` clamps the caret into the viewport every frame, so a key only ever
    // sees one that is already in range; `move_caret_within` normalizes as a safety net.
    let local = Rc::new(RefCell::new(Local {
        caret: 900,
        ..Local::default()
    }));
    assert!(local.borrow_mut().move_caret_within(1, 900, 3));
    assert_eq!(local.borrow().caret, 901);
    assert!(local.borrow_mut().move_caret_within(5, 900, 3));
    assert_eq!(local.borrow().caret, 902);
    assert!(!local.borrow_mut().move_caret_within(1, 900, 3));
    assert!(local.borrow_mut().move_caret_within(-9, 900, 3));
    assert_eq!(local.borrow().caret, 900);
    assert!(!local.borrow_mut().move_caret_within(-1, 900, 3));
}

#[test]
fn the_caret_cannot_move_in_an_empty_grid() {
    let local = Rc::new(RefCell::new(Local::default()));
    assert!(!local.borrow_mut().move_caret_within(1, 0, 0));
}

#[test]
fn an_unmeasured_area_falls_back_to_a_conventional_grid() {
    let local = Local::default();
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(local.size_for(TerminalId(1), cell), FALLBACK_GRID);
}

#[test]
fn a_measured_area_decides_the_grid() {
    let local = Local {
        area: Bounds::new(
            gpui::point(px(0.0), px(0.0)),
            gpui::size(px(216.0), px(416.0)),
        ),
        ..Local::default()
    };
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(local.size_for(TerminalId(1), cell), (20, 20));
}

#[test]
fn a_remembered_size_survives_an_unmeasured_frame() {
    let mut local = Local::default();
    local.sizes.insert(TerminalId(7), (100, 30));
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(local.size_for(TerminalId(7), cell), (100, 30));
}

#[test]
fn pre_frame_keys_and_pastes_flush_in_input_order() {
    let key = |character| {
        PendingInput::Key(KeyEvent {
            key: Key::Char(character),
            mods: Modifiers::empty(),
            text: Some(character.to_string()),
            action: KeyAction::Press,
        })
    };
    let mut pending = vec![key('a'), PendingInput::Paste("middle".to_owned()), key('b')];

    let requests = drain_pending_requests(&mut pending, TerminalId(9));

    assert!(pending.is_empty());
    assert!(matches!(
        requests.as_slice(),
        [
            RequestBody::TerminalKey {
                terminal: TerminalId(9),
                key: KeyEvent { key: Key::Char('a'), .. },
            },
            RequestBody::PasteTerminal {
                terminal: TerminalId(9),
                text,
            },
            RequestBody::TerminalKey {
                terminal: TerminalId(9),
                key: KeyEvent { key: Key::Char('b'), .. },
            },
        ] if text == "middle"
    ));
}

#[test]
fn mouse_release_only_consumes_an_active_terminal_drag() {
    let mut local = Local::default();
    // The capture-phase mouse-up-out handler must let watch tab/close clicks bubble.
    assert_eq!(end_mouse_drag(&mut local), None);
    let point = AbsoluteCellPoint::new(3, 1);
    for selected in [false, true] {
        local.mouse_selection = Some(MouseSelection {
            anchor: point,
            head: point,
            initial: AbsoluteCellSelection::new(point, point),
            initiating: point,
            granularity: SelectionGranularity::Cell,
            history_epoch: 7,
            cols: 80,
            alt_screen: false,
            dragging: true,
            selected,
        });
        assert_eq!(end_mouse_drag(&mut local), Some(selected));
        assert_eq!(local.mouse_selection.is_some(), selected);
        // Even a retained selection no longer owns subsequent releases on sibling controls.
        assert_eq!(end_mouse_drag(&mut local), None);
    }

    let point = AbsoluteCellPoint::new(3, 1);
    local.mouse_selection = Some(MouseSelection {
        anchor: point,
        head: point,
        initial: AbsoluteCellSelection::new(point, point),
        initiating: point,
        granularity: SelectionGranularity::Cell,
        history_epoch: 0,
        cols: 80,
        alt_screen: false,
        dragging: true,
        selected: true,
    });
    assert!(local.cancel_drag());
    assert!(local.mouse_selection.is_none());
    assert!(!local.cancel_drag());
}

#[test]
fn history_epoch_change_clears_selections_and_the_row_cache() {
    let point = AbsoluteCellPoint::new(3, 1);
    let mut local = Local {
        anchor: Some(3),
        anchor_history_epoch: Some(7),
        mouse_selection: Some(MouseSelection {
            anchor: point,
            head: point,
            initial: AbsoluteCellSelection::new(point, point),
            initiating: point,
            granularity: SelectionGranularity::Cell,
            history_epoch: 7,
            cols: 80,
            alt_screen: false,
            dragging: false,
            selected: true,
        }),
        ..Local::default()
    };
    local.row_caches.insert(
        TerminalId(1),
        TerminalRowCache {
            cols: 80,
            alt_screen: false,
            history_epoch: 7,
            last_seq: 4,
            last_viewport_base: 2,
            rows: BTreeMap::new(),
        },
    );

    assert!(invalidate_history_epoch(&mut local, Some(8)));
    assert!(local.mouse_selection.is_none());
    assert!(local.anchor.is_none());
    assert!(local.row_caches.is_empty());
}

#[test]
fn unchanged_history_epoch_keeps_selections_and_the_row_cache() {
    let point = AbsoluteCellPoint::new(3, 1);
    let mut local = Local {
        mouse_selection: Some(MouseSelection {
            anchor: point,
            head: point,
            initial: AbsoluteCellSelection::new(point, point),
            initiating: point,
            granularity: SelectionGranularity::Cell,
            history_epoch: 7,
            cols: 80,
            alt_screen: false,
            dragging: false,
            selected: true,
        }),
        ..Local::default()
    };
    local.row_caches.insert(
        TerminalId(1),
        TerminalRowCache {
            cols: 80,
            alt_screen: false,
            history_epoch: 7,
            last_seq: 4,
            last_viewport_base: 2,
            rows: BTreeMap::new(),
        },
    );

    assert!(!invalidate_history_epoch(&mut local, Some(7)));
    assert!(local.mouse_selection.is_some());
    assert_eq!(local.row_caches.len(), 1);
}

#[test]
fn row_cache_is_untouched_without_a_selection_and_retains_only_selected_rows() {
    let mut grid = MirrorGrid::new(4, 4);
    grid.seq = 3;
    let retained =
        cached_grid_row(&grid, 0).unwrap_or_else(|| panic!("an initialized mirror row must exist"));
    let mut cache = TerminalRowCache {
        cols: 4,
        alt_screen: false,
        history_epoch: 0,
        last_seq: 1,
        last_viewport_base: 99,
        rows: BTreeMap::from([(99, retained)]),
    };

    cache_selected_grid_rows(&mut cache, &grid, 100, None);
    assert_eq!(cache.rows.keys().copied().collect::<Vec<_>>(), vec![99]);
    assert_eq!((cache.last_seq, cache.last_viewport_base), (1, 99));

    cache_selected_grid_rows(&mut cache, &grid, 100, Some((102, 101)));
    assert_eq!(
        cache.rows.keys().copied().collect::<Vec<_>>(),
        vec![101, 102]
    );
    assert_eq!((cache.last_seq, cache.last_viewport_base), (3, 100));
}
