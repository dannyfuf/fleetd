use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use fleet_core::{
    model::Worktree,
    sessions::{SessionKind, SessionState, WorktreeStatus, WorktreeWindowStatus},
};
use fleet_proto::terminal::{CellAttrs, Color, RowUpdate};

use super::*;

pub(super) fn frame(seq: u64, full: bool, rows: Vec<RowUpdate>) -> FrameUpdate {
    FrameUpdate {
        terminal: TerminalId(1),
        seq,
        cols: 4,
        rows: 2,
        full,
        shift: None,
        rows_changed: rows,
        cursor: CursorState {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
        },
        viewport: ViewportInfo {
            scrollback_len: 0,
            offset: 0,
            history_epoch: 0,
        },
        modes: TerminalModes::default(),
        title: None,
    }
}

pub(super) fn snapshot() -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-04T12:00:00Z".to_owned(),
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "0.1.0".to_owned(),
            pid: 4211,
            started_at: "2026-09-04T09:00:00Z".to_owned(),
            home: "/tmp/fleet".to_owned(),
        },
    }
}

#[derive(Debug)]
struct RecordingSound(Arc<AtomicUsize>);

impl NotificationSound for RecordingSound {
    fn play(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

pub(super) fn state_with_recording_sound(now: Instant) -> (AppState, Arc<AtomicUsize>) {
    let plays = Arc::new(AtomicUsize::new(0));
    let mut state = AppState::new("/tmp/fleet", now);
    state.notification_sound = Box::new(RecordingSound(Arc::clone(&plays)));
    (state, plays)
}

pub(super) fn agent_snapshot(entries: &[(&str, &str, u64, AgentActivity)]) -> Snapshot {
    let mut snapshot = snapshot();
    for (session_id, slug, terminal_id, activity) in entries {
        let worktree_id: fleet_core::ids::WorktreeId = format!("buk/payroll#{slug}")
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let mut session = session_with(session_id, &[*terminal_id]);
        session.kind = SessionKind::Worktree(worktree_id.clone());
        snapshot.sessions.push(session);
        snapshot.worktrees.push(Worktree {
            id: worktree_id.clone(),
            repo_id: "buk/payroll"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            slug: (*slug).to_owned(),
            branch: format!("feat/{slug}"),
            base_ref: "origin/main".to_owned(),
            path: format!("/tmp/{slug}"),
            session: (*session_id).to_owned(),
            host: None,
            created_at: "2026-09-05T12:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        });
        snapshot.statuses.push(WorktreeStatus {
            worktree_id,
            session: SessionState::Detached,
            windows: vec![WorktreeWindowStatus {
                index: 0,
                name: "cc".to_owned(),
                command: "claude".to_owned(),
                keep_alive: vec!["claude".to_owned()],
                agent: Some("claude".to_owned()),
                agent_activity: *activity,
                agent_activity_changed_at: Some("2026-09-05T12:00:00Z".to_owned()),
            }],
            running: vec!["claude".to_owned()],
            agent_activity: *activity,
            agent_activity_changed_at: Some("2026-09-05T12:00:00Z".to_owned()),
        });
    }
    snapshot
}

pub(super) fn agent_event(session: &str, terminal_id: u64, activity: AgentActivity) -> Event {
    Event::AgentActivityChanged {
        session: session.parse().unwrap_or_else(|error| panic!("{error}")),
        terminal_id: TerminalId(terminal_id),
        agent: Some("claude".to_owned()),
        activity,
        changed_at: "2026-09-05T12:00:00Z".to_owned(),
    }
}

pub(super) fn row(index: u16, text: &str) -> RowUpdate {
    RowUpdate {
        index,
        cells: text
            .chars()
            .map(|character| Cell {
                text: character.to_string().into(),
                fg: Color::Default,
                bg: Color::Default,
                underline_color: None,
                attrs: CellAttrs::empty(),
                width: CellWidth::Narrow,
            })
            .collect(),
        wrapped: false,
    }
}

pub(super) fn job(id: &str, status: JobStatus, finished: Option<&str>) -> JobRecord {
    JobRecord {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        kind: fleet_proto::job::JobKind::Clone,
        target: "nixos".to_owned(),
        title: "clone nixos".to_owned(),
        status,
        progress: None,
        log_path: "/tmp/j.log".to_owned(),
        started_at: "2026-09-04T12:00:00Z".to_owned(),
        finished_at: finished.map(str::to_owned),
        cancellable: true,
        retryable: true,
    }
}

pub(super) fn session_with(id: &str, terminals: &[u64]) -> fleet_core::sessions::Session {
    use fleet_core::sessions::{SessionKind, Terminal, TerminalStatus};
    fleet_core::sessions::Session {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        kind: SessionKind::Worktree(
            "buk/payroll#feat"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        ),
        cwd: "/tmp".to_owned(),
        terminals: terminals
            .iter()
            .map(|id| Terminal {
                id: TerminalId(*id),
                name: format!("t{id}"),
                command: "clear".to_owned(),
                cwd: "/tmp".to_owned(),
                shell_pid: None,
                foreground_command: None,
                status: TerminalStatus::Running,
                title: None,
                keep_alive: Vec::new(),
                has_unseen_output: false,
                kind: fleet_core::sessions::TerminalKind::Pty,
            })
            .collect(),
        active_terminal: terminals.first().map(|id| TerminalId(*id)),
        slept_at: None,
        kept_terminals: Vec::new(),
    }
}
