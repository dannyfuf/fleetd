//! The harness layer: the adapter seam and the two harnesses behind it.
//!
//! Harness wire types never leave this module tree — `fleet-proto` never learns the word
//! `turn/start` — and nothing in here holds a GPUI handle or reads configuration at call time.
//! `NATIVE-AGENTS.md` §3 and `research/spec-A-harness.md` are the authority.

pub mod claude;
pub mod codex;
#[cfg(test)]
pub(crate) mod golden;
pub mod harness;
