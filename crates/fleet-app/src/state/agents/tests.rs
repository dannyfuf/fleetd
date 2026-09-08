use super::*;

use fleet_core::{
    agents::{AgentKind, GateId, GateKind, SessionState as AgentSessionState, ToolKind, TurnState},
    sessions::SessionKind,
};

use crate::state::test_support::{session_with, snapshot};

fn worktree(slug: &str) -> WorktreeId {
    format!("buk/payroll#{slug}")
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn summary(slug: &str, attention: Attention, last_seq: u64) -> AgentThreadSummary {
    let mut projection = ThreadProjection::new(ThreadId::new(), worktree(slug), AgentKind::Claude);
    projection.last_seq = Seq(last_seq);
    // The two seen-defined attentions are re-derived per client from these cursors (§3.3), so a
    // hand-made summary has to carry the sequence the daemon would have stamped for them.
    match attention {
        Attention::NeedsYou(AttentionKind::Finished) => {
            projection.last_completed_seq = Some(Seq(last_seq));
        }
        Attention::Unread => projection.last_nonterminal_seq = Some(Seq(last_seq)),
        _ => {}
    }
    let mut summary = projection.summary(Seq::default());
    summary.attention = attention;
    summary
}

fn populated(threads: Vec<AgentThreadSummary>) -> Snapshot {
    let mut snapshot = snapshot();
    // A populated context keeps the app off the first-run card, whose keys shadow every screen.
    snapshot.contexts.push(fleet_core::model::Context {
        id: "buk".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "buk".to_owned(),
        owners: vec!["buk".to_owned()],
        created_at: "2026-09-04T09:00:00Z".to_owned(),
    });
    let mut session = session_with("buk/payroll/feat", &[1]);
    session.kind = SessionKind::Worktree(worktree("feat"));
    snapshot.sessions.push(session);
    snapshot.agent_threads = threads;
    snapshot
}

fn state_with(threads: Vec<AgentThreadSummary>) -> AppState {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let snapshot = populated(threads);
    state.seed_agent_activity(&snapshot, now);
    state.apply_snapshot(snapshot, now);
    state
}

#[test]
fn the_context_bar_counts_every_thread_in_the_snapshot() {
    let state = state_with(vec![
        summary("a", Attention::NeedsYou(AttentionKind::Permission), 3),
        summary("b", Attention::NeedsYou(AttentionKind::Finished), 3),
        summary("c", Attention::Working, 3),
        summary("d", Attention::Failed, 3),
        summary("e", Attention::Unread, 3),
        summary("f", Attention::Idle, 3),
    ]);
    assert_eq!(
        state.agents.counts(),
        AgentCounts {
            needs_you: 2,
            working: 1,
            failed: 1,
        },
        "§3.3: unread and idle are counted by neither chip"
    );
    assert!(state.agents.counts().any());
    assert!(!AgentCounts::default().any());
}

#[test]
fn viewing_a_thread_clears_finished_and_unread_before_the_daemon_echoes() {
    let finished = summary("a", Attention::NeedsYou(AttentionKind::Finished), 7);
    let blocked = summary("b", Attention::NeedsYou(AttentionKind::Permission), 7);
    let mut state = state_with(vec![finished.clone(), blocked.clone()]);

    state.agents.mark_seen(finished.thread, Seq(7));
    state.agents.mark_seen(blocked.thread, Seq(7));
    assert_eq!(state.agents.attention(finished.thread), Attention::Idle);
    assert_eq!(
        state.agents.attention(blocked.thread),
        Attention::NeedsYou(AttentionKind::Permission),
        "an open gate survives being looked at; only seen-defined attentions clear"
    );
    assert_eq!(
        state.agents.counts(),
        AgentCounts {
            needs_you: 1,
            ..AgentCounts::default()
        }
    );
}

#[test]
fn an_already_blocked_thread_does_not_notify_on_the_first_snapshot() {
    let blocked = summary("a", Attention::NeedsYou(AttentionKind::Permission), 2);
    let mut state = state_with(vec![blocked.clone()]);
    assert!(
        state.toasts.is_empty(),
        "a thread blocked before this window connected is not news"
    );

    // The same thread moving to a different signal is an edge, and notifies once.
    let mut failed = blocked.clone();
    failed.attention = Attention::Failed;
    state.apply_agent_summary(failed.clone(), Instant::now());
    assert_eq!(state.toasts.len(), 1);
    state.apply_agent_summary(failed, Instant::now());
    assert_eq!(state.toasts.len(), 1, "a steady state notifies only once");
}

#[test]
fn a_worktrees_tabs_are_its_own_threads_in_snapshot_order() {
    let first = summary("feat", Attention::Idle, 1);
    let second = summary("feat", Attention::Idle, 1);
    let elsewhere = summary("other", Attention::Idle, 1);
    let state = state_with(vec![first.clone(), elsewhere, second.clone()]);
    assert_eq!(
        state
            .agents
            .of_worktree(&worktree("feat"))
            .iter()
            .map(|summary| summary.thread)
            .collect::<Vec<_>>(),
        vec![first.thread, second.thread]
    );
}

#[test]
fn an_open_gate_routes_the_keyboard_to_its_decision_context() {
    let thread = summary("feat", Attention::NeedsYou(AttentionKind::Permission), 1);
    let mut state = state_with(vec![thread.clone()]);
    state.screen = crate::state::Screen::Workspace {
        session: "buk/payroll/feat"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.agents.activate(worktree("feat"), thread.thread);
    assert_eq!(state.active_agent_thread(), Some(thread.thread));
    // With no projection installed the tab is still an agent tab, and idle owns the keys.
    assert_eq!(
        state.agent_context_chain(),
        Some(vec!["Agent", "AgentIdle"])
    );
    assert_eq!(state.context_chain(), vec!["Agent", "AgentIdle"]);
    assert_eq!(state.mode(), crate::state::Mode::Agent);
    assert_eq!(state.mode().word(), fleet_ui_kit::Mode::Agent);

    let mut projection = ThreadProjection::new(thread.thread, worktree("feat"), AgentKind::Claude);
    projection.session = AgentSessionState::Running;
    projection.turn = TurnState::Running(fleet_core::agents::TurnId::new());
    state.agents.install_snapshot(projection.clone(), &[]);
    assert_eq!(
        state.agent_context_chain(),
        Some(vec!["Agent", "AgentWorking"])
    );

    projection.gates.push(fleet_core::agents::OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "run a command".to_owned(),
            payload: "ls".to_owned(),
            rationale: None,
            options: Vec::new(),
        },
        opened_seq: Seq(1),
        blocked_since: None,
    });
    state.agents.install_snapshot(projection, &[]);
    assert_eq!(
        state.agent_context_chain(),
        Some(vec!["Agent", "AgentDecision", "AgentPermission"]),
        "§9: a card owns the bare keys while it is open"
    );

    // …but not while the payload is being corrected: `y`, `a`, `n` and `e` are letters then.
    state.agents.set_composing(thread.thread, true);
    assert_eq!(
        state.agent_context_chain(),
        Some(vec!["Agent", "AgentWorking"]),
        "the composer keeps its own keys while a payload or a note is being typed"
    );
    state.agents.set_composing(thread.thread, false);
    assert_eq!(
        state.agent_context_chain(),
        Some(vec!["Agent", "AgentDecision", "AgentPermission"])
    );
}

#[test]
fn a_snapshot_forgets_threads_the_daemon_no_longer_lists() {
    let thread = summary("feat", Attention::Idle, 1);
    let mut state = state_with(vec![thread.clone()]);
    state.agents.activate(worktree("feat"), thread.thread);
    state.agents.mark_seen(thread.thread, Seq(1));

    state.apply_snapshot(populated(Vec::new()), Instant::now());

    assert!(state.agents.summaries().is_empty());
    assert!(state.agents.active(&worktree("feat")).is_none());
    assert_eq!(state.agents.seen(thread.thread), Seq::default());
    assert_eq!(state.agents.counts(), AgentCounts::default());
}

#[test]
fn a_daemon_restart_re_reports_every_cursor_this_window_already_read() {
    let read = summary("a", Attention::NeedsYou(AttentionKind::Finished), 7);
    let fresh = summary("b", Attention::NeedsYou(AttentionKind::Finished), 7);
    let mut state = state_with(vec![read.clone(), fresh.clone()]);
    state.agents.mark_seen(read.thread, Seq(7));

    // §3.3: `last_seen_seq` lives in the daemon's runtime, so a restart brings back every
    // already-read tab as amber. The window re-reports what it read — for every thread, not
    // just the visible tab.
    let stale = state.agents.stale_seen();
    assert_eq!(stale, vec![(read.thread, Seq(7))]);

    for (thread, seq) in stale {
        state.agents.mark_reported(thread, seq);
    }
    assert!(
        state.agents.stale_seen().is_empty(),
        "one restart costs one request per thread"
    );
}

/// UX-11: the status bar mirrors the keys that fire, and a plan gate on a running turn reads
/// `NeedsYou(Plan)` while `Agent > AgentWorking` still owns them.
#[test]
fn the_status_bar_key_set_follows_the_context_not_the_badge() {
    use crate::screens::agent_thread::presentation::key_hint_set;

    let thread = summary("feat", Attention::NeedsYou(AttentionKind::Plan), 1);
    let mut state = state_with(vec![thread.clone()]);
    state.screen = crate::state::Screen::Workspace {
        session: "buk/payroll/feat"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.agents.activate(worktree("feat"), thread.thread);

    let mut projection = ThreadProjection::new(thread.thread, worktree("feat"), AgentKind::Claude);
    projection.session = AgentSessionState::Running;
    projection.turn = TurnState::Running(fleet_core::agents::TurnId::new());
    projection.gates.push(fleet_core::agents::OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Plan {
            markdown: "# plan".to_owned(),
            steps: vec!["read the reducer".to_owned()],
        },
        opened_seq: Seq(1),
        blocked_since: None,
    });
    state.agents.install_snapshot(projection, &[]);

    assert_eq!(
        state.agents.attention(thread.thread),
        Attention::NeedsYou(AttentionKind::Plan)
    );
    assert!(
        state.agents.is_working(thread.thread),
        "a running turn is `Working` whatever the tab badge says"
    );
    // While the plan note is being typed the card stands its keys down and the composer's set
    // is shown — which must be the working one, since that is the context that is live.
    state.agents.set_composing(thread.thread, true);
    assert_eq!(
        state.agent_context_chain(),
        Some(vec!["Agent", "AgentWorking"])
    );
    assert_eq!(
        key_hint_set(state.agents.is_working(thread.thread)).first(),
        Some(&("esc", "stop")),
        "the bar advertised idle commands nothing in `AgentWorking` is bound to"
    );
}
