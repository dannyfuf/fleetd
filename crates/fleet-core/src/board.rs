//! Backend-independent kanban board contracts.

pub mod automation;
pub mod defaults;
pub mod model;
pub mod ops;
pub mod property;
pub mod sync;
#[cfg(test)]
mod tests;

pub use automation::{
    LiveIndex, Plan, ResolvedPrefs, StartRun, brief, next_pending, re_evaluate,
    re_evaluate_settled, resolve_prefs,
};
pub use defaults::{
    BOARD_ID_MAX_LEN, PRESET_EXPECT_IMPLEMENT, PRESET_EXPECT_REVIEW, PRESET_INSTRUCTIONS_IMPLEMENT,
    PRESET_REVIEW_SKILL, apply_workflow_preset, default_statuses, new_board, new_worktree_board,
    render_template, workflow_preset, worktree_board_id,
};
pub use model::{
    Action, ActionKind, Activity, ActivityKind, BOARD_DOCUMENT_MIN_VERSION, BOARD_DOCUMENT_VERSION,
    BackendRef, Board, BoardDocument, BoardSettings, BoardSummary, BoardView, Card, CardAgentPrefs,
    CardRun, ColumnAgentPrefs, ColumnAutomation, Comment, Conflict, ConflictPolicy, Label, LiveRun,
    MAX_LIVE_RUNS_PER_BOARD, MAX_REPORT_COMMENTS_PER_CARD, MAX_RUNS_PER_CARD,
    PENDING_AMBER_AFTER_SECS, PendingRun, Priority, REPORT_EXCERPT_CAP_BYTES, RemoteLink,
    RunOutcome, Status, StatusCategory, SyncState, document_version, field_label,
};
pub use ops::{
    Blocked, BlockedTone, BoardError, BoardPatch, CardDraft, CardPatch, add_comment,
    apply_board_patch, apply_card_patch, attention, blocked, blocks, check_draft_writable,
    column_cards, create_card, first_status_in, is_satisfied, latest_run, merge_settings,
    move_card, normalise_automation, push_activity, summarize, valid_date, validate_automation,
    validate_board, validate_card, validate_env, validate_links, worktree_slug,
};
pub use property::{
    PropertyKind, PropertyOption, PropertySchema, PropertySource, PropertyValue, REQUIRED_MARKER,
    is_required, schema_row_name,
};
pub use sync::{
    ARCHIVED_BY_SYNC, BackendCapabilities, BackendDescriptor, BackendSchema, ConflictResolution,
    PullResult, PushAck, PushFailure, PushOp, PushResult, RemoteCard, RemoteComment, RemoteStatus,
    adopt_schema, apply_push_result, apply_remote, reconcile, remote_category, remote_status_key,
    resolve_conflict,
};
