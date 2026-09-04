//! Environment and installation diagnostic contracts.

use std::sync::Arc;

use fleet_proto::{job::JobRecord, response::DoctorCheck};

use crate::{DaemonError, DaemonResult, jobs::JobManager};
use crate::{
    adapters::{files::Files, git::Git, github::Github, shell::Shell},
    stores::config::ConfigStore,
};

/// Environment diagnostics and self-update service.
#[derive(Clone)]
pub struct Doctor {
    _jobs: Arc<JobManager>,
    _config: Arc<ConfigStore>,
    _shell: Arc<dyn Shell>,
    _git: Arc<dyn Git>,
    _github: Arc<dyn Github>,
    _files: Arc<dyn Files>,
}

impl Doctor {
    /// Creates the diagnostics service.
    #[must_use]
    pub fn new(
        jobs: Arc<JobManager>,
        config: Arc<ConfigStore>,
        shell: Arc<dyn Shell>,
        git: Arc<dyn Git>,
        github: Arc<dyn Github>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            _jobs: jobs,
            _config: config,
            _shell: shell,
            _git: git,
            _github: github,
            _files: files,
        }
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
