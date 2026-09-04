//! One module per component. Each exposes a public struct with builder methods and a
//! `RenderOnce` / `IntoElement` implementation, so views compose them and never style by hand.
//!
//! Components are **domain-agnostic**: they take `SharedString`, closures and plain scalars,
//! never a `fleet-core` type, and they read every color, size and duration from
//! [`crate::theme::Theme`].

pub mod age_label;
pub mod app_frame;
pub mod badge;
pub mod banner;
pub mod chip;
pub mod column_ladder;
pub mod confirm_dialog;
pub mod context_bar;
pub mod cycler;
pub mod daemon_dot;
pub mod daemon_splash;
pub mod degraded_chip;
pub mod dialog;
pub mod divider;
pub mod doctor_table;
pub mod empty_state;
pub mod exit_strip;
pub mod fact_list;
pub mod fact_row;
pub mod filter_bar;
pub mod freshness_stamp;
pub mod fuzzy_list;
pub mod job_row;
pub mod job_ticker;
pub mod keep_alive_chips;
pub mod key_hint;
pub mod key_value_list;
pub mod list_view;
pub mod log_view;
pub mod mode_word;
pub mod number_field;
pub mod overlay;
pub mod palette;
pub mod pane;
pub mod pane_header;
pub mod pr_badge;
pub mod prefix_hint;
pub mod row;
pub mod scroll_pill;
pub mod section_header;
pub mod segmented_tabs;
pub mod select;
pub mod sheet;
pub mod skeleton_rows;
pub mod spinner;
pub mod split_layout;
pub mod status_bar;
pub mod status_dot;
pub mod status_glyph;
pub mod sticky_error_slot;
pub mod terminal_grid;
pub mod terminal_modes;
pub mod terminal_tab_strip;
pub mod text_field;
pub mod toast_stack;
pub mod toggle;
pub mod veil;

pub use crate::focus::{FocusRing, FocusRingKind};

pub use age_label::{AgeLabel, format_age};
pub use app_frame::AppFrame;
pub use badge::{Badge, BadgeStyle};
pub use banner::Banner;
pub use chip::Chip;
pub use column_ladder::{ColumnLadder, ColumnSpec, ColumnWidth, ResolvedColumn};
pub use confirm_dialog::ConfirmDialog;
pub use context_bar::{ContextBar, ContextTab, TRAFFIC_LIGHT_INSET};
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
pub use freshness_stamp::{Freshness, FreshnessStamp};
pub use fuzzy_list::{FuzzyItem, FuzzyList};
pub use job_row::{JobRow, JobStatus};
pub use job_ticker::JobTicker;
pub use keep_alive_chips::{KeepAliveChips, KeepAliveLabel};
pub use key_hint::{KeyHint, KeyHintRow};
pub use key_value_list::KeyValueList;
pub use list_view::{ListCursor, ListView};
pub use log_view::{LOG_TAIL_LINES, LogView};
pub use mode_word::{Mode, ModeWord};
pub use number_field::NumberField;
pub use overlay::{Overlay, OverlayLayer};
pub use palette::{Palette, PaletteRow, PaletteSection, PaletteSectionKind};
pub use pane::{Pane, PaneBorder};
pub use pane_header::PaneHeader;
pub use pr_badge::{PrBadge, PrBadgeState};
pub use prefix_hint::PrefixHint;
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
    CellWidth, CursorShape, GridCell, GridCursor, GridRow, GridSelection, TerminalGrid,
    UnderlineStyle,
};
pub use terminal_modes::{TerminalMode, TerminalModes};
pub use terminal_tab_strip::{TerminalTab, TerminalTabStrip};
pub use text_field::TextField;
pub use toast_stack::{COALESCE_WINDOW_MS, Toast, ToastDuration, ToastStack};
pub use toggle::Toggle;
pub use veil::{VEIL_OPACITY, Veil};

/// `Modal` is the same surface as [`Dialog`]: scrim + card + 44 px header + 44 px footer.
/// The alias exists so a view that thinks in "modal" finds the right type.
pub type Modal = Dialog;

/// `TabBar` is the same surface as [`SegmentedTabs`]. The Workspace's terminal strip is a
/// different component ([`TerminalTabStrip`]) because its tabs carry indices, activity dots
/// and exit codes.
pub type TabBar = SegmentedTabs;
