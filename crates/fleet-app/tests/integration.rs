//! The one integration-test binary for `fleet-app`: the app against a real daemon.
//!
//! Every file beside this one is a module of it rather than its own test target
//! (`autotests = false` in `Cargo.toml`). Each target would link the whole dependency
//! graph into a separate executable; one binary links it once. A new test file must be
//! declared here — the `workspace_layering` suite fails on one that is not.

mod common;

mod board_flow;
mod harness_headless;
mod jobs_panel;
