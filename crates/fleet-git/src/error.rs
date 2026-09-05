//! Errors returned by the Git backend.

use std::{io, path::PathBuf, time::Duration};

/// Result type used by this crate.
pub type Result<T> = std::result::Result<T, GitError>;

/// A typed failure at the process, parsing, repository, or Git-operation boundary.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Git could not be started or communicated with.
    #[error("could not run {argv:?}: {source}")]
    Spawn {
        /// Redacted command line.
        argv: Vec<String>,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// Git exited unsuccessfully.
    #[error("git command {argv:?} exited with status {status:?}: {message}")]
    Exit {
        /// Process exit code, if the platform supplied one.
        status: Option<i32>,
        /// Complete standard output.
        stdout: Vec<u8>,
        /// Complete standard error.
        stderr: Vec<u8>,
        /// Redacted command line.
        argv: Vec<String>,
        /// Lossy, concise error text for display.
        message: String,
    },
    /// Machine-readable Git output or an instruction could not be parsed.
    #[error("could not parse {context}: {message}")]
    Parse {
        /// The output being parsed.
        context: &'static str,
        /// Human-readable detail.
        message: String,
    },
    /// The supplied path does not belong to a non-bare working tree.
    #[error("not a Git working tree: {0}")]
    NotARepository(PathBuf),
    /// A process exceeded its configured deadline.
    #[error("git command {argv:?} timed out after {timeout:?}")]
    Timeout {
        /// Redacted command line.
        argv: Vec<String>,
        /// Configured deadline.
        timeout: Duration,
    },
    /// Git stopped after producing merge conflicts.
    #[error("git command {argv:?} stopped with conflicts: {message}")]
    Conflict {
        /// Process exit code, if the platform supplied one.
        status: Option<i32>,
        /// Complete standard output.
        stdout: Vec<u8>,
        /// Complete standard error.
        stderr: Vec<u8>,
        /// Redacted command line.
        argv: Vec<String>,
        /// Lossy, concise error text for display.
        message: String,
    },
}

impl GitError {
    /// Builds a parsing error with a static context label.
    pub(crate) fn parse(context: &'static str, message: impl Into<String>) -> Self {
        Self::Parse {
            context,
            message: message.into(),
        }
    }
}
