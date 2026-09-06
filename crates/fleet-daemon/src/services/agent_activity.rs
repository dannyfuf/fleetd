//! Pure coding-agent activity detection from terminal input/output observations.

use std::time::{Duration, Instant};

use fleet_core::sessions::AgentActivity;
use fleet_term::TerminalActivity;

/// Output this soon after input is treated as terminal echo or prompt redraw.
pub const ECHO_WINDOW: Duration = Duration::from_millis(400);
/// A recognized agent becomes idle after this much quiet.
pub const IDLE_DEBOUNCE: Duration = Duration::from_millis(2_500);
/// The daemon samples host activity independently of the configurable status refresh.
pub const TRACK_INTERVAL: Duration = Duration::from_millis(500);

/// Stateful, clock-injected activity detector for one terminal.
#[derive(Debug, Default)]
pub struct AgentActivityTracker {
    activity: AgentActivity,
    agent: Option<String>,
    output_bytes_total: Option<u64>,
    active_reference_at: Option<Instant>,
}

impl AgentActivityTracker {
    /// Evaluates one terminal observation and returns only an activity transition.
    pub fn observe(
        &mut self,
        now: Instant,
        input: TerminalActivity,
        agent: Option<&str>,
    ) -> Option<AgentActivity> {
        let previous = self.activity;
        let agent_changed = self.agent.as_deref() != agent;
        let had_agent = self.agent.is_some();
        let new_output = self
            .output_bytes_total
            .is_some_and(|total| input.output_bytes_total > total);
        self.output_bytes_total = Some(input.output_bytes_total);

        let Some(agent) = agent else {
            self.agent = None;
            self.active_reference_at = None;
            self.activity = AgentActivity::Unknown;
            return (self.activity != previous).then_some(self.activity);
        };

        if agent_changed {
            self.agent = Some(agent.to_owned());
            self.active_reference_at = Some(now);
            if had_agent {
                self.activity = AgentActivity::Unknown;
            }
        }

        let echoed_input = input.last_output_at >= input.last_input_at
            && input.last_output_at.duration_since(input.last_input_at) <= ECHO_WINDOW;
        if new_output && !echoed_input {
            self.active_reference_at = Some(input.last_output_at);
            self.activity = AgentActivity::Working;
        } else if self
            .active_reference_at
            .is_some_and(|reference| now.saturating_duration_since(reference) >= IDLE_DEBOUNCE)
        {
            self.activity = AgentActivity::Idle;
        }

        (self.activity != previous).then_some(self.activity)
    }

    /// Applies an authoritative hook signal and resets the quiet-time reference.
    pub fn set_explicit(
        &mut self,
        activity: AgentActivity,
        now: Instant,
        output_bytes_total: Option<u64>,
    ) -> Option<AgentActivity> {
        let previous = self.activity;
        self.activity = activity;
        self.active_reference_at = Some(now);
        if let Some(output_bytes_total) = output_bytes_total {
            self.output_bytes_total = Some(output_bytes_total);
        }
        (self.activity != previous).then_some(self.activity)
    }

    /// Current activity state.
    #[must_use]
    pub const fn activity(&self) -> AgentActivity {
        self.activity
    }

    /// Current recognized agent name.
    #[must_use]
    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
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
            Some(AgentActivity::Working)
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
            Some(AgentActivity::Idle)
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
    fn explicit_finished_stays_idle_until_later_output() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.observe(base, activity(base, 0, 0, 0), Some("opencode"));
        assert_eq!(
            tracker.set_explicit(AgentActivity::Idle, base, Some(10)),
            Some(AgentActivity::Idle)
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
            Some(AgentActivity::Working)
        );
    }

    #[test]
    fn disappearing_agent_transitions_to_unknown_once() {
        let base = Instant::now();
        let mut tracker = AgentActivityTracker::default();
        tracker.observe(base, activity(base, 0, 0, 0), Some("claude"));
        tracker.set_explicit(AgentActivity::Working, base, Some(0));
        assert_eq!(
            tracker.observe(base, activity(base, 0, 0, 0), None),
            Some(AgentActivity::Unknown)
        );
        assert_eq!(tracker.observe(base, activity(base, 0, 0, 0), None), None);
    }
}
