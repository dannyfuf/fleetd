//! Active-job ticker and sticky-error precedence.

use fleet_proto::job::JobRecord;
use fleet_ui_kit::JobTicker;
use gpui::{AnyElement, IntoElement, SharedString};

use crate::{
    presentation::{active_job_summary, job_kind_label, job_target, parse_percent},
    state::StickyError,
};

/// What the ticker says: `⟳ <kind> <target> <pct>` with `+n` when more jobs run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickerContent {
    /// The fixed job-kind slug.
    pub kind: SharedString,
    /// The real domain id the job is about — never a synthetic key.
    pub target: SharedString,
    /// The percent, when the last progress line contains one.
    pub percent: Option<u8>,
    /// How many other jobs are running. Rendered `+n`, zero-suppressed.
    pub extra: usize,
}

/// The newest running job, or `None` when the daemon is idle (§1.2: a calm bar means calm).
///
/// "Newest" is by `started_at`, which is the only ordering the wire guarantees; ties keep the
/// daemon's own order, so the ticker never flickers between two jobs started in one tick.
#[must_use]
fn ticker_content(jobs: &[JobRecord]) -> Option<TickerContent> {
    let (newest, count) = active_job_summary(jobs)?;
    Some(TickerContent {
        kind: SharedString::new(job_kind_label(&newest.kind)),
        target: SharedString::new(job_target(&newest.kind, &newest.target)),
        percent: newest.progress.as_deref().and_then(parse_percent),
        extra: count - 1,
    })
}

/// Who owns the shared ticker / error slot of the status bar (§2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusSlot {
    /// A sticky error is showing. It always wins: errors never scroll past (§1.8).
    Error(StickyError),
    /// Work is in flight and nothing has failed.
    Ticker(TickerContent),
    /// Nothing to say.
    Idle,
}

/// Applies the §2.2 arbitration: the sticky error replaces the ticker whenever one exists.
#[must_use]
pub fn status_slot(jobs: &[JobRecord], sticky: Option<&StickyError>) -> StatusSlot {
    match sticky {
        Some(error) => StatusSlot::Error(error.clone()),
        None => ticker_content(jobs).map_or(StatusSlot::Idle, StatusSlot::Ticker),
    }
}

/// Renders the ticker.
#[must_use]
pub fn render(content: &TickerContent) -> AnyElement {
    let mut ticker =
        JobTicker::new(content.kind.clone(), content.target.clone()).extra(content.extra);
    if let Some(percent) = content.percent {
        ticker = ticker.percent(percent);
    }
    ticker.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::job_outcome_toast;
    use fleet_proto::job::{JobKind, JobStatus};

    fn job(id: &str, status: JobStatus, started: &str, progress: Option<&str>) -> JobRecord {
        JobRecord {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            kind: JobKind::Clone,
            target: "nixos".to_owned(),
            title: "Clone nixos".to_owned(),
            status,
            progress: progress.map(str::to_owned),
            log_path: "/tmp/j.log".to_owned(),
            started_at: started.to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: true,
        }
    }

    #[test]
    fn the_ticker_is_the_newest_running_job() {
        let jobs = vec![
            job(
                "job-a",
                JobStatus::Running,
                "2026-09-04T12:00:00Z",
                Some("Receiving objects: 40% (81/202)"),
            ),
            job("job-b", JobStatus::Running, "2026-09-04T12:01:00Z", None),
            job("job-c", JobStatus::Succeeded, "2026-09-04T12:02:00Z", None),
        ];
        let content = ticker_content(&jobs).unwrap_or_else(|| panic!("expected a ticker"));
        assert_eq!(content.kind, "clone");
        assert_eq!(
            content.extra, 1,
            "the other running job is counted, not shown"
        );
        assert_eq!(content.percent, None);
    }

    #[test]
    fn an_idle_daemon_leaves_the_slot_empty() {
        assert_eq!(ticker_content(&[]), None);
        assert_eq!(status_slot(&[], None), StatusSlot::Idle);
    }

    #[test]
    fn a_sticky_error_replaces_the_ticker() {
        let jobs = vec![job(
            "job-a",
            JobStatus::Running,
            "2026-09-04T12:00:00Z",
            None,
        )];
        assert!(matches!(status_slot(&jobs, None), StatusSlot::Ticker(_)));

        let error = StickyError {
            text: "gh: HTTP 502".to_owned(),
            job: None,
            retryable: true,
        };
        assert_eq!(
            status_slot(&jobs, Some(&error)),
            StatusSlot::Error(error),
            "an error must never be pushed off the bar by a running job"
        );
    }

    #[test]
    fn the_percent_comes_from_the_last_progress_line() {
        let jobs = vec![job(
            "job-a",
            JobStatus::Running,
            "2026-09-04T12:00:00Z",
            Some("Receiving objects:  7% (14/202)"),
        )];
        let content = ticker_content(&jobs).unwrap_or_else(|| panic!("expected a ticker"));
        assert_eq!(content.percent, Some(7));
    }

    #[test]
    fn only_an_off_screen_success_earns_a_toast() {
        let succeeded = job("job-a", JobStatus::Succeeded, "2026-09-04T12:00:00Z", None);
        assert_eq!(
            job_outcome_toast(&succeeded, false),
            Some("Cloned nixos \u{00b7} J".to_owned())
        );
        assert_eq!(
            job_outcome_toast(&succeeded, true),
            None,
            "the panel already shows the row, so the toast is forbidden"
        );

        let failed = job(
            "job-b",
            JobStatus::Failed {
                error: "boom".to_owned(),
            },
            "2026-09-04T12:00:00Z",
            None,
        );
        assert_eq!(
            job_outcome_toast(&failed, false),
            None,
            "errors are sticky, never transient"
        );

        let running = job("job-c", JobStatus::Running, "2026-09-04T12:00:00Z", None);
        assert_eq!(job_outcome_toast(&running, false), None);
    }

    #[test]
    fn the_daemons_own_cadence_never_toasts() {
        // §2.7: a PR fetch runs every `github.prTtlSeconds`; announcing it is noise, and its
        // target is a synthetic key rather than anything the user recognises.
        for kind in [
            JobKind::PrFetch,
            JobKind::PoolBuild,
            JobKind::PoolRefresh,
            JobKind::Inspect,
            JobKind::RepoFetch,
            JobKind::RepoDiscovery,
            JobKind::PostCreateHooks,
            JobKind::Prune,
        ] {
            let mut background = job("job-d", JobStatus::Succeeded, "2026-09-04T12:00:00Z", None);
            background.kind = kind.clone();
            assert_eq!(
                job_outcome_toast(&background, false),
                None,
                "{kind:?} must not toast"
            );
        }
    }

    #[test]
    fn a_toast_names_the_domain_id_never_the_job_id() {
        let mut succeeded = job("job-a", JobStatus::Succeeded, "2026-09-04T12:00:00Z", None);
        succeeded.kind = JobKind::CreateWorktree;
        succeeded.target =
            "acme/widgets#feature-one:762d2efa-4911-4a0e-8b1c-8f3e0d5b2a91".to_owned();
        assert_eq!(
            job_outcome_toast(&succeeded, false),
            Some("Created acme/widgets#feature-one \u{00b7} J".to_owned())
        );
    }
}
