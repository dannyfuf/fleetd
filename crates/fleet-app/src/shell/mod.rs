//! The application shell: the window, the frame, the routing and the focus.
//!
//! [`Shell`] is the only `Render` the window owns. It composes the kit's `AppFrame`, routes
//! between the Hub and the Workspace, draws the persistent chrome (§2.2) and the daemon
//! surfaces (§3.12), owns the quit flow (§3.8.8, §3.8.9), and builds the nested key contexts
//! [`crate::keymap`] is written against.
//!
//! Everything domain-shaped is delegated: [`crate::screens::hub::HubScreen`],
//! [`crate::screens::workspace::WorkspaceScreen`], [`crate::screens::jobs::JobsPanel`] and
//! [`crate::dialogs::Dialogs`] are the extension points, documented in `docs/APP-CONTRACTS.md`.

mod chrome;
mod daemon;
mod quit;
mod root;

pub use chrome::{bare_version, domain_target, job_kind_label, job_target};
pub use daemon::{BannerSpec, RESTART_SENTENCE, banner_spec, countdown_label};
pub use quit::{QuitDecision, StopDecision, quit_decision, stop_decision};
pub use root::{Shell, run};
