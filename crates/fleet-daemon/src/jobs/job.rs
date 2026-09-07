//! Runtime representation and execution contract for a daemon job.

use std::{path::Path, sync::Arc};

use fleet_core::ids::JobId;
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonResult,
    adapters::shell::{DetachedProcess, Shell, ShellCommand, ShellResult},
    jobs::manager::JobManager,
};

/// Execution context supplied to one detached daemon job.
#[derive(Clone)]
pub struct JobCtx {
    /// Stable identifier of the running job.
    pub id: JobId,
    /// Explicit cancellation signal owned by the job manager.
    pub cancel: CancellationToken,
    pub(crate) manager: JobManager,
}

impl JobCtx {
    /// Records a progress line, appends it to the job log, and broadcasts the changed record.
    pub fn progress(&self, line: impl Into<String>) -> DaemonResult<()> {
        self.manager.record_progress(&self.id, line.into())
    }

    /// Runs a streaming child whose lines become job progress and whose lifetime follows cancellation.
    pub async fn spawn_child(
        &self,
        shell: Arc<dyn Shell>,
        command: ShellCommand,
    ) -> DaemonResult<ShellResult> {
        let context = self.clone();
        shell
            .run_streaming(
                command,
                self.cancel.clone(),
                Arc::new(move |line| {
                    if let Err(error) = context.progress(line) {
                        tracing::warn!(%error, job = %context.id, "failed to record child progress");
                    }
                }),
            )
            .await
    }

    /// Starts a detached child writing directly to the supplied log path.
    pub async fn spawn_detached_child(
        &self,
        shell: Arc<dyn Shell>,
        command: ShellCommand,
        log_path: &Path,
    ) -> DaemonResult<DetachedProcess> {
        if self.cancel.is_cancelled() {
            return Err(crate::DaemonError::Cancelled);
        }
        self.manager.flush_log(&self.id)?;
        shell.run_detached(command, log_path).await
    }
}
