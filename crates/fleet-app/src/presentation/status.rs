use fleet_core::{
    github::{InspectionPrState, PrState},
    sessions::{AgentActivity, SessionState},
};
use fleet_ui_kit::{Icon, PrBadgeState, StatusKind};

pub fn session_glyph(
    session: SessionState,
    slept: bool,
    agent_activity: AgentActivity,
) -> StatusKind {
    match agent_activity {
        AgentActivity::Working => return StatusKind::AgentWorking,
        AgentActivity::Idle => return StatusKind::AgentFinished,
        AgentActivity::Unknown => {}
    }
    match session {
        SessionState::Attached => StatusKind::Attached,
        SessionState::Detached if slept => StatusKind::Sleeping,
        SessionState::Detached => StatusKind::DetachedAwake,
        SessionState::Unknown => StatusKind::Unknown,
        SessionState::None => StatusKind::NoSession,
    }
}

pub fn row_glyph(
    session: SessionState,
    slept: bool,
    agent_activity: AgentActivity,
    degraded: bool,
    unreachable: bool,
    job: bool,
) -> StatusKind {
    if unreachable {
        return StatusKind::HostUnreachable;
    }
    if degraded {
        return StatusKind::Degraded;
    }
    if job {
        return StatusKind::JobRunning;
    }
    session_glyph(session, slept, agent_activity)
}

pub fn inspection_badge(state: InspectionPrState) -> Option<PrBadgeState> {
    match state {
        InspectionPrState::Open => Some(PrBadgeState::Review),
        InspectionPrState::Merged => Some(PrBadgeState::Merged),
        InspectionPrState::Closed => None,
    }
}
pub fn pr_badge_state(state: PrState) -> PrBadgeState {
    match state {
        PrState::Draft => PrBadgeState::Draft,
        PrState::CiFail => PrBadgeState::CiFail,
        PrState::Changes => PrBadgeState::Changes,
        PrState::CiPending => PrBadgeState::CiPending,
        PrState::Approved => PrBadgeState::Approved,
        PrState::Review => PrBadgeState::Review,
    }
}

/// The two existing keep-alive vocabularies intentionally differ during migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAliveStyle {
    Worktree,
    Terminal,
}

pub fn keep_alive_icon(label: &str, style: KeepAliveStyle) -> Icon {
    match style {
        KeepAliveStyle::Terminal => {
            if label.starts_with(':') {
                return Icon::Server;
            }
            match label.to_ascii_lowercase().as_str() {
                "claude" | "codex" | "aider" => Icon::Bot,
                "opencode" => Icon::Sparkles,
                "server" | "serve" | "dev" => Icon::Server,
                "nvim" | "vim" | "hx" | "helix" => Icon::FilePen,
                _ => Icon::Zap,
            }
        }
        KeepAliveStyle::Worktree => {
            let label = label.to_lowercase();
            if label.starts_with(':') || label.contains("port") {
                Icon::Server
            } else if label.contains("claude")
                || label.contains("opencode")
                || label.contains("agent")
            {
                Icon::Bot
            } else {
                Icon::Zap
            }
        }
    }
}

/// Borrows the explicit name or nonempty OSC title, using the existing tab precedence.
pub fn terminal_label(terminal: &fleet_core::sessions::Terminal, renamed: bool) -> &str {
    if renamed {
        return &terminal.name;
    }
    terminal
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(&terminal.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_labels_keep_explicit_names_and_ignore_empty_titles() {
        use fleet_core::{
            ids::TerminalId,
            sessions::{Terminal, TerminalKind, TerminalStatus},
        };
        let mut terminal = Terminal {
            id: TerminalId(1),
            name: "shell".into(),
            command: String::new(),
            cwd: String::new(),
            shell_pid: None,
            foreground_command: None,
            status: TerminalStatus::Running,
            title: Some("  vim README  ".into()),
            keep_alive: vec!["nvim".into()],
            has_unseen_output: true,
            agent_attention: None,
            kind: TerminalKind::Pty,
        };
        assert_eq!(terminal_label(&terminal, false), "vim README");
        assert_eq!(terminal_label(&terminal, true), "shell");
        terminal.title = Some("   ".into());
        assert_eq!(terminal_label(&terminal, false), "shell");
    }

    #[test]
    fn mapping_keeps_surface_policy_explicit() {
        assert_eq!(
            keep_alive_icon("opencode", KeepAliveStyle::Worktree),
            Icon::Bot
        );
        assert_eq!(
            keep_alive_icon("opencode", KeepAliveStyle::Terminal),
            Icon::Sparkles
        );
        assert_eq!(
            keep_alive_icon("nvim", KeepAliveStyle::Terminal),
            Icon::FilePen
        );
        assert_eq!(keep_alive_icon("nvim", KeepAliveStyle::Worktree), Icon::Zap);
        assert_eq!(
            row_glyph(
                SessionState::Attached,
                false,
                AgentActivity::Working,
                true,
                true,
                true
            ),
            StatusKind::HostUnreachable
        );
        assert_eq!(
            row_glyph(
                SessionState::Attached,
                false,
                AgentActivity::Working,
                true,
                false,
                true
            ),
            StatusKind::Degraded
        );
        assert_eq!(
            row_glyph(
                SessionState::Attached,
                false,
                AgentActivity::Working,
                false,
                false,
                true
            ),
            StatusKind::JobRunning
        );
        assert_eq!(
            session_glyph(SessionState::Detached, true, AgentActivity::Working),
            StatusKind::AgentWorking
        );
        assert_eq!(inspection_badge(InspectionPrState::Closed), None);
    }
}
