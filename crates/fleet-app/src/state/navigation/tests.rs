use super::*;
use crate::state::test_support::*;

#[test]
fn watch_pane_smoke_sequence_applies_real_events_without_changing_terminal_focus() {
    use fleet_core::watches::{Watch, WatchChunk, WatchId, WatchStatus, WatchStream};
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-watch-phase2", now);
    let session = session_with("fleet/watch", &[1]);
    let mut snapshot = snapshot();
    snapshot.sessions = vec![session.clone()];
    state.apply_bridge_event(BridgeEvent::Connected(Box::new(snapshot)), now);
    state.screen = Screen::Workspace {
        session: session.id.clone(),
    };
    state.enter_prefix();
    state.cycle_watch(true, now);
    assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    assert_eq!(
        state.toasts.last().unwrap().toast.text.as_ref(),
        "no subagent watches"
    );
    state.toasts.clear();
    let mut watch = Watch {
        id: WatchId(1),
        session: session.id.clone(),
        terminal: TerminalId(1),
        label: "codex".into(),
        command: vec!["sh".into()],
        cwd: None,
        pid: Some(123),
        started_at: chrono::Utc::now().to_rfc3339(),
        status: WatchStatus::Running,
        source: fleet_core::watches::WatchSource::Cooperative,
        log_file: None,
    };
    state.apply_daemon_event(Event::WatchStarted(watch.clone()), now);
    assert!(state.watches.panes[&session.id].visible);
    assert_eq!(state.watches.panes[&session.id].selected, Some(watch.id));
    assert!(state.tick(now + Duration::from_secs(1)));
    for forward in [true, false] {
        state.watches.hide(&session.id);
        state.enter_prefix();
        state.cycle_watch(forward, now);
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
        assert!(state.watches.panes[&session.id].visible);
        assert_eq!(state.watches.panes[&session.id].selected, Some(watch.id));
        assert_eq!(
            state.active_session().unwrap().active_terminal,
            Some(TerminalId(1))
        );
        assert!(state.toasts.is_empty());
    }
    state.enter_prefix();
    assert_eq!(state.close_selected_watch(now), None);
    assert_eq!(
        state.watches.entries[&watch.id].watch.status,
        WatchStatus::Running
    );
    assert!(!state.watches.panes[&session.id].visible);
    assert_eq!(
        state.toasts.last().unwrap().toast.text.as_ref(),
        "watch still running; pane hidden"
    );
    state.toggle_watch_pane(now);
    assert!(state.watches.panes[&session.id].visible);
    for i in 0..5 {
        state.apply_daemon_event(
            Event::WatchOutput {
                watch: watch.id,
                chunks: vec![
                    WatchChunk {
                        seq: i * 2,
                        stream: WatchStream::Stdout,
                        text: format!("line {}\n", i + 1),
                    },
                    WatchChunk {
                        seq: i * 2 + 1,
                        stream: WatchStream::Stderr,
                        text: format!("warn {}\n", i + 1),
                    },
                ],
            },
            now,
        );
    }
    assert_eq!(
        state.watches.entries[&watch.id].shared_display().0.len(),
        10
    );
    watch.status = WatchStatus::Exited {
        code: Some(2),
        signal: None,
    };
    state.apply_daemon_event(
        Event::WatchExited(watch.clone()),
        now + Duration::from_secs(2),
    );
    assert_eq!(state.watches.entries[&watch.id].watch.status, watch.status);
    let duration = state.watches.entries[&watch.id].elapsed(now + Duration::from_secs(10));
    assert!(duration >= Duration::from_secs(2) && duration < Duration::from_secs(3));
    state.enter_prefix();
    state.toggle_watch_pane(now);
    assert!(!state.watches.panes[&session.id].visible);
    state.enter_prefix();
    state.toggle_watch_pane(now);
    assert!(state.watches.panes[&session.id].visible);
    state.zoomed = true;
    assert!(state.watches.panes[&session.id].visible);
    state.enter_prefix();
    assert_eq!(state.close_selected_watch(now), Some(watch.id));
    state.apply_daemon_event(Event::WatchDismissed(watch.id), now);
    assert!(!state.watches.panes[&session.id].visible);
    assert!(state.watches.entries.is_empty());
    assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    assert_eq!(
        state.active_session().unwrap().active_terminal,
        Some(TerminalId(1))
    );
}

#[test]
fn mru_moves_entries_to_the_front_and_exposes_the_alternate() {
    let mut mru = Mru::default();
    mru.touch("a");
    mru.touch("b");
    mru.touch("c");
    assert_eq!(mru.entries().first(), Some(&"c"));
    assert_eq!(mru.alternate(), Some(&"b"));
    mru.touch("a");
    assert_eq!(mru.entries(), &["a", "c", "b"]);
    mru.retain(|entry| entry != &"c");
    assert_eq!(mru.entries(), &["a", "b"]);
}

#[test]
fn cursor_movement_clamps_instead_of_wrapping() {
    assert_eq!(move_cursor(0, -1, 5), 0);
    assert_eq!(move_cursor(4, 1, 5), 4);
    assert_eq!(move_cursor(2, 2, 5), 4);
    assert_eq!(move_cursor(3, -2, 5), 1);
    assert_eq!(move_cursor(3, 1, 0), 0);
    assert_eq!(clamp_cursor(9, 3), 2);
    assert_eq!(clamp_cursor(9, 0), 0);
    assert_eq!(half_page(20), 10);
    assert_eq!(half_page(1), 1);
}

#[test]
fn context_chain_follows_the_screen_the_overlay_and_the_daemon() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    assert_eq!(state.context_chain(), vec!["Hub", "Worktrees"]);

    state.hub_pane = HubPane::Repos;
    assert_eq!(state.context_chain(), vec!["Hub", "Repos"]);

    state.hub_pane = HubPane::List;
    state.screen = Screen::Hub { tab: HubTab::Prs };
    assert_eq!(state.context_chain(), vec!["Hub", "Prs"]);

    state.screen = Screen::Workspace {
        session: "payroll/feat"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    assert_eq!(state.context_chain(), vec!["Workspace", "Terminal"]);
    state.enter_prefix();
    assert_eq!(state.context_chain(), vec!["Workspace", "Prefix"]);
    state.terminal_mode = TerminalMode::Scroll;
    assert_eq!(state.context_chain(), vec!["Workspace", "Scroll"]);

    state.open_overlay(Overlay::Jobs);
    assert_eq!(state.context_chain(), vec!["Jobs"]);
    state.open_overlay(Overlay::Dialog(Dialogs::Confirm));
    assert_eq!(state.context_chain(), vec!["Dialog", "Confirm"]);
    state.close_overlay();

    state.daemon = DaemonLink::Lost {
        attempt: 1,
        dismissed: false,
        reason: DaemonLossReason::ConnectionLost,
    };
    assert_eq!(
        state.context_chain(),
        vec!["Workspace", "Scroll", "Daemon", "Banner"],
        "an undismissed banner owns r / l / Esc"
    );
    state.daemon = DaemonLink::Lost {
        attempt: 1,
        dismissed: true,
        reason: DaemonLossReason::ConnectionLost,
    };
    assert_eq!(state.context_chain(), vec!["Workspace", "Scroll"]);

    state.daemon = DaemonLink::Failed {
        message: "no socket".to_owned(),
        log_tail: Vec::new(),
        stale_socket: true,
        protocol_mismatch: false,
    };
    assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
}

#[test]
fn agent_popup_open_switch_hide_preserves_the_underlying_focus_state() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.hub_pane = HubPane::Repos;
    let base = state.screen.clone();

    assert_eq!(
        state.toggle_agent_popup(Agent::Claude, None),
        AgentPopupTransition::Opened
    );
    assert_eq!(state.screen, base);
    assert_eq!(state.hub_pane, HubPane::Repos);
    assert_eq!(state.context_chain(), vec!["Agent", "Terminal"]);

    state.open_overlay(Overlay::Dialog(Dialogs::Help));
    assert_eq!(state.context_chain(), vec!["Dialog", "Help"]);
    assert_eq!(state.mode(), Mode::Dialog);
    assert!(state.close_overlay());
    assert_eq!(state.context_chain(), vec!["Agent", "Terminal"]);

    state.enter_agent_prefix();
    assert_eq!(state.context_chain(), vec!["Agent", "Prefix"]);
    assert!(state.leave_agent_prefix());
    assert_eq!(
        state.toggle_agent_popup(Agent::Opencode, None),
        AgentPopupTransition::Switched
    );
    assert_eq!(state.screen, base);
    assert_eq!(
        state.agent_popup.as_ref().map(|popup| popup.agent),
        Some(Agent::Opencode)
    );

    assert_eq!(
        state.toggle_agent_popup(Agent::Opencode, None),
        AgentPopupTransition::Hidden
    );
    assert!(state.agent_popup.is_none());
    assert_eq!(state.context_chain(), vec!["Hub", "Repos"]);
    assert_eq!(state.screen, base);
    assert_eq!(state.hub_pane, HubPane::Repos);
}

#[test]
fn agent_popup_does_not_change_workspace_terminal_mode() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.screen = Screen::Workspace {
        session: "owner/repo"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.terminal_mode = TerminalMode::Scroll;

    state.toggle_agent_popup(Agent::Claude, None);
    state.enter_agent_prefix();
    assert_eq!(state.terminal_mode, TerminalMode::Scroll);
    assert!(state.hide_agent_popup());
    assert_eq!(state.terminal_mode, TerminalMode::Scroll);
    assert_eq!(state.context_chain(), vec!["Workspace", "Scroll"]);
}

#[test]
fn agent_prefix_restores_scroll_instead_of_bypassing_its_cleanup() {
    let mut state = AppState::new("/tmp/fleet", Instant::now());
    state.toggle_agent_popup(Agent::Claude, None);
    state
        .agent_popup
        .as_mut()
        .unwrap_or_else(|| panic!("popup must be open"))
        .mode = AgentPopupMode::Scroll;

    state.enter_agent_prefix();
    assert_eq!(
        state.agent_popup.as_ref().map(|popup| popup.mode),
        Some(AgentPopupMode::Prefix)
    );
    assert!(state.leave_agent_prefix());
    assert_eq!(
        state.agent_popup.as_ref().map(|popup| popup.mode),
        Some(AgentPopupMode::Scroll)
    );
}

#[test]
fn missing_visible_agent_session_is_recoverable_only_while_connected() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.toggle_agent_popup(Agent::Claude, None);
    state.snapshot = Some(snapshot());

    assert_eq!(state.missing_agent_popup_session(), None);
    state.daemon = DaemonLink::Connected;
    assert_eq!(
        state.missing_agent_popup_session(),
        Some((Agent::Claude, None))
    );
}

#[test]
fn the_terminal_fallback_ensures_the_threads_own_worktree_session() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let worktree: fleet_core::ids::WorktreeId = "acme/api#feature"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let mut snapshot = snapshot();
    snapshot.worktrees = vec![fleet_core::model::Worktree {
        id: worktree.clone(),
        repo_id: "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "main".to_owned(),
        path: "/tmp/worktrees/acme/api/feature".to_owned(),
        session: "acme/api/feature".to_owned(),
        host: None,
        created_at: "2026-09-04T09:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    }];
    state.snapshot = Some(snapshot);
    state.daemon = DaemonLink::Connected;

    // `^a` from the Hub keeps the one repository-level popup, which lives in `repos_dir`.
    state.toggle_agent_popup(Agent::Claude, None);
    assert_eq!(
        state.agent_popup_session_id().map(|id| id.to_string()),
        Some("swarm-agent-claude".to_owned())
    );

    // §1/§2: `^s F` on a thread opens the fallback in *that thread's* worktree, so it ensures
    // a different session — r2-06 showed `claude` starting in `<home>/repos` instead.
    let transition = state.toggle_agent_popup(Agent::Claude, Some(worktree.clone()));
    assert_eq!(transition, AgentPopupTransition::Switched);
    assert_eq!(
        state.agent_popup_session_id().map(|id| id.to_string()),
        Some("acme/api/feature/agent-claude".to_owned())
    );
    assert_eq!(
        state.missing_agent_popup_session(),
        Some((Agent::Claude, Some(worktree.clone())))
    );

    // The same key on the same worktree still hides it.
    assert_eq!(
        state.toggle_agent_popup(Agent::Claude, Some(worktree)),
        AgentPopupTransition::Hidden
    );
}

#[test]
fn an_overlay_owns_the_keyboard_on_the_first_run_card() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_snapshot(snapshot(), now);
    assert!(state.is_first_run(), "the sample snapshot is empty");
    assert_eq!(state.context_chain(), vec!["FirstRun"]);

    // §3.13 binds `?` and `,` on the card; without this the dialog opens with the
    // `FirstRun` chain and `Esc` can never match, trapping the app.
    state.open_overlay(Overlay::Dialog(Dialogs::Help));
    assert_eq!(state.context_chain(), vec!["Dialog", "Help"]);
    state.open_overlay(Overlay::Dialog(Dialogs::Settings));
    assert_eq!(state.context_chain(), vec!["Dialog", "Settings"]);
    state.close_overlay();
    assert_eq!(state.context_chain(), vec!["FirstRun"]);
}

#[test]
fn prefix_is_one_shot() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.screen = Screen::Workspace {
        session: "payroll/feat"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.enter_prefix();
    assert_eq!(state.mode(), Mode::Prefix);
    assert!(state.leave_prefix());
    assert_eq!(state.mode(), Mode::Terminal);
    assert!(!state.leave_prefix(), "leaving twice is a no-op");
}

#[test]
fn prefix_is_unreachable_from_the_hub() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.enter_prefix();
    assert_eq!(state.mode(), Mode::Normal);
}

#[test]
fn escape_clears_the_filter_in_two_stages_and_never_quits() {
    assert_eq!(filter_escape(true), FilterEscape::LeaveInput);
    assert_eq!(filter_escape(false), FilterEscape::ClearFilter);

    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.open_overlay(Overlay::Filter);
    state.filter.query = "rut".to_owned();
    state.filter.editing = true;

    assert!(state.cancel());
    assert!(!state.filter.editing, "the first Esc leaves the input");
    assert_eq!(state.filter.query, "rut", "and keeps the filter");
    assert!(state.overlay.is_none());

    assert!(state.cancel());
    assert!(state.filter.query.is_empty(), "the second Esc clears it");

    assert!(!state.cancel(), "a third Esc is a no-op, never a quit");
}

#[test]
fn mode_word_follows_the_overlay_stack() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    assert_eq!(state.mode().word(), ModeWord::Normal);
    state.open_overlay(Overlay::Palette);
    assert_eq!(state.mode().word(), ModeWord::Palette);
    state.open_overlay(Overlay::Dialog(Dialogs::Quit));
    assert_eq!(state.mode().word(), ModeWord::Dialog);
}

#[test]
fn the_workspace_mode_follows_the_kind_of_the_active_tab() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let session: SessionId = "payroll/feat".parse().unwrap_or_else(|e| panic!("{e}"));
    state.screen = Screen::Workspace {
        session: session.clone(),
    };

    // Otherwise the empty sample snapshot reads as the first run and owns the chain.
    state.has_seen_non_empty_state = true;

    let mut open = snapshot();
    let mut record = session_with("payroll/feat", &[1, 2, 3]);
    record.terminals[2].kind = fleet_core::sessions::TerminalKind::Native;
    record.active_terminal = Some(TerminalId(1));
    open.sessions = vec![record.clone()];
    state.apply_snapshot(open, now);
    assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    assert_eq!(state.context_chain(), vec!["Workspace", "Terminal"]);
    assert!(!state.active_terminal_is_native());

    // `ctrl-s 3`.
    let mut on_native = snapshot();
    record.active_terminal = Some(TerminalId(3));
    on_native.sessions = vec![record.clone()];
    state.apply_snapshot(on_native, now);
    assert!(state.active_terminal_is_native());
    assert_eq!(state.terminal_mode, TerminalMode::Native);
    assert_eq!(state.context_chain(), vec!["Workspace", "Native"]);
    // §2.8 has no ninth word: the status bar still says the Workspace has the keyboard.
    assert_eq!(state.mode().word(), ModeWord::Terminal);

    // `ctrl-s` over the pane still enters the prefix, and leaving it comes back to Native.
    state.enter_prefix();
    assert_eq!(state.context_chain(), vec!["Workspace", "Prefix"]);
    assert!(state.leave_prefix());
    assert_eq!(state.terminal_mode, TerminalMode::Native);

    // A snapshot must never yank a transient Fleet mode away underneath the user.
    state.terminal_mode = TerminalMode::Scroll;
    let mut again = snapshot();
    again.sessions = vec![record.clone()];
    state.apply_snapshot(again, now);
    assert_eq!(state.terminal_mode, TerminalMode::Scroll);

    // And `ctrl-s 1` goes back to a PTY.
    state.terminal_mode = TerminalMode::Native;
    let mut back = snapshot();
    record.active_terminal = Some(TerminalId(1));
    back.sessions = vec![record];
    state.apply_snapshot(back, now);
    assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    assert_eq!(state.context_chain(), vec!["Workspace", "Terminal"]);
}
