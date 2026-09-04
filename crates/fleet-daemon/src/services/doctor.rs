//! Environment and installation diagnostic contracts.

use std::sync::Arc;

use fleet_proto::{job::JobRecord, response::DoctorCheck};

use crate::{DaemonError, DaemonResult, jobs::JobManager};

/// Environment diagnostics and self-update service.
#[derive(Clone)]
pub struct Doctor {
    _jobs: Arc<JobManager>,
}

impl Doctor {
    /// Creates the diagnostics service.
    #[must_use]
    pub fn new(jobs: Arc<JobManager>) -> Self {
        Self { _jobs: jobs }
    }

    /// Checks exact external dependencies including `gh auth status` (inventory section 7).
    pub async fn check(&self) -> DaemonResult<Vec<DoctorCheck>> {
        Err(DaemonError::Unimplemented("doctor::check"))
    }

    /// Starts the source update/package/build workflow as a detached job (inventory sections 6 and 7).
    pub async fn update(&self) -> DaemonResult<JobRecord> {
        Err(DaemonError::Unimplemented("doctor::update"))
    }
}
