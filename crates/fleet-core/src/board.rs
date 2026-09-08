//! Backend-independent kanban board contracts.

pub mod defaults;
pub mod model;
pub mod ops;
pub mod property;
pub mod sync;
#[cfg(test)]
mod tests;

pub use defaults::{default_statuses, new_board};
pub use model::{
    Activity, ActivityKind, BOARD_DOCUMENT_VERSION, BackendRef, Board, BoardDocument,
    BoardSettings, BoardSummary, BoardView, Card, Comment, Conflict, ConflictPolicy, Label,
    Priority, RemoteLink, Status, StatusCategory, SyncState, field_label,
};
pub use ops::{
    BoardError, BoardPatch, CardDraft, CardPatch, add_comment, apply_board_patch, apply_card_patch,
    check_draft_writable, column_cards, create_card, first_status_in, merge_settings, move_card,
    push_activity, summarize, valid_date, validate_board, validate_card, worktree_slug,
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
