//! The sidebar's Agents section (UX-SPEC §3.2): every native agent thread of the active context,
//! then every agent window, one click from anywhere in the Hub.
//!
//! The rows are the palette's `AGENTS` data seen from the Hub rather than from one worktree:
//! a thread's attention, its provider and its title, and a click runs what the palette's `go`
//! runs. Built in the Hub's update path and compared, never in `render`.

use fleet_core::{
    agents::{Attention, AttentionKind, ThreadId},
    config::Agent,
    ids::ContextId,
    sessions::{AgentActivity, SessionKind},
};
use fleet_ui_kit::Tone;
use gpui::SharedString;

use crate::{screens::agent_thread::presentation::title_subject, state::AppState};

/// What a click on an agent row goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTarget {
    /// A native thread: its tab, opening its worktree first when it is not on screen.
    Thread(ThreadId),
    /// The repository-level agent window `a` / `A` shows.
    Window(Agent),
}

/// One row of the Agents section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    /// What a click goes to.
    pub target: AgentTarget,
    /// `provider · title`, or `provider · worktree` before the thread has a title.
    pub label: SharedString,
    /// Amber needs you, green working, red failed, grey otherwise.
    pub dot: Tone,
    /// Whether the row carries the `needs you` chip.
    pub needs_you: bool,
}

/// The word an agent window's row reads after its provider.
const WINDOW: &str = "agent window";

/// The provider word of an agent window, or `None` for the legacy value no window is opened for.
const fn window_provider(agent: Agent) -> Option<&'static str> {
    match agent {
        Agent::Claude => Some("claude"),
        Agent::Codex => Some("codex"),
        Agent::Opencode => None,
    }
}

/// The dot and the chip a thread's attention draws (NATIVE-AGENTS §2: amber is what needs a
/// person; a parked usage window is progress, so it stays grey).
const fn attention_marks(attention: Attention) -> (Tone, bool) {
    match attention {
        Attention::NeedsYou(
            AttentionKind::Permission
            | AttentionKind::Question
            | AttentionKind::Plan
            | AttentionKind::Finished,
        ) => (Tone::Warning, true),
        Attention::Working => (Tone::Success, false),
        Attention::Failed => (Tone::Danger, false),
        Attention::Waiting | Attention::Unread | Attention::Idle => (Tone::Muted, false),
    }
}

/// Every top-level native thread whose worktree belongs to `context` (all of them without one),
/// in the daemon's order, then every agent window the daemon holds.
///
/// Delegated children stay out: their caller's transcript reaches them, and the caller's dot
/// already carries a child that needs you.
#[must_use]
pub fn agent_rows(state: &AppState, context: Option<&ContextId>) -> Vec<AgentRow> {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return Vec::new();
    };
    let in_context = |repo: &str| {
        context.is_none_or(|context| {
            snapshot
                .repos
                .iter()
                .any(|candidate| candidate.id.as_str() == repo && &candidate.context_id == context)
        })
    };
    let threads = state
        .agents
        .summaries()
        .iter()
        .filter(|summary| summary.parent.is_none() && in_context(summary.worktree.repo()))
        .map(|summary| {
            let provider = summary.provider.executable();
            let title = title_subject(summary);
            // A thread with no title of its own yet is named by its worktree, so two fresh
            // threads in two worktrees never read the same.
            let subject = if title.is_empty() || title.eq_ignore_ascii_case(provider) {
                summary.worktree.slug().to_owned()
            } else {
                title
            };
            let label = format!("{provider} \u{b7} {subject}");
            let (dot, needs_you) = attention_marks(state.agents.attention(summary.thread));
            AgentRow {
                target: AgentTarget::Thread(summary.thread),
                label: SharedString::from(label),
                dot,
                needs_you,
            }
        });
    let windows = snapshot.sessions.iter().filter_map(|session| {
        let SessionKind::Agent(agent) = session.kind else {
            return None;
        };
        let provider = window_provider(agent)?;
        let working = state.session_agent_activity(&session.id) == AgentActivity::Working;
        Some(AgentRow {
            target: AgentTarget::Window(agent),
            label: SharedString::from(format!("{provider} \u{b7} {WINDOW}")),
            dot: if working { Tone::Success } else { Tone::Muted },
            needs_you: false,
        })
    });
    threads.chain(windows).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_you_is_amber_with_a_chip_and_a_parked_thread_stays_grey() {
        assert_eq!(
            attention_marks(Attention::NeedsYou(AttentionKind::Permission)),
            (Tone::Warning, true)
        );
        assert_eq!(
            attention_marks(Attention::NeedsYou(AttentionKind::Finished)),
            (Tone::Warning, true)
        );
        assert_eq!(attention_marks(Attention::Working), (Tone::Success, false));
        assert_eq!(attention_marks(Attention::Waiting), (Tone::Muted, false));
        assert_eq!(attention_marks(Attention::Idle), (Tone::Muted, false));
    }

    #[test]
    fn the_legacy_agent_gets_no_window_row() {
        assert_eq!(window_provider(Agent::Claude), Some("claude"));
        assert_eq!(window_provider(Agent::Opencode), None);
    }
}
