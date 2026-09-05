//! The status bar's job ticker, and the rule that decides who owns that slot.
//!
//! §2.2 gives the status bar four slots and one arbitration: the **sticky error slot replaces
//! the ticker when present**. That single sentence is the whole content rule of §3.11's
//! neighbourhood, and it is the thing a status bar gets wrong if every screen re-derives it.
//! [`status_slot`] decides it once; [`crate::views::sticky_error`] owns the other half.

use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use fleet_ui_kit::{JobTicker, Tone};
use gpui::{AnyElement, IntoElement, SharedString};

use crate::{
    shell::{domain_target, job_kind_label, job_target},
    state::{StickyError, parse_percent, running_jobs},
};

/// What the ticker says: `⟳ <kind> <target> <pct>` with `+n` when more jobs run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickerContent {
    /// The fixed job-kind slug.
    pub kind: String,
    /// The real domain id the job is about — never a synthetic key.
    pub target: String,
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
pub fn ticker_content(jobs: &[JobRecord]) -> Option<TickerContent> {
    let running = running_jobs(jobs);
    let newest = running
        .iter()
        .max_by(|left, right| left.started_at.cmp(&right.started_at))?;
    Some(TickerContent {
        kind: job_kind_label(&newest.kind).to_owned(),
        target: job_target(&newest.kind, &newest.target).to_owned(),
        percent: newest.progress.as_deref().and_then(parse_percent),
        extra: running.len() - 1,
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

/// Whether the toast law (§2.7) allows announcing a job outcome as a toast.
///
/// The law is *"a toast is allowed only when there is no row and no pill that already shows the
/// outcome"*, and §2.7's "never a toast" list names *"job succeeded when its row is on screen"*
/// outright. Two things follow, and both are conditions here:
///
/// 1. The Jobs panel must be closed, so the row really is off screen.
/// 2. The job must be one the **user** started. The daemon's own cadence — PR fetches every
///    `github.prTtlSeconds`, pool builds, status refreshes, inspects — has no news in it, and
///    toasting it turned entering the PR screen into two toasts every 90 seconds.
///
/// A failure is never a toast: it is sticky (§1.8).
#[must_use]
pub fn job_outcome_toast(job: &JobRecord, jobs_panel_open: bool) -> Option<String> {
    if jobs_panel_open || !matches!(job.status, JobStatus::Succeeded) {
        return None;
    }
    let target = domain_target(&job.target);
    match &job.kind {
        JobKind::Clone => Some(format!("Cloned {target} \u{00b7} J")),
        JobKind::CreateWorktree => Some(format!("Created {target} \u{00b7} J")),
        JobKind::DeleteRepo | JobKind::DeleteWorktree => {
            Some(format!("Deleted {target} \u{00b7} J"))
        }
        JobKind::Import => Some("Imported from ~/.swarm \u{00b7} J".to_owned()),
        JobKind::Update => Some("Fleet updated \u{00b7} J".to_owned()),
        // Background cadence: the ticker and the Jobs panel already say all there is to say.
        JobKind::PoolBuild
        | JobKind::PoolRefresh
        | JobKind::Prune
        | JobKind::Inspect
        | JobKind::PostCreateHooks
        | JobKind::PrFetch
        | JobKind::RepoFetch
        | JobKind::RepoDiscovery
        | JobKind::Custom(_) => None,
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

/// The one-line summary a dialog or a quit confirm uses to name in-flight work.
#[must_use]
pub fn running_summary(jobs: &[JobRecord]) -> Option<SharedString> {
    let content = ticker_content(jobs)?;
    let text = if content.extra == 0 {
        format!("{} {}", content.kind, content.target)
    } else {
        format!("{} {} +{}", content.kind, content.target, content.extra)
    };
    Some(SharedString::from(text))
}

/// The tone of a ticker line. Always amber: in flight is "needs attention", never "done".
#[must_use]
pub const fn ticker_tone() -> Tone {
    Tone::Warning
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_running_summary_counts_the_rest() {
        let jobs = vec![
            job("job-a", JobStatus::Running, "2026-09-04T12:00:00Z", None),
            job("job-b", JobStatus::Running, "2026-09-04T12:01:00Z", None),
        ];
        assert_eq!(
            running_summary(&jobs).map(|s| s.to_string()),
            Some("clone nixos +1".to_owned())
        );
        assert_eq!(running_summary(&[]), None);
    }
}
