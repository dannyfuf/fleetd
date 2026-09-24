//! The one integration-test binary for `fleet-proto`: the wire protocol's byte-exact goldens.
//!
//! Every file beside this one is a module of it rather than its own test target
//! (`autotests = false` in `Cargo.toml`). Each target would link the whole dependency
//! graph into a separate executable; one binary links it once. A new test file must be
//! declared here — the `workspace_layering` suite fails on one that is not.

mod support;

mod agent_compatibility;
mod compatibility;
mod reset_state;
