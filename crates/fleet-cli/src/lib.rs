//! Fleet's command-line surface, including clap parsing, daemon-backed commands, protocol-compatible JSON envelopes, and human-readable output.

pub mod args;
pub mod commands;
pub mod envelope;
mod exec;
pub mod human;

pub use args::{VERSION, VERSION_DISPLAY};
pub use commands::run;
