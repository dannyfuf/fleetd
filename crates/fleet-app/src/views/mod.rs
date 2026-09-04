//! Domain-oriented GPUI views composed from the Fleet UI kit.
//!
//! A *view* is a piece of the app that more than one screen draws: a worktree row, the detail
//! panel, a PR row, a keep-alive chip strip. Screens live in [`crate::screens`]; the pieces
//! they share belong here, and both compose only `fleet-ui-kit` components — never ad-hoc
//! styling, never a raw color (`docs/DESIGN-SYSTEM.md`).
//!
//! Add one module per view and re-export it here.

pub mod detail_panel;
pub mod hub_context_bar;
pub mod prs_screen;
pub mod repos_rail;
pub mod worktrees_list;
pub mod workspace_header;
pub mod workspace_tabs;
