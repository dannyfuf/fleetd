//! Read-only domain presentation shared by state, screens, dialogs, and views.
//! Controllers own revisions and mutation targets; these helpers never initiate requests.

mod damage;
mod formatting;
mod jobs;
mod keys;
mod navigation;
mod selection;
mod snapshot;
mod status;
mod workspace_keys;

pub use damage::{EventDamage, event_damage};
pub use formatting::{
    age_label, age_secs, bare_version, home_dir, now_unix, parse_timestamp, tilde,
};
pub use jobs::{
    JobDisplay, active_job_summary, is_active, is_dismissable, is_user_visible_job, job_kind_label,
    job_outcome_toast, job_sentence, job_target, latest_unseen_failure, parse_percent, sub_line,
};
pub use keys::{FuzzyMatch, FuzzyQuery, contains_folded, humanize, pretty_keys};
pub use navigation::enter_session;
pub use selection::{
    DisplayedHub, DisplayedPr, DisplayedRepo, DisplayedRepoKind, DisplayedTarget,
    DisplayedWorktree, filter_counts, filter_target, selected_repo_id, selected_worktree_id,
};
pub use snapshot::SnapshotIndex;
pub use status::{
    KeepAliveStyle, inspection_badge, keep_alive_icon, pr_badge_state, row_glyph, session_glyph,
    terminal_label,
};
pub(crate) use workspace_keys::{hub_jobs_key, hub_sticky_error_key, workspace_keys};
