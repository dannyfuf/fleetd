//! Pure coding-agent activity detection from terminal input/output observations.

use std::time::{Duration, Instant};

use fleet_core::{agents::AttentionKind, sessions::AgentActivity};
use fleet_term::TerminalActivity;

/// Output this soon after input is treated as terminal echo or prompt redraw.
const ECHO_WINDOW: Duration = Duration::from_millis(400);
/// A recognized agent becomes idle after this much quiet.
const IDLE_DEBOUNCE: Duration = Duration::from_millis(2_500);
/// The daemon samples host activity independently of the configurable status refresh.
pub(super) const TRACK_INTERVAL: Duration = Duration::from_millis(500);

/// Stateful, clock-injected activity detector for one terminal.
#[derive(Debug, Default)]
pub(super) struct AgentActivityTracker {
    activity: AgentActivity,
    attention: Option<AttentionKind>,
    agent: Option<String>,
    output_bytes_total: Option<u64>,
    last_input_at: Option<Instant>,
    active_reference_at: Option<Instant>,
}

/// Complete terminal-agent state returned whenever status or semantic attention changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AgentActivityState {
    pub(super) activity: AgentActivity,
    pub(super) attention: Option<AttentionKind>,
}

impl AgentActivityTracker {
    /// Evaluates one terminal observation and returns only an activity transition.
    pub(super) fn observe(
        &mut self,
        now: Instant,
        input: TerminalActivity,
        agent: Option<&str>,
    ) -> Option<AgentActivityState> {
        let previous = self.state();
        let agent_changed = self.agent.as_deref() != agent;
        let had_agent = self.agent.is_some();
        let new_output = self
            .output_bytes_total
            .is_some_and(|total| input.output_bytes_total > total);
        let new_input = self
            .last_input_at
            .is_some_and(|last_input_at| input.last_input_at > last_input_at);
        self.output_bytes_total = Some(input.output_bytes_total);
        self.last_input_at = Some(input.last_input_at);

        let Some(agent) = agent else {
            self.agent = None;
            self.active_reference_at = None;
            self.activity = AgentActivity::Unknown;
            return (self.state() != previous).then_some(self.state());
        };

        if agent_changed {
            self.agent = Some(agent.to_owned());
            self.active_reference_at = Some(now);
            if had_agent {
                self.activity = AgentActivity::Unknown;
            }
        }

        if new_input {
            self.attention = None;
        }

        let echoed_input = input.last_output_at >= input.last_input_at
            && input.last_output_at.duration_since(input.last_input_at) <= ECHO_WINDOW;
        if new_output && !echoed_input {
            // An explicit attention reason survives heuristic quiet/idle observations. Only
            // output whose byte counter advances outside the input echo window proves that the
            // agent actually resumed, so prompt echo/redraw cannot dismiss a user-visible gate.
            self.attention = None;
            self.active_reference_at = Some(input.last_output_at);
            self.activity = AgentActivity::Working;
        } else if self
            .active_reference_at
            .is_some_and(|reference| now.saturating_duration_since(reference) >= IDLE_DEBOUNCE)
        {
            self.activity = AgentActivity::Idle;
        }

        (self.state() != previous).then_some(self.state())
    }

    /// Applies an authoritative hook signal and resets the quiet-time/input references.
    pub(super) fn set_explicit(
        &mut self,
        mut activity: AgentActivity,
        attention: Option<AttentionKind>,
        now: Instant,
        terminal_activity: Option<TerminalActivity>,
    ) -> Option<AgentActivityState> {
        let previous = self.state();
        let attention = if attention.is_some() {
            activity = AgentActivity::Idle;
            attention
        } else if activity == AgentActivity::Working {
            None
        } else {
            self.attention
        };
        self.activity = activity;
        self.attention = attention;
        self.active_reference_at = Some(now);
        if let Some(terminal_activity) = terminal_activity {
            self.output_bytes_total = Some(terminal_activity.output_bytes_total);
            self.last_input_at = Some(terminal_activity.last_input_at);
        }
        (self.state() != previous).then_some(self.state())
    }

    /// Current activity state.
    #[must_use]
    pub(super) const fn activity(&self) -> AgentActivity {
        self.activity
    }

    const fn state(&self) -> AgentActivityState {
        AgentActivityState {
            activity: self.activity,
            attention: self.attention,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(base: Instant, output_ms: u64, input_ms: u64, bytes: u64) -> TerminalActivity {
        TerminalActivity {
            last_output_at: base + Duration::from_millis(output_ms),
            last_input_at: base + Duration::from_millis(input_ms),
            output_bytes_total: bytes,
        }
    }

    #[test]
    fn output_transitions_to_working_and_quiet_to_idle_without_spam() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        assert_eq!(
            tracker.observe(base, activity(base, 0, 0, 0), Some("claude")),
            None
        );
        assert_eq!(
            tracker.observe(
                base + Duration::from_secs(1),
                activity(base, 1_000, 0, 10),
                Some("claude")
            ),
            Some(AgentActivityState {
                activity: AgentActivity::Working,
                attention: None,
            })
        );
        assert_eq!(
            tracker.observe(
                base + Duration::from_millis(3_499),
                activity(base, 1_000, 0, 10),
                Some("claude")
            ),
            None
        );
        assert_eq!(
            tracker.observe(
                base + Duration::from_millis(3_500),
                activity(base, 1_000, 0, 10),
                Some("claude")
            ),
            Some(AgentActivityState {
                activity: AgentActivity::Idle,
                attention: None,
            })
        );
        assert_eq!(
            tracker.observe(
                base + Duration::from_secs(4),
                activity(base, 1_000, 0, 10),
                Some("claude")
            ),
            None
        );
    }

    #[test]
    fn output_in_echo_window_does_not_count() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.observe(base, activity(base, 0, 0, 0), Some("codex"));
        assert_eq!(
            tracker.observe(
                base + Duration::from_millis(300),
                activity(base, 300, 100, 5),
                Some("codex")
            ),
            None
        );
        assert_eq!(tracker.activity(), AgentActivity::Unknown);
    }

    #[test]
    fn output_without_agent_stays_unknown() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        assert_eq!(tracker.observe(base, activity(base, 0, 0, 50), None), None);
        assert_eq!(tracker.activity(), AgentActivity::Unknown);
    }

    #[test]
    fn explicit_attention_survives_quiet_and_clears_on_real_output() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.observe(base, activity(base, 0, 0, 0), Some("opencode"));
        assert_eq!(
            tracker.set_explicit(
                AgentActivity::Idle,
                Some(AttentionKind::Finished),
                base,
                Some(activity(base, 0, 0, 10)),
            ),
            Some(AgentActivityState {
                activity: AgentActivity::Idle,
                attention: Some(AttentionKind::Finished),
            })
        );
        assert_eq!(
            tracker.observe(
                base + Duration::from_secs(5),
                activity(base, 0, 0, 10),
                Some("opencode")
            ),
            None
        );
        assert_eq!(
            tracker.observe(
                base + Duration::from_secs(6),
                activity(base, 6_000, 0, 11),
                Some("opencode")
            ),
            Some(AgentActivityState {
                activity: AgentActivity::Working,
                attention: None,
            })
        );
    }

    #[test]
    fn input_clears_attention_but_echo_does_not_resume_working() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.observe(base, activity(base, 0, 0, 0), Some("claude"));
        tracker.set_explicit(
            AgentActivity::Idle,
            Some(AttentionKind::Question),
            base,
            Some(activity(base, 0, 0, 0)),
        );

        assert_eq!(
            tracker.observe(
                base + Duration::from_millis(200),
                activity(base, 200, 100, 5),
                Some("claude"),
            ),
            Some(AgentActivityState {
                activity: AgentActivity::Idle,
                attention: None,
            })
        );
    }

    #[test]
    fn explicit_working_clears_attention() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.set_explicit(
            AgentActivity::Idle,
            Some(AttentionKind::Permission),
            base,
            None,
        );
        assert_eq!(
            tracker.set_explicit(AgentActivity::Working, None, base, None),
            Some(AgentActivityState {
                activity: AgentActivity::Working,
                attention: None,
            })
        );
    }

    #[test]
    fn legacy_explicit_idle_does_not_clear_attention() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.set_explicit(
            AgentActivity::Idle,
            Some(AttentionKind::Finished),
            base,
            None,
        );
        assert_eq!(
            tracker.set_explicit(AgentActivity::Idle, None, base, None),
            None
        );
        assert_eq!(tracker.attention, Some(AttentionKind::Finished));
    }

    #[test]
    fn disappearing_agent_transitions_to_unknown_once() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.observe(base, activity(base, 0, 0, 0), Some("claude"));
        tracker.set_explicit(AgentActivity::Working, None, base, None);
        assert_eq!(
            tracker.observe(base, activity(base, 0, 0, 0), None),
            Some(AgentActivityState {
                activity: AgentActivity::Unknown,
                attention: None,
            })
        );
        assert_eq!(tracker.observe(base, activity(base, 0, 0, 0), None), None);
    }
}
