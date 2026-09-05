//! Terminal frame, cell, cursor, mode, and key-event wire types.

use bitflags::bitflags;
use fleet_core::ids::TerminalId;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Terminal cell color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    /// Resolve from the client theme.
    #[default]
    Default,
    /// Indexed terminal palette color.
    Palette(u8),
    /// True-color RGB value.
    Rgb {
        /// Red channel.
        r: u8,
        /// Green channel.
        g: u8,
        /// Blue channel.
        b: u8,
    },
}

bitflags! {
    /// Visual attributes applied to a terminal cell.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct CellAttrs: u16 {
        /// Bold intensity.
        const BOLD = 1 << 0;
        /// Dim intensity.
        const DIM = 1 << 1;
        /// Italic text.
        const ITALIC = 1 << 2;
        /// Single underline.
        const UNDERLINE = 1 << 3;
        /// Double underline.
        const DOUBLE_UNDERLINE = 1 << 4;
        /// Curly underline.
        const CURLY_UNDERLINE = 1 << 5;
        /// Strikethrough.
        const STRIKETHROUGH = 1 << 6;
        /// Swap foreground and background.
        const INVERSE = 1 << 7;
        /// Blink text.
        const BLINK = 1 << 8;
        /// Hide text.
        const INVISIBLE = 1 << 9;
    }
}

/// Width role of a terminal cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellWidth {
    /// One-column grapheme.
    #[default]
    Narrow,
    /// Two-column grapheme stored in this cell.
    Wide,
    /// Placeholder following a wide grapheme.
    Spacer,
}

/// One rendered terminal grid cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    /// Compact grapheme text.
    pub text: SmolStr,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Independent underline color.
    pub underline_color: Option<Color>,
    /// Visual flags.
    pub attrs: CellAttrs,
    /// Grid width role.
    pub width: CellWidth,
}

/// Replacement contents for one terminal row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowUpdate {
    /// Zero-based row index.
    pub index: u16,
    /// Complete cells for the row.
    pub cells: Vec<Cell>,
}

/// Terminal cursor shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Filled block cursor.
    #[default]
    Block,
    /// Vertical bar cursor.
    Bar,
    /// Horizontal underline cursor.
    Underline,
}

/// Current terminal cursor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorState {
    /// Zero-based cursor row.
    pub row: u16,
    /// Zero-based cursor column.
    pub col: u16,
    /// Whether the cursor should be painted.
    pub visible: bool,
    /// Cursor shape.
    pub shape: CursorShape,
}

/// Current scrollback viewport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportInfo {
    /// Number of retained scrollback rows.
    pub scrollback_len: usize,
    /// Rows above the live bottom currently displayed.
    pub offset: usize,
}

/// Terminal modes needed by rendering and key encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalModes {
    /// Alternate-screen mode.
    pub alt_screen: bool,
    /// Any mouse reporting mode is active.
    pub mouse_reporting: bool,
    /// Bracketed-paste mode.
    pub bracketed_paste: bool,
    /// Focus-event reporting mode.
    pub focus_events: bool,
    /// Active Kitty keyboard protocol flags.
    pub kitty_keyboard_flags: u8,
    /// Application cursor-key mode.
    pub app_cursor_keys: bool,
}

/// Full or dirty-row terminal frame update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameUpdate {
    /// Updated terminal.
    pub terminal: TerminalId,
    /// Monotonic terminal-local frame sequence.
    pub seq: u64,
    /// Grid column count.
    pub cols: u16,
    /// Grid row count.
    pub rows: u16,
    /// Whether this frame replaces the complete mirror grid.
    pub full: bool,
    /// Viewport row movement: positive shifts existing rows up, negative shifts them down.
    /// Apply before row replacements. Omitted on full frames and content/size changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift: Option<i32>,
    /// Complete replacements for changed rows.
    pub rows_changed: Vec<RowUpdate>,
    /// Cursor state.
    pub cursor: CursorState,
    /// Scrollback viewport state.
    pub viewport: ViewportInfo,
    /// Terminal modes.
    pub modes: TerminalModes,
    /// New title when the title changed.
    pub title: Option<String>,
}

/// Semantic keyboard key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    /// Enter or Return.
    Enter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Horizontal tab.
    Tab,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Insert.
    Insert,
    /// Forward Delete.
    Delete,
    /// Function key 1.
    F1,
    /// Function key 2.
    F2,
    /// Function key 3.
    F3,
    /// Function key 4.
    F4,
    /// Function key 5.
    F5,
    /// Function key 6.
    F6,
    /// Function key 7.
    F7,
    /// Function key 8.
    F8,
    /// Function key 9.
    F9,
    /// Function key 10.
    F10,
    /// Function key 11.
    F11,
    /// Function key 12.
    F12,
    /// Unicode scalar value.
    Char(char),
}

bitflags! {
    /// Keyboard modifier flags.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Modifiers: u8 {
        /// Shift modifier.
        const SHIFT = 1 << 0;
        /// Control modifier.
        const CTRL = 1 << 1;
        /// Alt/Option modifier.
        const ALT = 1 << 2;
        /// Super/Command/Windows modifier.
        const SUPER = 1 << 3;
    }
}

/// Key action phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    /// Initial press.
    Press,
    /// Auto-repeat press.
    Repeat,
    /// Key release.
    Release,
}

/// Client keyboard event sent for terminal encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEvent {
    /// Semantic key.
    pub key: Key,
    /// Active modifier flags.
    pub mods: Modifiers,
    /// Platform-composed text, when available.
    pub text: Option<String>,
    /// Press/repeat/release phase.
    pub action: KeyAction,
}

/// Mouse button identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Middle button.
    Middle,
    /// Secondary button.
    Right,
    /// Wheel up pseudo-button.
    WheelUp,
    /// Wheel down pseudo-button.
    WheelDown,
    /// Additional platform-specific button.
    Other(u8),
}

/// Mouse event phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseEventKind {
    /// Button press.
    Press,
    /// Button release.
    Release,
    /// Pointer movement without an active button.
    Move,
    /// Pointer movement with an active button.
    Drag,
}

/// Client mouse event for terminal mouse reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MouseEvent {
    /// Button associated with the event.
    pub button: MouseButton,
    /// Event phase.
    pub kind: MouseEventKind,
    /// Zero-based terminal column.
    pub col: u16,
    /// Zero-based terminal row.
    pub row: u16,
    /// Active modifier flags.
    pub mods: Modifiers,
}

/// Whole-row wheel input; routing uses the daemon's live VT modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WheelEvent {
    /// Positive moves down toward live output; negative moves up into history.
    pub steps: i32,
    /// Zero-based pointer column.
    pub col: u16,
    /// Zero-based pointer row.
    pub row: u16,
    /// Shift, Control, Alt, and Super/Command modifiers.
    pub mods: Modifiers,
}

/// Scrollback navigation command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollCommand {
    /// Move by signed row count.
    Lines(i32),
    /// Move by signed viewport count.
    Pages(i32),
    /// Jump to the oldest retained row.
    Top,
    /// Jump back to the live bottom.
    Bottom,
    /// Set an exact bottom-relative offset.
    ToOffset(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_terminal_enums_round_trip() {
        macro_rules! round_trip {
            ($value:expr, $ty:ty) => {{
                let value = $value;
                let json = serde_json::to_string(&value).unwrap_or_else(|error| panic!("{error}"));
                let decoded: $ty =
                    serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
                assert_eq!(decoded, value);
            }};
        }
        round_trip!(Color::Rgb { r: 1, g: 2, b: 3 }, Color);
        round_trip!(CellWidth::Wide, CellWidth);
        round_trip!(CursorShape::Bar, CursorShape);
        round_trip!(Key::Char('é'), Key);
        round_trip!(KeyAction::Repeat, KeyAction);
        round_trip!(MouseButton::Other(4), MouseButton);
        round_trip!(MouseEventKind::Drag, MouseEventKind);
        round_trip!(ScrollCommand::Lines(-3), ScrollCommand);
        round_trip!(CellAttrs::BOLD | CellAttrs::ITALIC, CellAttrs);
        round_trip!(Modifiers::CTRL | Modifiers::ALT, Modifiers);
    }

    #[test]
    fn frame_update_round_trips() {
        let frame = FrameUpdate {
            terminal: TerminalId(7),
            seq: 9,
            cols: 80,
            rows: 24,
            full: true,
            shift: None,
            rows_changed: vec![RowUpdate {
                index: 0,
                cells: vec![Cell {
                    text: SmolStr::new("λ"),
                    fg: Color::Palette(2),
                    bg: Color::Default,
                    underline_color: Some(Color::Rgb { r: 1, g: 2, b: 3 }),
                    attrs: CellAttrs::BOLD | CellAttrs::UNDERLINE,
                    width: CellWidth::Narrow,
                }],
            }],
            cursor: CursorState {
                row: 1,
                col: 2,
                visible: true,
                shape: CursorShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len: 100,
                offset: 3,
            },
            modes: TerminalModes {
                bracketed_paste: true,
                ..TerminalModes::default()
            },
            title: Some("shell".to_owned()),
        };
        let json = serde_json::to_string(&frame).unwrap_or_else(|error| panic!("{error}"));
        let decoded: FrameUpdate =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, frame);
        let mut shifted = frame;
        shifted.full = false;
        shifted.shift = Some(-3);
        let json = serde_json::to_string(&shifted).unwrap();
        assert_eq!(serde_json::from_str::<FrameUpdate>(&json).unwrap(), shifted);
    }
}
