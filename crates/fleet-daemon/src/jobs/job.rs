//! Runtime representation and execution contract for a daemon job.

use std::{
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
};

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
    pub(crate) cleanup: CleanupTracker,
}

#[derive(Clone)]
pub(crate) struct CleanupTracker {
    inner: Arc<CleanupTrackerInner>,
}

struct CleanupTrackerInner {
    registration: Mutex<Option<tokio::sync::mpsc::Sender<()>>>,
    completions: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<()>>,
}

impl Default for CleanupTracker {
    fn default() -> Self {
        let (registration, completions) = tokio::sync::mpsc::channel(1);
        Self {
            inner: Arc::new(CleanupTrackerInner {
                registration: Mutex::new(Some(registration)),
                completions: tokio::sync::Mutex::new(completions),
            }),
        }
    }
}

impl CleanupTracker {
    pub(crate) fn spawn(&self, cleanup: impl Future<Output = ()> + Send + 'static) {
        let registration = self
            .inner
            .registration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .cloned();
        let Some(registration) = registration else {
            return;
        };
        tokio::spawn(async move {
            cleanup.await;
            drop(registration);
        });
    }

    pub(crate) async fn wait(&self) {
        let mut completions = self.inner.completions.lock().await;
        self.inner
            .registration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        while completions.recv().await.is_some() {}
    }
}

impl JobCtx {
    pub(crate) fn track_cleanup(&self, cleanup: impl Future<Output = ()> + Send + 'static) {
        self.cleanup.spawn(cleanup);
    }

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
