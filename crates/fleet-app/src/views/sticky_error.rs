//! The sticky error slot of §1.8: the last failure, addressable with `!`, retryable with `R`.
//!
//! Three rules from the spec are encoded here rather than left to each caller:
//!
//! 1. **An error is never a toast.** It takes the status-bar slot and stays until it is
//!    dismissed or superseded (§1.8, §2.7 "Never a toast … and any error").
//! 2. **The focus key carries its prefix.** In the Hub it is `!`; over a terminal grid the
//!    Workspace has exactly one escape key, so it is `^s !` — never a bare `!` (§3.6 [D-8],
//!    `KEYMAP.md` "No bare keys over a terminal").
//! 3. **`R` is offered only when the daemon says the job can be retried.** Offering a retry
//!    that cannot happen is a worse failure than offering none.

use fleet_core::ids::JobId;
use fleet_proto::job::{JobRecord, JobStatus};
use fleet_ui_kit::{KeyHintRow, StickyErrorSlot};
use gpui::{AnyElement, IntoElement, SharedString};

use crate::state::{Screen, StickyError};

/// The key that focuses the slot from the current screen.
///
/// `KEYMAP.md` binds `!` in `Hub` and `ctrl-s !` in `Workspace > Prefix`; the affordance drawn
/// over a terminal grid must spell the chord it needs, not the one the Hub uses.
#[must_use]
pub const fn focus_key(screen: &Screen) -> &'static str {
    match screen {
        Screen::Hub { .. } => "!",
        Screen::Workspace { .. } => "^s !",
    }
}

/// Derives the slot's content from the job list, skipping failures the user has already seen.
///
/// [D-9]: a failed job holds the red jobs chip and this slot *until the Jobs panel has been
/// opened*; the row itself survives until `D`. `seen` is that set of acknowledged failures.
#[must_use]
pub fn sticky_error_for(jobs: &[JobRecord], seen: &[JobId]) -> Option<StickyError> {
    jobs.iter()
        .filter(|job| matches!(job.status, JobStatus::Failed { .. }))
        .filter(|job| !seen.contains(&job.id))
        .max_by(|left, right| left.finished_at.cmp(&right.finished_at))
        .map(|job| StickyError {
            text: match &job.status {
                JobStatus::Failed { error } => error.clone(),
                _ => job.title.clone(),
            },
            job: Some(job.id.clone()),
            retryable: job.retryable,
        })
}

/// One line of at most `budget` characters, so a long `gh` error cannot push the mode word out
/// of the status bar. Truncation is at the tail, because the head of an error names its cause.
#[must_use]
pub fn one_line(text: &str, budget: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= budget {
        return collapsed;
    }
    let head: String = collapsed.chars().take(budget.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

/// The status-bar slot itself.
#[must_use]
pub fn render(error: &StickyError, screen: &Screen) -> AnyElement {
    StickyErrorSlot::new(SharedString::from(one_line(&error.text, MAX_LINE)))
        .key(focus_key(screen))
        .into_any_element()
}

/// How much of an error the 26 px status bar can carry before the mode word is at risk.
pub const MAX_LINE: usize = 72;

/// The keys the Jobs panel offers on the failed job the slot points at (§1.8).
#[must_use]
pub fn actions(error: &StickyError) -> KeyHintRow {
    let mut hints = KeyHintRow::new();
    if error.retryable {
        hints = hints.key("R", "retry");
    }
    hints.key("y", "copy log path").key("D", "dismiss")
}

/// Whether `R` on the sticky error can do anything at all.
#[must_use]
pub const fn is_retryable(error: &StickyError) -> bool {
    error.retryable && error.job.is_some()
}

#[cfg(test)]
mod tests {
    use fleet_core::ids::SessionId;
    use fleet_proto::job::JobKind;

    use super::*;
    use crate::state::HubTab;

    fn failed(id: &str, error: &str, finished: &str, retryable: bool) -> JobRecord {
        JobRecord {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            kind: JobKind::PrFetch,
            target: "review".to_owned(),
            title: "Fetch pull requests".to_owned(),
            status: JobStatus::Failed {
                error: error.to_owned(),
            },
            progress: None,
            log_path: "/tmp/j.log".to_owned(),
            started_at: "2026-09-04T12:00:00Z".to_owned(),
            finished_at: Some(finished.to_owned()),
            cancellable: false,
            retryable,
        }
    }

    #[test]
    fn the_newest_failure_owns_the_slot() {
        let jobs = vec![
            failed("job-a", "older", "2026-09-04T12:00:00Z", true),
            failed("job-b", "newer", "2026-09-04T12:05:00Z", true),
        ];
        let error = sticky_error_for(&jobs, &[]).unwrap_or_else(|| panic!("expected an error"));
        assert_eq!(error.text, "newer");
        assert!(is_retryable(&error));
    }

    #[test]
    fn an_acknowledged_failure_releases_the_slot() {
        let jobs = vec![failed("job-a", "gh: HTTP 502", "2026-09-04T12:00:00Z", true)];
        let seen = vec![jobs[0].id.clone()];
        assert_eq!(sticky_error_for(&jobs, &seen), None);
    }

    #[test]
    fn a_healthy_job_list_has_no_sticky_error() {
        assert_eq!(sticky_error_for(&[], &[]), None);
    }

    #[test]
    fn the_focus_key_carries_the_prefix_over_a_terminal() {
        let hub = Screen::Hub {
            tab: HubTab::Worktrees,
        };
        assert_eq!(focus_key(&hub), "!");
        let workspace = Screen::Workspace {
            session: SessionId::try_from("payroll/feat-rut")
                .unwrap_or_else(|error| panic!("{error}")),
        };
        assert_eq!(
            focus_key(&workspace),
            "^s !",
            "a bare `!` over a terminal would go to the PTY"
        );
    }

    #[test]
    fn a_long_error_is_collapsed_to_one_line() {
        let long = "gh: HTTP 502\n  upstream connect error or disconnect/reset before headers";
        let line = one_line(long, 40);
        assert!(!line.contains('\n'));
        assert_eq!(line.chars().count(), 40);
        assert!(line.ends_with('\u{2026}'));
        assert_eq!(one_line("short", 40), "short");
    }

    #[test]
    fn retry_is_offered_only_when_the_daemon_allows_it() {
        let retryable = StickyError {
            text: "boom".to_owned(),
            job: Some("job-a".parse().unwrap_or_else(|error| panic!("{error}"))),
            retryable: true,
        };
        assert!(is_retryable(&retryable));

        let not_retryable = StickyError {
            retryable: false,
            ..retryable.clone()
        };
        assert!(!is_retryable(&not_retryable));

        let no_job = StickyError {
            job: None,
            ..retryable
        };
        assert!(!is_retryable(&no_job));
    }
}
