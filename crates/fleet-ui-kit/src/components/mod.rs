//! One module per component. Each exposes a public struct with builder methods and a
//! `RenderOnce` / `IntoElement` implementation, so views compose them and never style by hand.
//!
//! Components are **domain-agnostic**: they take `SharedString`, closures and plain scalars,
//! never a `fleet-core` type, and they read every color, size and duration from
//! [`crate::theme::Theme`].

mod age_label;
pub mod agent;
mod app_frame;
mod badge;
mod banner;
mod card_tile;
mod chip;
mod column_ladder;
mod confirm_dialog;
mod context_bar;
mod control;
mod cycler;
mod daemon_dot;
mod daemon_splash;
mod degraded_chip;
mod dialog;
mod divider;
mod doctor_table;
mod empty_state;
mod exit_strip;
mod fact_list;
mod fact_row;
mod filter_bar;
mod freshness_stamp;
mod fuzzy_list;
mod job_row;
mod job_ticker;
mod kanban_column;
mod keep_alive_chips;
mod key_hint;
mod key_value_list;
mod list_view;
mod log_view;
mod markdown;
mod markdown_text;
mod mode_word;
mod multiline_input;
mod navigation;
mod number_field;
mod overlay;
mod palette;
mod pane;
mod pane_header;
mod pr_badge;
mod prefix_hint;
mod priority_glyph;
mod row;
mod scroll_pill;
mod section_header;
mod segmented_tabs;
mod select;
mod sheet;
mod skeleton_rows;
mod spinner;
mod split_layout;
mod status_bar;
mod status_dot;
mod status_glyph;
mod sticky_error_slot;
mod terminal_grid;
mod terminal_modes;
mod terminal_tab_strip;
mod text_area;
mod text_field;
mod toast_stack;
mod toggle;
mod veil;

pub use crate::focus::{FocusRing, FocusRingKind};

pub use age_label::{AgeLabel, format_age};
pub use agent::*;
pub use app_frame::AppFrame;
pub use badge::{Badge, BadgeStyle};
pub use banner::Banner;
pub use card_tile::{ASSIGNEE_INITIALS, CARD_TITLE_LINES, CardTile, initials, label_tone};
pub use chip::Chip;
pub use column_ladder::{ColumnLadder, ColumnSpec, ColumnWidth, ResolvedColumn};
pub use confirm_dialog::ConfirmDialog;
pub use context_bar::{ContextBar, ContextTab};
pub use cycler::Cycler;
pub use daemon_dot::{DaemonDot, DaemonState};
pub use daemon_splash::{DaemonSplash, DaemonSplashKind};
pub use degraded_chip::DegradedChip;
pub use dialog::Dialog;
pub use divider::{Divider, DividerAxis};
pub use doctor_table::{DoctorRow, DoctorStatus, DoctorTable};
pub use empty_state::EmptyState;
pub use exit_strip::ExitStrip;
pub use fact_list::{ConfirmKey, Fact, FactList};
pub use fact_row::{FactRow, FactValue};
pub use filter_bar::FilterBar;
pub use freshness_stamp::{AGING_SECS, FRESH_SECS};
pub use freshness_stamp::{Freshness, FreshnessStamp};
pub use fuzzy_list::{FuzzyItem, FuzzyList};
pub use job_row::{JobRow, JobStatus};
pub use job_ticker::JobTicker;
pub use kanban_column::{COLUMN_WIDTH_CH, KanbanBoard, KanbanColumn};
pub use keep_alive_chips::MAX_VISIBLE;
pub use keep_alive_chips::{KeepAliveChips, KeepAliveLabel};
pub use key_hint::{KeyHint, KeyHintRow};
pub use key_value_list::KeyValueList;
pub use list_view::{
    DEFAULT_PAGE, ListDown, ListFirst, ListLast, ListMotion, ListPageDown, ListPageUp, ListUp,
    SCROLLOFF, SKELETON_ROWS, list_key_bindings,
};
pub use list_view::{ListCursor, ListView};
pub use log_view::{LOG_TAIL_LINES, LogCommand, LogView};
pub use markdown::{
    MarkdownBlock, MarkdownDocument, MarkdownInline, markdown,
    parse_markdown as parse_markdown_document,
};
pub use markdown_text::{
    LIST_MARKER_CH, MAX_HEADING_LEVEL, MarkdownText, MdBlock, MdSpan, parse_markdown,
};
pub use mode_word::{Mode, ModeWord};
pub use multiline_input::{
    HISTORY_LIMIT, MULTILINE_INPUT_KEY_CONTEXT, MultilineBuffer, MultilineInput,
    MultilineInputEvent, PromptHistory,
};
pub use number_field::NumberField;
pub use overlay::{Overlay, OverlayLayer};
pub use palette::{Palette, PaletteRow, PaletteSection, PaletteSectionKind};
pub use pane::{Pane, PaneBorder};
pub use pane_header::PaneHeader;
pub use pr_badge::{PrBadge, PrBadgeState};
pub use prefix_hint::PrefixHint;
pub use priority_glyph::{PRIORITY_BARS, PriorityGlyph, PriorityLevel};
pub use row::GLYPH_COLUMN_CH;
pub use row::{ColumnAlign, Row, RowColumn};
pub use scroll_pill::{ScrollPill, ScrollbackBadge};
pub use section_header::SectionHeader;
pub use segmented_tabs::{SegmentedTab, SegmentedTabs};
pub use select::Select;
pub use sheet::Sheet;
pub use skeleton_rows::SkeletonRows;
pub use spinner::{Spinner, SpinnerWithLabel};
pub use split_layout::{SplitAxis, SplitLayout};
pub use status_bar::StatusBar;
pub use status_dot::StatusDot;
pub use status_glyph::{StatusGlyph, StatusKind};
pub use sticky_error_slot::StickyErrorSlot;
pub use terminal_grid::{
    CellMetrics, CellWidth, CursorShape, GridCell, GridCursor, GridRow, GridSelection,
    TerminalGrid, TerminalGridCache, UnderlineStyle,
};
pub use terminal_modes::{TerminalMode, TerminalModes};
pub use terminal_tab_strip::{TerminalAgentState, TerminalTab, TerminalTabKind, TerminalTabStrip};
pub use text_area::{TAB_WIDTH, TEXT_AREA_ROWS, TextArea, TextAreaState};
pub use text_field::{
    EditEffect, TEXT_FIELD_KEY_CONTEXT, TextField, TextFieldState, TextInput, TextInputEvent,
};
pub use toast_stack::{COALESCE_WINDOW_MS, Toast, ToastDuration, ToastStack};
pub use toggle::Toggle;
pub use veil::Veil;
