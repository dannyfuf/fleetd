//! Fleet's command-line surface, including clap parsing, daemon-backed commands, protocol-compatible JSON envelopes, and human-readable output.

mod args;
mod commands;
mod envelope;
mod exec;
mod human;

pub use commands::run;
