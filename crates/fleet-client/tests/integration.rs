//! The one integration-test binary for `fleet-client`: the client against scripted and real peers.
//!
//! Every file beside this one is a module of it rather than its own test target
//! (`autotests = false` in `Cargo.toml`). Each target would link the whole dependency
//! graph into a separate executable; one binary links it once. A new test file must be
//! declared here — the `workspace_layering` suite fails on one that is not.

mod agents_window;
mod bugfix_connection;
mod bugfix_terminal_spawn;
mod client;
mod generic_stream;
