//! The one integration-test binary for `fleet-daemon`: the daemon's services, server and remote machines.
//!
//! Every file beside this one is a module of it rather than its own test target
//! (`autotests = false` in `Cargo.toml`). Each target would link the whole dependency
//! graph into a separate executable; one binary links it once. A new test file must be
//! declared here — the `workspace_layering` suite fails on one that is not.

mod infra;

mod agents_real_binaries;
mod agents_remote;
mod boards_automation;
mod boards_jira;
mod boards_service;
mod bootstrap_host;
mod contexts_service;
mod doctor_checks;
mod github_service;
mod hosts_status;
mod import_from_swarm;
mod inspect_behavior;
mod pool_fake_claim;
mod pool_lifecycle;
mod prune_safety;
mod pty_holder;
mod remote_boards;
mod remote_create;
mod remote_infra;
mod remote_lifecycle;
mod remote_link;
mod remote_mirror;
mod remote_sessions;
mod repos_service;
mod reset_state;
mod router_core;
mod server_events;
mod server_process;
mod sessions_lifecycle;
mod sleep_policy;
mod watches_service;
mod worktrees_lifecycle;
