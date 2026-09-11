//! Domain-agnostic presentation contracts for the native-agent thread.
//!
//! `docs/NATIVE-AGENTS.md` §11 is the catalog and `docs/DESIGN-SYSTEM.md` §6.6 the entry. The
//! module obeys the kit's two absolute rules: **no domain type crosses the boundary** — every
//! component takes `SharedString`s, scalars, small value types declared here and closures — and
//! **no literal colour, size, radius, font size or duration reaches a component**. The fixed
//! geometry of the canvas lives in [`metrics`], the copy in [`format`] and [`group`], and
//! everything else comes from the theme.

mod decision;
mod decision_dock;
mod format;
mod group;
mod metadata_row;
mod metrics;
mod rows;
mod scroll;
mod tool_row;
mod transcript_list;

#[cfg(test)]
mod tests;

pub use decision::{
    ApprovalRequest, Decision, DecisionAction, DecisionKind, DecisionOption, DecisionQuestion,
    MAX_QUESTION_OPTIONS, QuestionOption, QuestionSet, SOMETHING_ELSE,
};
pub use decision_dock::DecisionDock;
pub use format::{
    MINUS, format_compacted, format_cost, format_counter, format_duration, format_exit,
    format_file_delta, format_files_changed, format_resumed, format_retrying, format_stopped_after,
    format_thought, format_token_count, format_worked, format_working, turn_footer_segments,
};
pub use group::{ToolGroupCounts, format_group_summary};
pub use metadata_row::{
    MetadataFit, MetadataFitResult, MetadataRow, MetadataSegment, fit as metadata_fit,
};
pub use metrics::{
    AGENT_BODY_MAX_H, AGENT_CARET_H, AGENT_CONTENT_W, AGENT_FOLLOW_REARM_PX, AGENT_LIST_OVERDRAW,
    AGENT_PLAN_PREVIEW_H, AGENT_PREVIEW_MAX_H, AGENT_SCROLLBAR_INSET, AGENT_TOOL_KIND_W,
    AGENT_USER_MAX_W, AGENT_WELL_MAX_H,
};
pub use rows::{
    AssistantMetaRow, AssistantRow, CheckpointRow, DiffRow, EmptyRow, ErrorRow, GateOutcome,
    GateRow, NoticeRow, PlanRow, ReasoningRow, RowSplice, SubagentRow, TranscriptRhythm,
    TranscriptRow, TranscriptRowId, TranscriptRowKind, TurnFoldRow, TurnFooterRow, UserRow,
    UserRowState, WorkGroupRow, WorkLiveRow, WorkingPhase, WorkingRow, diff_rows,
};
pub use scroll::{FollowState, Gesture, ScrollMode, breaks_follow, is_at_end};
pub use tool_row::{ToolGlyph, ToolRow, ToolRowElement, ToolRowState, expand_hint};
pub use transcript_list::{
    RowAction, RowBodyRenderer, TranscriptEvent, TranscriptList, scroll_thumb,
};
