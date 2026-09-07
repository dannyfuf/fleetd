//! Durable configuration and state stores plus cross-process locking.

pub mod config;
pub mod lock;
pub mod state;

/// Per-board JSON document persistence.
pub mod board;
