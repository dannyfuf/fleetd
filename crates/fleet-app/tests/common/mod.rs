//! The isolated-daemon fixture these tests share with the end-to-end harness.
//!
//! There is one implementation of "start an isolated fleetd" in the workspace and it lives in
//! `fleet_harness::env` (`docs/TESTING-HARNESS.md` §4): a private `FLEET_HOME`, a child-only
//! `HOME`, a `PATH` whose first entry holds the fake `gh`, `FLEET_DAEMON` pointed at the
//! freshly built binary, and RAII cleanup of both the process and the temporary home.

pub use fleet_harness::env::IsolatedDaemon as Daemon;
