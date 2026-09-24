//! The one integration-test binary for `fleet-git`: git plumbing against real repositories.
//!
//! Every file beside this one is a module of it rather than its own test target
//! (`autotests = false` in `Cargo.toml`). Each target would link the whole dependency
//! graph into a separate executable; one binary links it once. A new test file must be
//! declared here — the `workspace_layering` suite fails on one that is not.

mod support;

mod bugfix_mutation;
mod bugfix_parse_patch;
mod bugfix_read_watch;
mod bugfix_rebase;
mod read_efficiency;
mod repository;
