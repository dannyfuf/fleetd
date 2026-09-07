//! One-shot delivery of a submitted job's value to the caller awaiting it.
//!
//! Request handlers that must answer with a job's result submit the work through
//! [`JobManager`](crate::jobs::JobManager) and wait here. `DaemonError` is not `Clone`,
//! so a failure reaches only one of the two sides intact: the constructor picks which,
//! and the other side receives what `reproduce` derives from it.

use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::{DaemonError, DaemonResult};

type Reproduce = fn(&DaemonError) -> DaemonError;

/// Sender half retained by the submitted job. Cloneable so it can live in a retryable
/// operation; only the first delivery is sent.
pub(crate) struct JobDelivery<T> {
    sender: Arc<Mutex<Option<oneshot::Sender<DaemonResult<T>>>>>,
    reproduce: Reproduce,
    job_keeps_original: bool,
}

impl<T> Clone for JobDelivery<T> {
    fn clone(&self) -> Self {
        Self {
            sender: Arc::clone(&self.sender),
            reproduce: self.reproduce,
            job_keeps_original: self.job_keeps_original,
        }
    }
}

/// Receiver half held by the awaiting caller.
pub(crate) struct AwaitedJob<T> {
    receiver: oneshot::Receiver<DaemonResult<T>>,
}

impl<T> JobDelivery<T> {
    /// The job record keeps the original failure; the caller receives `reproduce`'s copy.
    pub(crate) fn caller_gets_copy(reproduce: Reproduce) -> (Self, AwaitedJob<T>) {
        Self::split(reproduce, true)
    }

    /// The caller keeps the original failure; the job record gets `reproduce`'s copy.
    pub(crate) fn job_gets_copy(reproduce: Reproduce) -> (Self, AwaitedJob<T>) {
        Self::split(reproduce, false)
    }

    fn split(reproduce: Reproduce, job_keeps_original: bool) -> (Self, AwaitedJob<T>) {
        let (sender, receiver) = oneshot::channel();
        (
            Self {
                sender: Arc::new(Mutex::new(Some(sender))),
                reproduce,
                job_keeps_original,
            },
            AwaitedJob { receiver },
        )
    }

    /// Hands `result` to the awaiting caller and returns the outcome the job reports.
    pub(crate) fn finish(&self, result: DaemonResult<T>) -> DaemonResult<()> {
        let error = match result {
            Ok(value) => {
                self.send(Ok(value));
                return Ok(());
            }
            Err(error) => error,
        };
        if self.job_keeps_original {
            self.send(Err((self.reproduce)(&error)));
            Err(error)
        } else {
            let recorded = (self.reproduce)(&error);
            self.send(Err(error));
            Err(recorded)
        }
    }

    fn send(&self, result: DaemonResult<T>) {
        if let Some(sender) = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ignored = sender.send(result);
        }
    }
}

impl<T> AwaitedJob<T> {
    /// Resolves once the job delivers an outcome, or as cancelled when it never does.
    pub(crate) async fn wait(self) -> DaemonResult<T> {
        self.receiver.await.map_err(|_| DaemonError::Cancelled)?
    }
}

/// Rebuilds a failure with its category intact so both sides of an awaited job agree.
pub(crate) fn copy_error(error: &DaemonError) -> DaemonError {
    match error {
        DaemonError::NotFound(value) => DaemonError::NotFound(value.clone()),
        DaemonError::Conflict(value) => DaemonError::Conflict(value.clone()),
        DaemonError::Validation(value) => DaemonError::Validation(value.clone()),
        DaemonError::Filesystem { path, source } => {
            DaemonError::fs(path, std::io::Error::new(source.kind(), source.to_string()))
        }
        DaemonError::Json(value) => DaemonError::Validation(value.to_string()),
        DaemonError::Shell(value) => DaemonError::Shell(value.clone()),
        DaemonError::Git(value) => DaemonError::Git(value.clone()),
        DaemonError::Github(value) => DaemonError::Github(value.clone()),
        DaemonError::Process(value) => DaemonError::Process(value.clone()),
        DaemonError::Timeout(value) => DaemonError::Timeout(value.clone()),
        DaemonError::Cancelled => DaemonError::Cancelled,
        DaemonError::Protocol(value) => DaemonError::Protocol(value.clone()),
        DaemonError::Unimplemented(value) => DaemonError::Unimplemented(value),
        DaemonError::Unsupported(value) => DaemonError::Unsupported(value.clone()),
        DaemonError::Join(value) => DaemonError::Join(value.clone()),
    }
}

/// Flattens a failure into the Git category used by Git-facing jobs and their callers.
pub(crate) fn git_error(error: &DaemonError) -> DaemonError {
    DaemonError::Git(error.to_string())
}

/// Flattens a failure into the GitHub category promised by GitHub lookups.
pub(crate) fn github_error(error: &DaemonError) -> DaemonError {
    DaemonError::Github(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn caller_receives_a_copy_while_the_job_keeps_the_original() {
        let (delivery, awaited) = JobDelivery::<u8>::caller_gets_copy(copy_error);
        let recorded = delivery
            .finish(Err(DaemonError::Conflict("busy".to_owned())))
            .unwrap_err();
        assert!(matches!(recorded, DaemonError::Conflict(ref value) if value == "busy"));
        assert!(matches!(
            awaited.wait().await.unwrap_err(),
            DaemonError::Conflict(value) if value == "busy"
        ));
    }

    #[tokio::test]
    async fn job_records_the_derived_category_while_the_caller_keeps_the_original() {
        let (delivery, awaited) = JobDelivery::<u8>::job_gets_copy(git_error);
        let recorded = delivery
            .finish(Err(DaemonError::NotFound("worktree".to_owned())))
            .unwrap_err();
        assert!(matches!(recorded, DaemonError::Git(_)));
        assert!(matches!(
            awaited.wait().await.unwrap_err(),
            DaemonError::NotFound(value) if value == "worktree"
        ));
    }

    #[tokio::test]
    async fn a_dropped_delivery_resolves_as_cancelled() {
        let (delivery, awaited) = JobDelivery::<u8>::caller_gets_copy(copy_error);
        drop(delivery);
        assert!(matches!(
            awaited.wait().await.unwrap_err(),
            DaemonError::Cancelled
        ));
    }

    #[tokio::test]
    async fn only_the_first_delivery_reaches_the_caller() {
        let (delivery, awaited) = JobDelivery::caller_gets_copy(copy_error);
        assert!(delivery.finish(Ok(7_u8)).is_ok());
        assert!(delivery.finish(Ok(9_u8)).is_ok());
        assert_eq!(awaited.wait().await.unwrap(), 7);
    }
}
