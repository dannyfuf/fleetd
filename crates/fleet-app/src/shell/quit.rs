//! Quit policy preserves daemon-owned work unless shutdown was explicitly requested.

use fleet_proto::job::JobRecord;

use crate::presentation::is_active;

/// What `ctrl-q` does. The daemon keeps running either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuitDecision {
    /// Quit immediately: nothing is running, or the user turned the warning off.
    QuitNow,
    /// Show §3.8.8, which enumerates what keeps running in fleetd.
    Confirm,
}

/// What `ctrl-shift-q` does. Everything listed dies, so the confirm is not opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopDecision {
    /// Stop the daemon and quit without asking: nothing is running.
    StopNow,
    /// Show §3.8.9, which enumerates what is killed and what is cancelled.
    Confirm,
}

/// The §3.8.8 decision.
#[must_use]
pub(super) const fn quit_decision(warn_before_quit: bool, running: usize) -> QuitDecision {
    if warn_before_quit && running > 0 {
        QuitDecision::Confirm
    } else {
        QuitDecision::QuitNow
    }
}

/// The §3.8.9 decision. Sessions count as running work, because stopping fleetd kills them.
#[must_use]
pub(super) const fn stop_decision(running: usize, sessions: usize) -> StopDecision {
    if running > 0 || sessions > 0 {
        StopDecision::Confirm
    } else {
        StopDecision::StopNow
    }
}

/// How many jobs the quit dialogs enumerate.
#[must_use]
pub(super) fn running_count(jobs: &[JobRecord]) -> usize {
    jobs.iter().filter(|job| is_active(&job.status)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_q_confirms_only_when_asked_to_and_something_is_running() {
        assert_eq!(quit_decision(true, 2), QuitDecision::Confirm);
        assert_eq!(quit_decision(true, 0), QuitDecision::QuitNow);
        assert_eq!(quit_decision(false, 2), QuitDecision::QuitNow);
        assert_eq!(quit_decision(false, 0), QuitDecision::QuitNow);
    }

    #[test]
    fn ctrl_shift_q_confirms_whenever_anything_would_die() {
        assert_eq!(stop_decision(0, 0), StopDecision::StopNow);
        assert_eq!(stop_decision(1, 0), StopDecision::Confirm);
        assert_eq!(stop_decision(0, 1), StopDecision::Confirm);
    }
}
