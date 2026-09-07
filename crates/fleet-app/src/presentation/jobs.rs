use super::parse_timestamp;
use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use fleet_ui_kit::{JobStatus as RowStatus, format_age};
use gpui::SharedString;

const UUID_LEN: usize = 36;
const ELAPSED_AFTER_SECONDS: i64 = 30;

pub fn job_kind_label(kind: &JobKind) -> &str {
    match kind {
        JobKind::Clone => "clone",
        JobKind::PoolBuild => "pool",
        JobKind::PoolRefresh => "refresh",
        JobKind::CreateWorktree => "create",
        JobKind::DeleteWorktree => "delete",
        JobKind::DeleteRepo => "delete repo",
        JobKind::Prune => "prune",
        JobKind::Inspect => "inspect",
        JobKind::PostCreateHooks => "hooks",
        JobKind::PrFetch => "prs",
        JobKind::RepoFetch => "fetch",
        JobKind::RepoDiscovery => "discover",
        JobKind::Update => "update",
        JobKind::Import => "import",
        JobKind::Custom(name) => name,
    }
}

fn domain_target(target: &str) -> &str {
    let Some(head_end) = target.len().checked_sub(UUID_LEN + 1) else {
        return target;
    };
    if !target.is_char_boundary(head_end) {
        return target;
    }
    if !matches!(target.as_bytes()[head_end], b':' | b'-') {
        return target;
    }
    if !is_uuid(&target[head_end + 1..]) {
        return target;
    }
    let head = &target[..head_end];
    if head.is_empty() { target } else { head }
}

pub fn job_target<'job>(kind: &JobKind, target: &'job str) -> &'job str {
    let target = domain_target(target);
    if target.eq_ignore_ascii_case(job_kind_label(kind)) {
        ""
    } else {
        target
    }
}

fn is_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != UUID_LEN {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

pub const fn is_active(status: &JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
    )
}

pub const fn is_dismissable(status: &JobStatus) -> bool {
    !is_active(status)
}

const fn row_status(status: &JobStatus) -> RowStatus {
    match status {
        JobStatus::Queued => RowStatus::Queued,
        JobStatus::Running => RowStatus::Running,
        JobStatus::Cancelling => RowStatus::Cancelling,
        JobStatus::Cancelled => RowStatus::Cancelled,
        JobStatus::Succeeded => RowStatus::Done,
        JobStatus::Failed { .. } => RowStatus::Failed,
    }
}

pub fn sub_line(job: &JobRecord) -> Option<&str> {
    match &job.status {
        JobStatus::Failed { error } => Some(error.as_str()),
        status if is_active(status) => job.progress.as_deref(),
        _ => None,
    }
}

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

pub fn parse_percent(progress: &str) -> Option<u8> {
    let (head, _) = progress.split_once('%')?;
    let start = head
        .trim_end_matches(|character: char| character.is_ascii_digit())
        .len();
    head[start..]
        .parse::<u16>()
        .ok()
        .map(|value| value.min(100) as u8)
}

/// Prepared at job-update boundaries; elapsed text alone depends on the current clock.
#[derive(Debug, Clone)]
pub struct JobDisplay {
    pub element_id: SharedString,
    pub kind: SharedString,
    pub target: SharedString,
    pub status: RowStatus,
    pub progress: Option<SharedString>,
    pub percent: Option<u8>,
    pub retryable: bool,
    started_at: Option<i64>,
    // None is still running; Some(None) is an unparseable completion time.
    finished_at: Option<Option<i64>>,
    active: bool,
    cancelled: bool,
}

impl JobDisplay {
    pub fn new(job: &JobRecord) -> Self {
        Self {
            element_id: format!("job-{}", job.id).into(),
            kind: job_kind_label(&job.kind).to_owned().into(),
            target: job_target(&job.kind, &job.target).to_owned().into(),
            status: row_status(&job.status),
            progress: sub_line(job).map(|text| text.to_owned().into()),
            percent: if is_active(&job.status) {
                job.progress.as_deref().and_then(parse_percent)
            } else {
                None
            },
            retryable: matches!(job.status, JobStatus::Failed { .. }) && job.retryable,
            started_at: parse_timestamp(&job.started_at),
            finished_at: job.finished_at.as_deref().map(parse_timestamp),
            active: is_active(&job.status),
            cancelled: matches!(job.status, JobStatus::Cancelled),
        }
    }

    pub fn elapsed_seconds(&self, now: i64) -> Option<i64> {
        let end = self.finished_at.unwrap_or(Some(now))?;
        Some(end.saturating_sub(self.started_at?).max(0))
    }

    pub fn elapsed_label(&self, now: i64) -> Option<String> {
        if self.cancelled {
            return Some("–".to_owned());
        }
        let seconds = self.elapsed_seconds(now)?;
        if self.active {
            (seconds >= ELAPSED_AFTER_SECONDS)
                .then(|| format!("{}:{:02}", seconds / 60, seconds % 60))
        } else {
            Some(format_age(seconds))
        }
    }
}

/// Most-recent active job and total count, without a temporary collection.
pub fn active_job_summary(jobs: &[JobRecord]) -> Option<(&JobRecord, usize)> {
    jobs.iter()
        .filter(|job| is_active(&job.status))
        .fold(None, |summary, job| {
            let Some((newest, count)) = summary else {
                return Some((job, 1));
            };
            Some((
                if job.started_at >= newest.started_at {
                    job
                } else {
                    newest
                },
                count + 1,
            ))
        })
}

/// The caller supplies its acknowledgement set without requiring a particular storage type.
pub fn latest_unseen_failure(
    jobs: &[JobRecord],
    mut seen: impl FnMut(&fleet_core::ids::JobId) -> bool,
) -> Option<&JobRecord> {
    jobs.iter()
        .filter(|job| matches!(job.status, JobStatus::Failed { .. }) && !seen(&job.id))
        .max_by(|left, right| left.finished_at.cmp(&right.finished_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(id: &str, status: JobStatus) -> JobRecord {
        JobRecord {
            id: id.parse().expect("job id"),
            kind: JobKind::Clone,
            target: "acme/api".into(),
            title: "Clone api".into(),
            status,
            progress: Some("Receiving 40%".into()),
            log_path: "/tmp/job.log".into(),
            started_at: "1970-01-01T00:00:00Z".into(),
            finished_at: None,
            cancellable: true,
            retryable: true,
        }
    }
    #[test]
    fn display_reuses_parsed_time_and_preserves_elapsed_policy() {
        let running = JobDisplay::new(&job("running", JobStatus::Running));
        assert_eq!(running.elapsed_label(29), None);
        assert_eq!(running.elapsed_label(65).as_deref(), Some("1:05"));
        assert_eq!(running.percent, Some(40));
        assert_eq!(running.target.as_ref(), "acme/api");
        let mut finished = job("finished", JobStatus::Succeeded);
        finished.finished_at = Some("1970-01-01T00:01:00Z".into());
        let display = JobDisplay::new(&finished);
        assert_eq!(display.elapsed_seconds(500), Some(60));
        assert_eq!(display.progress, None);
        finished.finished_at = Some("bad".into());
        assert_eq!(JobDisplay::new(&finished).elapsed_seconds(500), None);
        assert_eq!(
            JobDisplay::new(&job("cancelled", JobStatus::Cancelled))
                .elapsed_label(500)
                .as_deref(),
            Some("–")
        );
    }
    #[test]
    fn targets_and_percentages_preserve_existing_parsing() {
        assert_eq!(
            domain_target("acme/api#branch:762d2efa-4911-4a0e-8b1c-8f3e0d5b2a91"),
            "acme/api#branch"
        );
        assert_eq!(domain_target("acme/api#abc123"), "acme/api#abc123");
        assert_eq!(job_target(&JobKind::Inspect, "inspect"), "");
        assert_eq!(parse_percent("é 102%"), Some(100));
        assert_eq!(parse_percent("40 %"), None);
        assert_eq!(parse_percent("65536%"), None);
    }
    #[test]
    fn dismissal_and_sub_line_follow_job_status() {
        for status in [JobStatus::Queued, JobStatus::Running, JobStatus::Cancelling] {
            assert!(!is_dismissable(&status), "{status:?}");
        }
        for status in [
            JobStatus::Succeeded,
            JobStatus::Cancelled,
            JobStatus::Failed {
                error: "boom".into(),
            },
        ] {
            assert!(is_dismissable(&status), "{status:?}");
        }
        let running = job("running", JobStatus::Running);
        assert_eq!(sub_line(&running), Some("Receiving 40%"));
        assert_eq!(sub_line(&job("done", JobStatus::Succeeded)), None);
        assert_eq!(
            sub_line(&job(
                "failed",
                JobStatus::Failed {
                    error: "no space left".into(),
                },
            )),
            Some("no space left")
        );
    }

    #[test]
    fn active_summary_keeps_daemon_order_for_timestamp_ties() {
        let jobs = [
            job("one", JobStatus::Queued),
            job("two", JobStatus::Cancelling),
            job("three", JobStatus::Succeeded),
        ];
        let (newest, count) = active_job_summary(&jobs).expect("active jobs");
        assert_eq!(newest.id.as_str(), "two");
        assert_eq!(count, 2);
        let failed = [
            job(
                "old",
                JobStatus::Failed {
                    error: "old".into(),
                },
            ),
            job(
                "new",
                JobStatus::Failed {
                    error: "new".into(),
                },
            ),
        ];
        let unseen =
            latest_unseen_failure(&failed, |id| id.as_str() == "new").expect("unseen failure");
        assert_eq!(unseen.id.as_str(), "old");
        assert!(job_outcome_toast(&failed[0], false).is_none());
    }
}
