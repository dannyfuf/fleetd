//! The implementation-independent terminal-emulation contract.

use fleet_proto::terminal::{
    CursorState, FrameUpdate, KeyEvent, MouseEvent, ScrollCommand, TerminalModes, ViewportInfo,
    WheelEvent,
};
use thiserror::Error;

/// A terminal-side effect emitted while parsing the VT byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// The emulator answered a terminal query and the bytes must go back to the PTY child.
    PtyWrite(Vec<u8>),
    /// The terminal title changed through OSC 0 or OSC 2.
    Title(String),
    /// The terminal emitted an audible or visual bell.
    Bell,
    /// The terminal reported its current working directory through OSC 7.
    Cwd(String),
    /// The terminal requested a clipboard write through OSC 52.
    ClipboardWrite {
        /// MIME type of the preferred clipboard representation.
        mime: String,
        /// Decoded clipboard contents.
        data: String,
    },
}

/// Failure while constructing or reconfiguring a virtual-terminal engine.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The requested grid dimensions were zero.
    #[error("terminal dimensions must be non-zero (got {cols}x{rows})")]
    InvalidSize {
        /// Requested column count.
        cols: u16,
        /// Requested row count.
        rows: u16,
    },
    /// The selected VT backend rejected an operation.
    #[error("virtual-terminal backend error: {0}")]
    Backend(String),
}

/// The live-mode decision for one wheel event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WheelAction {
    /// Move Fleet's primary viewport.
    Viewport(i32),
    /// Send encoded application input to the PTY.
    Pty(Vec<u8>),
    /// No scrolling is supported in the active mode.
    Drop,
}

/// Mode-aware terminal emulator used by the daemon-side host thread.
pub trait VtEngine: Send {
    /// Constructs an empty terminal grid with bounded scrollback.
    fn new(cols: u16, rows: u16, scrollback_bytes: usize) -> Result<Self, EngineError>
    where
        Self: Sized;

    /// Feeds bytes read from the PTY into the emulator.
    fn feed(&mut self, bytes: &[u8]);

    /// Resizes the terminal grid, reflowing the primary screen where supported.
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError>;

    /// Takes changed rows, or the complete viewport when `full` is true.
    fn take_frame(&mut self, full: bool) -> FrameUpdate;

    /// Returns the current viewport-relative cursor state.
    fn cursor(&self) -> CursorState;

    /// Returns terminal modes needed by renderers and input encoders.
    fn modes(&self) -> TerminalModes;

    /// Returns the current non-empty title, when one has been set.
    fn title(&self) -> Option<String>;

    /// Moves the visible scrollback viewport.
    fn scroll(&mut self, command: ScrollCommand);

    /// Routes wheel input using live terminal modes and Ghostty encoders.
    fn wheel(&mut self, event: &WheelEvent) -> WheelAction;

    /// Performs one bounded incremental history-compression step while idle.
    fn compress_idle(&mut self);

    /// Returns history bounds for coalescing viewport commands.
    fn viewport(&self) -> ViewportInfo;

    /// Returns the current viewport height.
    fn rows(&self) -> u16;

    /// Returns the current bottom-relative scrollback offset.
    fn viewport_offset(&self) -> usize;

    /// Encodes a semantic keyboard event according to current terminal modes.
    fn encode_key(&mut self, event: &KeyEvent) -> Vec<u8>;

    /// Encodes a semantic mouse event according to current reporting modes.
    fn encode_mouse(&mut self, event: &MouseEvent) -> Vec<u8>;

    /// Encodes pasted text, using bracketed paste when enabled.
    fn encode_paste(&self, text: &str) -> Vec<u8>;

    /// Drains side effects collected since the preceding call.
    fn take_events(&mut self) -> Vec<EngineEvent>;
}
