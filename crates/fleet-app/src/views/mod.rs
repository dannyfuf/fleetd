//! Domain-specific view composition. Shared read-only projections live in `crate::presentation`.

pub mod board_card_detail;
pub mod board_screen;
pub(crate) mod changes_panel;
pub mod detail;
pub mod doctor_view;
pub mod first_run;
pub(crate) mod harness;
pub mod hub_context_bar;
pub mod job_ticker;
pub mod jobs_panel;
pub(crate) mod prefix_menu;
pub mod prs_screen;
pub mod repos_rail;
pub mod sticky_error;
pub mod watch_pane;
pub mod workspace_tabs;
pub mod worktrees_list;
