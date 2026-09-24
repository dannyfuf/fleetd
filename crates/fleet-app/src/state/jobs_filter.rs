//! Jobs-panel filter state shared by the panel and harness projection.

use fleet_proto::job::{JobRecord, JobStatus};

use crate::presentation::{is_active, is_user_visible_job};

/// The four positions of the Jobs panel's filter (§3.7: all → running → failed → done), picked
/// on its segmented control or cycled with `f`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobFilter {
    /// Every retained job.
    #[default]
    All,
    /// Queued, running and cancelling jobs.
    Running,
    /// Failed jobs only.
    Failed,
    /// Succeeded and cancelled jobs: the evidence of what ran.
    Done,
}

impl JobFilter {
    /// Every position, in the order the segmented control draws them and `f` cycles them.
    pub const ALL: [Self; 4] = [Self::All, Self::Running, Self::Failed, Self::Done];

    /// The next position of the cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::All => Self::Running,
            Self::Running => Self::Failed,
            Self::Failed => Self::Done,
            Self::Done => Self::All,
        }
    }

    /// This position's index in [`Self::ALL`]: its segment, and its `jobs.filter[N]`.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::All => 0,
            Self::Running => 1,
            Self::Failed => 2,
            Self::Done => 3,
        }
    }

    /// The word the harness projection reports, or `None` for the unfiltered default (§1.2).
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Running => Some("running"),
            Self::Failed => Some("failed"),
            Self::Done => Some("done"),
        }
    }

    /// The segment's title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Running => "Running",
            Self::Failed => "Failed",
            Self::Done => "Done",
        }
    }

    /// Whether a job passes this filter.
    #[must_use]
    pub fn matches(self, job: &JobRecord) -> bool {
        if !is_user_visible_job(job) {
            return false;
        }
        match self {
            Self::All => true,
            Self::Running => is_active(&job.status),
            Self::Failed => matches!(job.status, JobStatus::Failed { .. }),
            Self::Done => is_finished(&job.status),
        }
    }

    /// The jobs this filter shows, in the order the panel draws them: the live and failed jobs
    /// first, then the quiet "Finished" group, each in the daemon's order.
    ///
    /// The cursor, the rows, the harness projection and `jobs.row[N]` all index this one
    /// sequence, so they cannot disagree about which job is row `N`.
    pub fn visible(self, jobs: &[JobRecord]) -> impl Iterator<Item = &JobRecord> {
        let open = jobs
            .iter()
            .filter(move |job| self.matches(job) && !is_finished(&job.status));
        let finished = jobs
            .iter()
            .filter(move |job| self.matches(job) && is_finished(&job.status));
        open.chain(finished)
    }
}

/// Succeeded or cancelled: over, and nothing left to do about it. A failure is over too, but it
/// stays in the upper group until it is retried or cleared.
#[must_use]
pub const fn is_finished(status: &JobStatus) -> bool {
    matches!(status, JobStatus::Succeeded | JobStatus::Cancelled)
}
