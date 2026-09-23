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
mod button;
mod callout;
mod card_tile;
mod checkbox;
mod chip;
mod column_ladder;
mod confirm_dialog;
mod control;
mod copy_field;
mod cycler;
mod daemon_dot;
mod daemon_splash;
mod degraded_chip;
mod dialog;
mod dismiss;
mod divider;
mod doctor_table;
mod empty_state;
mod exit_strip;
mod fact_list;
mod fact_row;
mod filter_bar;
mod filter_field;
mod freshness_stamp;
mod fuzzy_list;
mod info_card;
mod input;
mod job_row;
mod job_ticker;
mod kanban_column;
mod kbd;
mod keep_alive_chips;
mod key_hint;
mod key_value_list;
mod list_header;
mod list_view;
mod log_view;
mod markdown;
mod markdown_text;
mod menu;
mod mode_word;
mod multiline_input;
mod navigation;
mod number_field;
mod overlay;
mod page_header;
mod palette;
mod pane;
mod pane_header;
#[cfg(test)]
mod pointer_tests;
mod pr_badge;
mod prefix_menu;
mod priority_glyph;
mod row;
mod scroll_pill;
mod section_header;
mod segmented_control;
mod segmented_tabs;
mod select;
mod sheet;
mod skeleton_rows;
mod spinner;
mod split_layout;
mod status_bar;
mod status_dot;
mod status_glyph;
mod step_card;
mod sticky_error_slot;
mod switch;
mod terminal_grid;
mod terminal_modes;
mod terminal_tab_strip;
mod title_bar;
mod toast_stack;
mod toggle;
mod tooltip;
mod value_field;
mod veil;

pub use crate::focus::{FocusRing, FocusRingKind};
pub use copy_field::CopyField;

pub use age_label::{AgeLabel, format_age};
pub use agent::*;
pub use app_frame::AppFrame;
pub use badge::{Badge, BadgeStyle};
pub use banner::Banner;
pub use button::{
    Button, ButtonSize, ButtonStyle, IconButton, StatusButton, StatusMark, SwitcherButton,
};
pub use callout::Callout;
pub use card_tile::{
    ASSIGNEE_INITIALS, BlockedTone, CARD_TITLE_LINES, CardTile, RunMark, initials, label_tone,
};
pub use checkbox::Checkbox;
pub use chip::Chip;
pub use column_ladder::{ColumnLadder, ColumnSpec, ColumnWidth, ResolvedColumn};
pub use confirm_dialog::ConfirmDialog;
pub use cycler::{Cycler, CyclerForm, SEGMENTED_MAX, SEGMENTED_MAX_CHARS};
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
pub use filter_field::FilterField;
pub use freshness_stamp::{AGING_SECS, FRESH_SECS};
pub use freshness_stamp::{Freshness, FreshnessStamp};
pub use fuzzy_list::{FuzzyItem, FuzzyList};
pub use info_card::InfoCard;
pub use input::actions as text_input;
pub use input::{
    HISTORY_CAP, InputBuffer, InputMode, TEXT_INPUT_KEY_CONTEXT, TYPING_GROUP_WINDOW, TextInput,
    TextInputEvent,
};
pub use job_row::{JobRow, JobStatus};
pub use job_ticker::JobTicker;
pub use kanban_column::{COLUMN_WIDTH_CH, KanbanBoard, KanbanColumn};
pub use kbd::{Kbd, KbdSize, KbdTone, pretty_keys};
pub use keep_alive_chips::MAX_VISIBLE;
pub use keep_alive_chips::{KeepAliveChips, KeepAliveLabel};
pub use key_hint::{KeyHint, KeyHintRow};
pub use key_value_list::KeyValueList;
pub use list_header::ListHeader;
pub use list_view::{
    DEFAULT_PAGE, ListDown, ListFirst, ListLast, ListMotion, ListPageDown, ListPageUp, ListUp,
    SCROLLOFF, SKELETON_ROWS, list_key_bindings,
};
pub use list_view::{ListCursor, ListPointer, ListView, RowPress};
pub use log_view::{LOG_TAIL_LINES, LogCommand, LogView};
pub use markdown::{
    CodeHighlights, HighlightCache, MarkdownBlock, MarkdownDocument, MarkdownInline, markdown,
    parse_markdown as parse_markdown_document, parse_markdown_cached as parse_markdown_prefix,
};
pub use markdown_text::{
    LIST_MARKER_CH, MAX_HEADING_LEVEL, MarkdownText, MdBlock, MdSpan, parse_markdown,
};
pub use menu::{
    ContextMenu, Dropdown, MENU_ITEM_TARGET, MENU_KEY_CONTEXT, Menu, MenuAnchor, MenuItem,
    PopoverMenu, menu_actions, menu_holds_focus, menu_key_bindings,
};
pub use mode_word::ModeWord;
pub use multiline_input::{
    HISTORY_LIMIT, MULTILINE_INPUT_KEY_CONTEXT, MultilineInput, MultilineInputEvent, PromptHistory,
    Trigger,
};
pub use number_field::NumberField;
pub use overlay::{Overlay, OverlayLayer};
pub use page_header::PageHeader;
pub use palette::{Palette, PaletteRow, PaletteSection};
pub use pane::{Pane, PaneBorder};
pub use pane_header::PaneHeader;
pub use pr_badge::{PrBadge, PrBadgeState};
pub use prefix_menu::{PrefixMenu, PrefixMenuColumn, PrefixMenuItem};
pub use priority_glyph::{PRIORITY_BARS, PriorityGlyph, PriorityLevel};
pub use row::GLYPH_COLUMN_CH;
pub use row::{ColumnAlign, Row, RowColumn};
pub use scroll_pill::{ScrollPill, ScrollbackBadge};
pub use section_header::SectionHeader;
pub use segmented_control::{Segment, SegmentedControl};
pub use segmented_tabs::{SegmentedTab, SegmentedTabs};
pub use select::Select;
pub use sheet::{Sheet, SheetSide};
pub use skeleton_rows::SkeletonRows;
pub use spinner::{Spinner, SpinnerWithLabel};
pub use split_layout::{SplitAxis, SplitLayout};
pub use status_bar::StatusBar;
pub use status_dot::StatusDot;
pub use status_glyph::{StatusGlyph, StatusKind};
pub use step_card::{StepCard, StepMark};
pub use sticky_error_slot::StickyErrorSlot;
pub use switch::Switch;
pub use terminal_grid::{
    CellMetrics, CellWidth, CursorShape, GridCell, GridCursor, GridRow, GridSelection,
    TerminalGrid, TerminalGridCache, UnderlineStyle,
};
pub use terminal_modes::{TerminalMode, TerminalModes};
pub use terminal_tab_strip::{TerminalAgentState, TerminalTab, TerminalTabKind, TerminalTabStrip};
pub use title_bar::{CommandField, TitleBar};
pub use toast_stack::{COALESCE_WINDOW_MS, Toast, ToastDuration, ToastStack};
pub use toggle::Toggle;
pub use tooltip::{Tooltip, WithTooltip};
pub use value_field::ValueField;
pub use veil::Veil;
