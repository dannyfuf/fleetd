//! Domain-agnostic presentation contracts for native-agent transcript rows.

mod decision_card;
mod format;
mod metrics;
mod tool_row;
mod transcript_list;

pub use decision_card::{
    DecisionAction, DecisionCard, DecisionCardElement, DecisionCardKind, DecisionOption,
    DecisionQuestion, SOMETHING_ELSE, decision_card, decision_key_hints, permission_actions,
    plan_actions, question_actions,
};
pub use format::{
    MINUS, format_compacted, format_duration, format_file_delta, format_files_changed,
    format_resumed, format_retrying, format_thinking, format_token_count, format_turn_footer,
    format_worked,
};
pub use metrics::{
    AGENT_BODY_MAX_H, AGENT_CARET_H, AGENT_CONTENT_W, AGENT_LIST_OVERDRAW, AGENT_SCROLLBAR_INSET,
    AGENT_TOOL_KIND_W,
};
pub use tool_row::{ToolRow, ToolRowElement, ToolRowState, expand_hint, tool_row};
pub use transcript_list::{
    RowSplice, ToolBodyRenderer, TranscriptEvent, TranscriptList, TranscriptRow, attachment_pill,
    diff_rows, error_card, scroll_fraction, user_block, user_block_with,
};
