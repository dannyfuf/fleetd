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

pub use command::Runner;
pub use command_log::{CommandEvent, CommandOutcome, CommandRecord};
pub use error::{GitError, Result};
pub use model::*;
pub use rebase::{MoveDirection, sequence_editor};
pub use repository::{DEFAULT_DIFF_CONTEXT, MAX_DIFF_CONTEXT, Repository};
