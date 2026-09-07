//! Errors shared by daemon adapters, stores, jobs, services, and transport actors.

use std::{io, path::PathBuf};

use fleet_proto::error::{ErrorKind, ProtoError};
use thiserror::Error;

/// The result type used throughout the daemon.
pub type DaemonResult<T> = Result<T, DaemonError>;

/// A typed daemon failure that maps to Fleet's stable protocol error categories.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// A requested entity or file does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// Existing state conflicts with a requested operation.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Input or persisted data failed validation.
    #[error("validation failed: {0}")]
    Validation(String),
    /// A filesystem operation failed.
    #[error("filesystem operation failed for {path}: {source}")]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// JSON encoding or decoding failed.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// An external command failed.
    #[error("command failed: {0}")]
    Shell(String),
    /// A Git operation failed.
    #[error("git operation failed: {0}")]
    Git(String),
    /// A GitHub CLI operation failed.
    #[error("GitHub operation failed: {0}")]
    Github(String),
    /// Process inspection failed.
    #[error("process inspection failed: {0}")]
    Process(String),
    /// A bounded operation exceeded its deadline.
    #[error("timed out: {0}")]
    Timeout(String),
    /// Explicit cancellation stopped an operation.
    #[error("operation cancelled")]
    Cancelled,
    /// Protocol framing or transport failed.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// A service contract exists but its domain algorithm is not implemented yet.
    #[error("unimplemented service operation: {0}")]
    Unimplemented(&'static str),
    /// A requested protocol or operation is not supported by this daemon.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A Tokio blocking or worker task failed to join.
    #[error("background task failed: {0}")]
    Join(String),
}

impl DaemonError {
    /// Constructs a filesystem error while retaining its path context.
    #[must_use]
    pub fn fs(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Filesystem {
            path: path.into(),
            source,
        }
    }
}

impl From<DaemonError> for ProtoError {
    fn from(error: DaemonError) -> Self {
        let kind = match &error {
            DaemonError::NotFound(_) => ErrorKind::NotFound,
            DaemonError::Conflict(_) => ErrorKind::Conflict,
            DaemonError::Validation(_) | DaemonError::Json(_) => ErrorKind::Validation,
            DaemonError::Filesystem { .. } => ErrorKind::Fs,
            DaemonError::Git(_) => ErrorKind::Git,
            DaemonError::Github(_) => ErrorKind::Github,
            DaemonError::Cancelled => ErrorKind::Cancelled,
            DaemonError::Unimplemented(_) | DaemonError::Unsupported(_) => ErrorKind::Unsupported,
            DaemonError::Shell(_)
            | DaemonError::Process(_)
            | DaemonError::Timeout(_)
            | DaemonError::Protocol(_)
            | DaemonError::Join(_) => ErrorKind::Unknown,
        };
        let message = match error {
            DaemonError::Unsupported(message) => message,
            other => other.to_string(),
        };
        Self {
            kind,
            message: message.replace(['\n', '\r'], " "),
        }
    }
}

impl From<fleet_core::board::BoardError> for DaemonError {
    fn from(error: fleet_core::board::BoardError) -> Self {
        use fleet_core::board::BoardError;
        match error {
            // The NotFound renderer already prefixes "not found:"; siblings read "repo x".
            BoardError::BoardNotFound(id) => Self::NotFound(format!("board {id}")),
            BoardError::CardNotFound(id) => Self::NotFound(format!("card {id}")),
            BoardError::Duplicate(_) | BoardError::Conflicted(_) => {
                Self::Conflict(error.to_string())
            }
            BoardError::Unsupported(_) => Self::Unsupported(error.to_string()),
            // The backend's own sentence, once: `Shell` prints its own prefix, and a second
            // "backend error:" inside it is noise the CLI shows verbatim.
            BoardError::Backend(message) => Self::Shell(message.clone()),
            // The CLI and the app print this one verbatim, so it must not be rewrapped:
            // `Validation` renders the message as-is, exactly like `Invalid`.
            BoardError::ReadOnlyField(_)
            | BoardError::UnknownStatus(_)
            | BoardError::UnknownLabel(_)
            | BoardError::Invalid { .. }
            | BoardError::UnknownBackend(_) => Self::Validation(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_protocol_errors_preserve_the_requested_message() {
        let error = ProtoError::from(DaemonError::Unsupported(
            "remote hosts are not supported yet".to_owned(),
        ));

        assert_eq!(error.kind, ErrorKind::Unsupported);
        assert_eq!(error.message, "remote hosts are not supported yet");
    }
}
