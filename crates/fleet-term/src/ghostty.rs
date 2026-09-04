//! Ghostty-backed virtual-terminal engine integration.
//!
//! `libghostty-vt 0.2.1` builds its vendored Ghostty source with Zig 0.15.2.

use std::sync::{Arc, Mutex, MutexGuard};

use fleet_core::ids::TerminalId;
use fleet_proto::terminal::{
    Cell, CellAttrs, CellWidth, Color, CursorShape, CursorState, FrameUpdate, KeyEvent, MouseEvent,
    RowUpdate, ScrollCommand, TerminalModes, ViewportInfo,
};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    mouse::{Encoder as MouseEncoder, EncoderSize as MouseEncoderSize},
    paste,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    screen::{CellWide, Screen},
    style::{Style, StyleColor, Underline},
    terminal::{Mode, ScrollViewport},
};
use tracing::warn;

use crate::{
    engine::{EngineError, EngineEvent, VtEngine},
    keys::{ghostty_key_event, ghostty_mouse_event},
};

/// A headless Ghostty terminal emulator and its persistent render snapshot state.
pub struct GhosttyEngine {
    terminal: Terminal<'static, 'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    key_encoder: libghostty_vt::key::Encoder<'static>,
    mouse_encoder: MouseEncoder<'static>,
    mouse_button_down: bool,
    events: Arc<Mutex<Vec<EngineEvent>>>,
    last_cursor: CursorState,
    last_frame_title: Option<String>,
    cols: u16,
    rows: u16,
}

// SAFETY: libghostty's safe wrapper is conservatively !Send because its handles contain
// NonNull pointers and callback trait objects. Ghostty's C VT objects have no thread affinity.
// This wrapper exposes only exclusive mutation, never leaks handles, and installs only callbacks
// whose captured queue is Send + Sync, so moving the complete engine between threads is sound.
unsafe impl Send for GhosttyEngine {}

impl GhosttyEngine {
    /// Constructs a Ghostty terminal engine.
    pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Result<Self, EngineError> {
        <Self as VtEngine>::new(cols, rows, scrollback_lines)
    }

    fn backend<T>(result: Result<T, libghostty_vt::Error>) -> Result<T, EngineError> {
        result.map_err(|error| EngineError::Backend(error.to_string()))
    }

    fn try_take_frame(&mut self, full: bool) -> Result<FrameUpdate, EngineError> {
        let snapshot = Self::backend(self.render_state.update(&self.terminal))?;
        let cols = Self::backend(snapshot.cols())?;
        let rows = Self::backend(snapshot.rows())?;
        let dirty = Self::backend(snapshot.dirty())?;
        let render_all = full || dirty == Dirty::Full;
        let cursor = cursor_from_snapshot(&snapshot, self.last_cursor);
        let mut rows_changed = Vec::new();
        let mut row_iteration = Self::backend(self.row_iterator.update(&snapshot))?;
        let mut index = 0_u16;
        while let Some(row) = row_iteration.next() {
            let row_dirty = Self::backend(row.dirty())?;
            if render_all || row_dirty {
                let mut cells = Vec::with_capacity(usize::from(cols));
                {
                    let mut cell_iteration = Self::backend(self.cell_iterator.update(row))?;
                    while let Some(cell) = cell_iteration.next() {
                        cells.push(convert_cell(cell)?);
                    }
                }
                rows_changed.push(RowUpdate { index, cells });
            }
            Self::backend(row.set_dirty(false))?;
            index = index.saturating_add(1);
        }
        Self::backend(snapshot.set_dirty(Dirty::Clean))?;

        self.cols = cols;
        self.rows = rows;
        self.last_cursor = cursor;
        let current_title = self.title();
        let title = if render_all || current_title != self.last_frame_title {
            current_title.clone()
        } else {
            None
        };
        self.last_frame_title = current_title;

        Ok(FrameUpdate {
            terminal: TerminalId(0),
            seq: 0,
            cols,
            rows,
            full: render_all,
            rows_changed,
            cursor,
            viewport: self.viewport_info(),
            modes: self.modes(),
            title,
        })
    }

    fn viewport_info(&self) -> ViewportInfo {
        ViewportInfo {
            scrollback_len: self.terminal.scrollback_rows().unwrap_or_default(),
            offset: self.viewport_offset(),
        }
    }
}

impl VtEngine for GhosttyEngine {
    fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Result<Self, EngineError> {
        if cols == 0 || rows == 0 {
            return Err(EngineError::InvalidSize { cols, rows });
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Self::backend(Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: scrollback_lines,
        }))?;
        let title_events = Arc::clone(&events);
        Self::backend(terminal.on_title_changed(move |terminal| {
            if let Ok(title) = terminal.title() {
                lock_events(&title_events).push(EngineEvent::Title(title.to_owned()));
            }
        }))?;
        let cwd_events = Arc::clone(&events);
        Self::backend(terminal.on_pwd_changed(move |terminal| {
            if let Ok(cwd) = terminal.pwd() {
                lock_events(&cwd_events).push(EngineEvent::Cwd(cwd.to_owned()));
            }
        }))?;
        let bell_events = Arc::clone(&events);
        Self::backend(terminal.on_bell(move |_| {
            lock_events(&bell_events).push(EngineEvent::Bell);
        }))?;
        let clipboard_events = Arc::clone(&events);
        Self::backend(terminal.on_clipboard_write(move |_, write| {
            if let Some(content) = write
                .contents()
                .find(|content| content.mime == "text/plain")
                .or_else(|| write.contents().next())
            {
                lock_events(&clipboard_events).push(EngineEvent::ClipboardWrite {
                    mime: content.mime.to_owned(),
                    data: content.data.to_owned(),
                });
            }
            Ok(())
        }))?;

        let render_state = Self::backend(RenderState::new())?;
        let row_iterator = Self::backend(RowIterator::new())?;
        let cell_iterator = Self::backend(CellIterator::new())?;
        let key_encoder = Self::backend(libghostty_vt::key::Encoder::new())?;
        let mut mouse_encoder = Self::backend(MouseEncoder::new())?;
        configure_mouse_size(&mut mouse_encoder, cols, rows);
        Ok(Self {
            terminal,
            render_state,
            row_iterator,
            cell_iterator,
            key_encoder,
            mouse_encoder,
            mouse_button_down: false,
            events,
            last_cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            last_frame_title: None,
            cols,
            rows,
        })
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError> {
        if cols == 0 || rows == 0 {
            return Err(EngineError::InvalidSize { cols, rows });
        }
        Self::backend(self.terminal.resize(cols, rows, 1, 1))?;
        configure_mouse_size(&mut self.mouse_encoder, cols, rows);
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    fn take_frame(&mut self, full: bool) -> FrameUpdate {
        match self.try_take_frame(full) {
            Ok(frame) => frame,
            Err(error) => {
                warn!(%error, "failed to snapshot Ghostty terminal");
                FrameUpdate {
                    terminal: TerminalId(0),
                    seq: 0,
                    cols: self.cols,
                    rows: self.rows,
                    full,
                    rows_changed: Vec::new(),
                    cursor: self.cursor(),
                    viewport: self.viewport_info(),
                    modes: self.modes(),
                    title: None,
                }
            }
        }
    }

    fn cursor(&self) -> CursorState {
        CursorState {
            row: self.terminal.cursor_y().unwrap_or(self.last_cursor.row),
            col: self.terminal.cursor_x().unwrap_or(self.last_cursor.col),
            visible: self
                .terminal
                .is_cursor_visible()
                .unwrap_or(self.last_cursor.visible),
            shape: self.last_cursor.shape,
        }
    }

    fn modes(&self) -> TerminalModes {
        TerminalModes {
            alt_screen: self
                .terminal
                .active_screen()
                .is_ok_and(|screen| screen == Screen::Alternate),
            mouse_reporting: self.terminal.is_mouse_tracking().unwrap_or(false),
            bracketed_paste: mode(&self.terminal, Mode::BRACKETED_PASTE),
            focus_events: mode(&self.terminal, Mode::FOCUS_EVENT),
            kitty_keyboard_flags: self
                .terminal
                .kitty_keyboard_flags()
                .map(|flags| flags.bits())
                .unwrap_or_default(),
            app_cursor_keys: mode(&self.terminal, Mode::DECCKM),
        }
    }

    fn title(&self) -> Option<String> {
        self.terminal
            .title()
            .ok()
            .filter(|title| !title.is_empty())
            .map(str::to_owned)
    }

    fn scroll(&mut self, command: ScrollCommand) {
        let scroll = match command {
            ScrollCommand::Lines(lines) => ScrollViewport::Delta(lines as isize),
            ScrollCommand::Pages(pages) => {
                ScrollViewport::Delta((pages as isize).saturating_mul(self.rows as isize))
            }
            ScrollCommand::Top => ScrollViewport::Top,
            ScrollCommand::Bottom => ScrollViewport::Bottom,
            ScrollCommand::ToOffset(offset) => {
                let scrollback = self.terminal.scrollback_rows().unwrap_or_default();
                ScrollViewport::Row(scrollback.saturating_sub(offset.min(scrollback)))
            }
        };
        self.terminal.scroll_viewport(scroll);
    }

    fn viewport_offset(&self) -> usize {
        self.terminal.scrollbar().map_or(0, |scrollbar| {
            let bottom = scrollbar.offset.saturating_add(scrollbar.len);
            usize::try_from(scrollbar.total.saturating_sub(bottom)).unwrap_or(usize::MAX)
        })
    }

    fn encode_key(&mut self, event: &KeyEvent) -> Vec<u8> {
        let Ok(event) = ghostty_key_event(event) else {
            return Vec::new();
        };
        self.key_encoder.set_options_from_terminal(&self.terminal);
        let mut output = Vec::new();
        if let Err(error) = self.key_encoder.encode_to_vec(&event, &mut output) {
            warn!(%error, "failed to encode Ghostty key event");
            output.clear();
        }
        output
    }

    fn encode_mouse(&mut self, event: &MouseEvent) -> Vec<u8> {
        let Ok(event) = ghostty_mouse_event(event) else {
            return Vec::new();
        };
        match event.action() {
            libghostty_vt::mouse::Action::Press => self.mouse_button_down = true,
            libghostty_vt::mouse::Action::Release => self.mouse_button_down = false,
            libghostty_vt::mouse::Action::Motion => {}
            _ => {}
        }
        self.mouse_encoder
            .set_options_from_terminal(&self.terminal)
            .set_any_button_pressed(self.mouse_button_down);
        let mut output = Vec::new();
        if let Err(error) = self.mouse_encoder.encode_to_vec(&event, &mut output) {
            warn!(%error, "failed to encode Ghostty mouse event");
            output.clear();
        }
        output
    }

    fn encode_paste(&self, text: &str) -> Vec<u8> {
        let mut input = text.as_bytes().to_vec();
        let mut output = vec![0_u8; input.len().saturating_add(12)];
        match paste::encode(&mut input, self.modes().bracketed_paste, &mut output) {
            Ok(written) => {
                output.truncate(written);
                output
            }
            Err(error) => {
                warn!(%error, "failed to encode Ghostty paste");
                Vec::new()
            }
        }
    }

    fn take_events(&mut self) -> Vec<EngineEvent> {
        std::mem::take(&mut *lock_events(&self.events))
    }
}

fn lock_events(events: &Mutex<Vec<EngineEvent>>) -> MutexGuard<'_, Vec<EngineEvent>> {
    match events.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn mode(terminal: &Terminal<'_, '_>, mode: Mode) -> bool {
    terminal.mode(mode).unwrap_or(false)
}

fn configure_mouse_size(encoder: &mut MouseEncoder<'_>, cols: u16, rows: u16) {
    encoder.set_size(MouseEncoderSize {
        screen_width: u32::from(cols),
        screen_height: u32::from(rows),
        cell_width: 1,
        cell_height: 1,
        padding_top: 0,
        padding_bottom: 0,
        padding_right: 0,
        padding_left: 0,
    });
}

fn cursor_from_snapshot(
    snapshot: &libghostty_vt::render::Snapshot<'_, '_>,
    fallback: CursorState,
) -> CursorState {
    let position = snapshot.cursor_viewport().ok().flatten();
    CursorState {
        row: position.map_or(fallback.row, |cursor| cursor.y),
        col: position.map_or(fallback.col, |cursor| cursor.x),
        visible: snapshot.cursor_visible().unwrap_or(fallback.visible) && position.is_some(),
        shape: snapshot
            .cursor_visual_style()
            .map(cursor_shape)
            .unwrap_or(fallback.shape),
    }
}

fn cursor_shape(shape: CursorVisualStyle) -> CursorShape {
    match shape {
        CursorVisualStyle::Bar => CursorShape::Bar,
        CursorVisualStyle::Underline => CursorShape::Underline,
        CursorVisualStyle::Block | CursorVisualStyle::BlockHollow => CursorShape::Block,
        _ => CursorShape::Block,
    }
}

fn convert_cell(cell: &libghostty_vt::render::CellIteration<'_, '_>) -> Result<Cell, EngineError> {
    let mut text = String::new();
    GhosttyEngine::backend(cell.graphemes_utf8(&mut text))?;
    let style = GhosttyEngine::backend(cell.style())?;
    let width = GhosttyEngine::backend(cell.raw_cell())
        .and_then(|cell| GhosttyEngine::backend(cell.wide()))?;
    Ok(Cell {
        text: text.into(),
        fg: convert_color(style.fg_color),
        bg: convert_color(style.bg_color),
        underline_color: optional_color(style.underline_color),
        attrs: cell_attrs(style),
        width: match width {
            CellWide::Narrow => CellWidth::Narrow,
            CellWide::Wide => CellWidth::Wide,
            CellWide::SpacerTail | CellWide::SpacerHead => CellWidth::Spacer,
        },
    })
}

fn convert_color(color: StyleColor) -> Color {
    match color {
        StyleColor::None => Color::Default,
        StyleColor::Palette(index) => Color::Palette(index.0),
        StyleColor::Rgb(color) => Color::Rgb {
            r: color.r,
            g: color.g,
            b: color.b,
        },
    }
}

fn optional_color(color: StyleColor) -> Option<Color> {
    match color {
        StyleColor::None => None,
        value => Some(convert_color(value)),
    }
}

fn cell_attrs(style: Style) -> CellAttrs {
    let mut attrs = CellAttrs::empty();
    attrs.set(CellAttrs::BOLD, style.bold);
    attrs.set(CellAttrs::DIM, style.faint);
    attrs.set(CellAttrs::ITALIC, style.italic);
    attrs.set(CellAttrs::STRIKETHROUGH, style.strikethrough);
    attrs.set(CellAttrs::INVERSE, style.inverse);
    attrs.set(CellAttrs::BLINK, style.blink);
    attrs.set(CellAttrs::INVISIBLE, style.invisible);
    match style.underline {
        Underline::None => {}
        Underline::Double => attrs.insert(CellAttrs::DOUBLE_UNDERLINE),
        Underline::Curly => attrs.insert(CellAttrs::CURLY_UNDERLINE),
        Underline::Single | Underline::Dotted | Underline::Dashed => {
            attrs.insert(CellAttrs::UNDERLINE);
        }
        _ => attrs.insert(CellAttrs::UNDERLINE),
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_and_sgr_attributes() {
        let mut engine = GhosttyEngine::new(8, 3, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine.feed(b"\x1b[31mhi\x1b[0m\r\nx");
        let frame = engine.take_frame(true);
        let first = &frame.rows_changed[0].cells;
        assert_eq!(first[0].text.as_str(), "h");
        assert_eq!(first[1].text.as_str(), "i");
        assert_eq!(first[0].fg, Color::Palette(1));
        assert!(first[0].attrs.is_empty());
        assert_eq!(frame.rows_changed[1].cells[0].text.as_str(), "x");
        assert_eq!(frame.rows_changed[1].cells[0].fg, Color::Default);
    }

    #[test]
    fn resizes_grid_and_full_frame() {
        let mut engine = GhosttyEngine::new(8, 3, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine
            .resize(12, 5)
            .unwrap_or_else(|error| panic!("failed to resize engine: {error}"));
        let frame = engine.take_frame(true);
        assert_eq!((frame.cols, frame.rows), (12, 5));
        assert_eq!(frame.rows_changed.len(), 5);
        assert!(frame.rows_changed.iter().all(|row| row.cells.len() == 12));
    }

    #[test]
    fn incremental_frame_contains_only_dirty_rows() {
        let mut engine = GhosttyEngine::new(8, 3, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        let _ = engine.take_frame(true);
        engine.feed(b"\x1b[2;1Hx");
        let frame = engine.take_frame(false);
        assert!(!frame.full);
        assert!(frame.rows_changed.len() < usize::from(frame.rows));
        assert!(frame.rows_changed.iter().all(|row| row.index != 2));
        let changed = frame
            .rows_changed
            .iter()
            .find(|row| row.index == 1)
            .unwrap_or_else(|| panic!("written row was not dirty"));
        assert_eq!(changed.cells[0].text.as_str(), "x");
    }

    #[test]
    fn application_cursor_and_bracketed_paste_follow_modes() {
        let mut engine = GhosttyEngine::new(8, 3, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine.feed(b"\x1b[?1h\x1b[?2004h");
        let up = KeyEvent {
            key: fleet_proto::terminal::Key::Up,
            mods: fleet_proto::terminal::Modifiers::empty(),
            text: None,
            action: fleet_proto::terminal::KeyAction::Press,
        };
        assert_eq!(engine.encode_key(&up), b"\x1bOA");
        let ctrl_c = KeyEvent {
            key: fleet_proto::terminal::Key::Char('c'),
            mods: fleet_proto::terminal::Modifiers::CTRL,
            text: None,
            action: fleet_proto::terminal::KeyAction::Press,
        };
        assert_eq!(engine.encode_key(&ctrl_c), b"\x03");
        assert_eq!(engine.encode_paste("hello"), b"\x1b[200~hello\x1b[201~");
    }

    #[test]
    fn collects_terminal_effects() {
        let mut engine = GhosttyEngine::new(8, 3, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine.feed(b"\x07\x1b]2;Fleet title\x07\x1b]7;file:///tmp\x07\x1b]52;c;aGVsbG8=\x07");
        let events = engine.take_events();
        assert!(events.contains(&EngineEvent::Bell));
        assert!(events.contains(&EngineEvent::Title("Fleet title".to_owned())));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, EngineEvent::Cwd(_)))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::ClipboardWrite { data, .. } if data == "hello"
        )));
    }
}
