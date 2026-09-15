//! Jobs-panel filter state shared by the panel and harness projection.

use fleet_proto::job::{JobRecord, JobStatus};

use crate::presentation::is_active;

/// The three positions of the `f` filter (§3.7: all → running → failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobFilter {
    /// Every retained job.
    #[default]
    All,
    /// Queued, running and cancelling jobs.
    Running,
    /// Failed jobs only.
    Failed,
}

impl JobFilter {
    /// The next position of the cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::All => Self::Running,
            Self::Running => Self::Failed,
            Self::Failed => Self::All,
        }
    }

    /// The word the pane header shows, or `None` for the unfiltered default (§1.2).
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Running => Some("running"),
            Self::Failed => Some("failed"),
        }
    }

    /// Whether a job passes this filter.
    #[must_use]
    pub fn matches(self, job: &JobRecord) -> bool {
        match self {
            Self::All => true,
            Self::Running => is_active(&job.status),
            Self::Failed => matches!(job.status, JobStatus::Failed { .. }),
        }
    }
}
