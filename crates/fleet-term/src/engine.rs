//! The implementation-independent terminal-emulation contract.

use fleet_proto::terminal::{
    CursorState, FrameUpdate, KeyEvent, MouseEvent, ScrollCommand, TerminalModes,
};
use thiserror::Error;

/// A terminal-side effect emitted while parsing the VT byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
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

/// Mode-aware terminal emulator used by the daemon-side host thread.
pub trait VtEngine: Send {
    /// Constructs an empty terminal grid with bounded scrollback.
    fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Result<Self, EngineError>
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
