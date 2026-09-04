//! Fleet's command-line surface, including clap parsing, daemon-backed commands, protocol-compatible JSON envelopes, and human-readable output.

pub mod args;
pub mod commands;
pub mod envelope;
pub mod human;

pub use commands::run;
