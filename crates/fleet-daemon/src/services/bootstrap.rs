//! Remote daemon bootstrap job skeleton.

use fleet_core::ids::{HostId, JobId};

use crate::{DaemonError, DaemonResult};

/// Starts remote build-and-install jobs.
#[derive(Default)]
pub struct Bootstrap;

impl Bootstrap {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub async fn start(&self, _host: HostId, _git_ref: Option<String>) -> DaemonResult<JobId> {
        Err(DaemonError::Unsupported(
            "Bootstrap::start: not implemented".to_owned(),
        ))
    }
}
