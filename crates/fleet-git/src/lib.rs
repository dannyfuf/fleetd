//! Typed, byte-preserving Git backend for Fleet's native lazygit UI.
//!
//! The crate shells out directly to `git` with explicit argv, serializes all
//! mutations per [`Repository`], and exposes owned snapshots and parsed diffs.
#![warn(missing_docs)]

mod command;
mod command_log;
mod error;
mod model;
mod mutation;
pub mod parse;
mod patch;
mod read;
mod rebase;
mod repository;
pub mod watch;

/// Entry points used when the application executable is reinvoked by Git as a
/// sequence editor.
pub mod sequence_editor {
    pub use crate::rebase::{SEQUENCE_INSTRUCTION_ENV, maybe_run_from_env, run_sequence_editor};
}

pub use command::{GitCommand, GitOutput, Runner};
pub use command_log::{CommandEvent, CommandOutcome, CommandRecord};
pub use error::{GitError, Result};
pub use model::*;
pub use rebase::{FixupFlag, MoveDirection, RebaseAction, RebaseBase, RebasePlan, TodoEdit};
pub use repository::{DEFAULT_DIFF_CONTEXT, MAX_DIFF_CONTEXT, Repository};
