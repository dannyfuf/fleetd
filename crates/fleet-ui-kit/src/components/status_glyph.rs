//! `StatusGlyph` — the §2.5 vocabulary, and the single source of truth for it.
//!
//! Every screen that renders a session, a job or a clone renders it through this component, so
//! the shape a user learns in the worktrees list is the same shape in the palette, in a confirm
//! and in the Workspace header. The rules that matter and are encoded here:
//!
//! * `None` is a **dim dot at 30 %**, never a blank cell. A blank cell means "this column does
//!   not apply to this row" (§2.5 [D-3]).
//! * `Unknown` is an **amber** `circle-help`, never the `None` rendering: absence of knowledge
//!   never renders as good news (§1.3).
//! * A host that is unreachable forces the session to `Unknown`; it never falls back to `None`.

use gpui::{App, ElementId, SharedString, Window, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    theme::ActiveTheme,
    tone::Tone,
};

/// Every state a status glyph can express.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// A session is attached. `circle-dot`, green.
    Attached,
    /// Detached and awake. `circle`, primary text color.
    DetachedAwake,
    /// Detached and slept. `moon`, secondary.
    Sleeping,
    /// No session at all. `dot` at 30 %.
    NoSession,
    /// Status could not be determined. `circle-help`, amber.
    Unknown,
    /// A recognized coding agent is actively working. `loader-circle`, amber, spinning.
    AgentWorking,
    /// A recognized coding agent is heuristically idle. `circle-check`, green.
    AgentFinished,
    /// Post-create hooks failed but the worktree exists. `triangle-alert`, amber.
    Degraded,
    /// A job is running on this row. `loader-circle`, amber, spinning.
    JobRunning,
    /// A clone is in flight. `loader-circle`, amber, spinning.
    Cloning,
    /// A clone failed. `circle-x`, red.
    CloneFailed,
    /// The host is unreachable. `cloud-off`, amber.
    HostUnreachable,
}

impl StatusKind {
    /// The Lucide glyph for this state.
    pub fn icon(self) -> Icon {
        match self {
            StatusKind::Attached => Icon::CircleDot,
            StatusKind::DetachedAwake => Icon::Circle,
            StatusKind::Sleeping => Icon::Moon,
            StatusKind::NoSession => Icon::Dot,
            StatusKind::Unknown => Icon::CircleQuestionMark,
            StatusKind::Degraded => Icon::TriangleAlert,
            StatusKind::AgentWorking | StatusKind::JobRunning | StatusKind::Cloning => {
                Icon::LoaderCircle
            }
            StatusKind::AgentFinished => Icon::CircleCheck,
            StatusKind::CloneFailed => Icon::CircleX,
            StatusKind::HostUnreachable => Icon::CloudOff,
        }
    }

    /// The tone for this state.
    pub fn tone(self) -> Tone {
        match self {
            StatusKind::Attached | StatusKind::AgentFinished => Tone::Success,
            StatusKind::DetachedAwake => Tone::Default,
            StatusKind::Sleeping => Tone::Secondary,
            StatusKind::NoSession => Tone::Muted,
            StatusKind::Unknown
            | StatusKind::AgentWorking
            | StatusKind::Degraded
            | StatusKind::JobRunning
            | StatusKind::Cloning
            | StatusKind::HostUnreachable => Tone::Warning,
            StatusKind::CloneFailed => Tone::Danger,
        }
    }

    /// The opacity multiplier. Only `NoSession` lowers it.
    pub fn opacity(self, no_session_opacity: f32) -> f32 {
        match self {
            StatusKind::NoSession => no_session_opacity,
            _ => 1.0,
        }
    }

    /// Whether the glyph spins.
    pub fn spins(self) -> bool {
        matches!(
            self,
            StatusKind::AgentWorking | StatusKind::JobRunning | StatusKind::Cloning
        )
    }

    /// The state a pane whose data is frozen forces every session glyph to (§2.6, last row of
    /// the freshness ladder): the daemon is gone, so *nothing* is known any more, and unknown
    /// is amber — never the dim dot of `NoSession`.
    pub fn frozen() -> Self {
        StatusKind::Unknown
    }

    /// The detail-panel wording for this state, without any interpolated reason.
    pub fn detail_word(self) -> &'static str {
        match self {
            StatusKind::Attached => "attached",
            StatusKind::DetachedAwake => "running, detached",
            StatusKind::Sleeping => "sleeping",
            StatusKind::NoSession => "no session",
            StatusKind::Unknown => "unknown",
            StatusKind::AgentWorking => "Agent working",
            StatusKind::AgentFinished => "Agent finished — waiting for you",
            StatusKind::Degraded => "post-create hooks failed",
            StatusKind::JobRunning => "job running",
            StatusKind::Cloning => "cloning\u{2026}",
            StatusKind::CloneFailed => "clone failed",
            StatusKind::HostUnreachable => "host unreachable",
        }
    }

    /// The detail-panel sentence, with the daemon's reason appended verbatim when there is one:
    /// `unknown — host devbox offline`, `sleeping — kept cc (claude)`.
    ///
    /// The reason is never paraphrased: swarm's warning strings are greppable diagnostics
    /// (§6.3, `FactRow`), and this is the same rule one level up.
    pub fn detail_sentence(self, reason: Option<&str>) -> SharedString {
        match reason {
            Some(reason) if !reason.is_empty() => {
                SharedString::from(format!("{} \u{2014} {reason}", self.detail_word()))
            }
            _ => SharedString::new_static(self.detail_word()),
        }
    }
}

/// One status glyph, in the fixed 2 ch leading column of every row.
#[derive(IntoElement)]
pub struct StatusGlyph {
    kind: StatusKind,
    size: IconSize,
    id: Option<ElementId>,
}

impl StatusGlyph {
    /// A glyph for a state.
    #[track_caller]
    pub fn new(kind: StatusKind) -> Self {
        Self {
            kind,
            size: IconSize::Large,
            id: Some(std::panic::Location::caller().into()),
        }
    }

    /// Set the size: 16 px in a list, 12 px in the status bar.
    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    /// Stable id, required when the state spins.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// The state this glyph draws.
    pub fn kind(&self) -> StatusKind {
        self.kind
    }
}

impl RenderOnce for StatusGlyph {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = self.kind.tone().color(theme);
        self.kind
            .icon()
            .el()
            .size(self.size)
            .color(color)
            .opacity(self.kind.opacity(theme.metrics.no_session_opacity))
            .spinning(self.kind.spins())
            .map(|glyph| match self.id {
                Some(id) => glyph.id(id),
                None => glyph,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_statuses_use_the_job_and_success_visual_languages() {
        assert_eq!(StatusKind::AgentWorking.icon(), Icon::LoaderCircle);
        assert_eq!(StatusKind::AgentWorking.tone(), Tone::Warning);
        assert!(StatusKind::AgentWorking.spins());
        assert_eq!(StatusKind::AgentWorking.detail_word(), "Agent working");

        assert_eq!(StatusKind::AgentFinished.icon(), Icon::CircleCheck);
        assert_eq!(StatusKind::AgentFinished.tone(), Tone::Success);
        assert!(!StatusKind::AgentFinished.spins());
        assert_eq!(
            StatusKind::AgentFinished.detail_word(),
            "Agent finished — waiting for you"
        );
    }
}
