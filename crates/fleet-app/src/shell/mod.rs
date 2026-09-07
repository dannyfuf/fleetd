//! Window coordination, persistent chrome, and daemon surfaces.

mod chrome;
mod daemon;
mod quit;
mod root;

pub use root::{Shell, run};
