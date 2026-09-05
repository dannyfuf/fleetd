//! Ghostty-backed virtual-terminal engine integration.
//!
//! `libghostty-vt 0.2.1` builds its vendored Ghostty source with Zig 0.15.2.

use std::sync::{Arc, Mutex, MutexGuard};

use fleet_core::ids::TerminalId;
use fleet_proto::terminal::{
    Cell, CellAttrs, CellWidth, Color, CursorShape, CursorState, FrameUpdate, Key, KeyAction,
    KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind, RowUpdate, ScrollCommand,
    TerminalModes, ViewportInfo, WheelEvent,
};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    mouse::{Encoder as MouseEncoder, EncoderSize as MouseEncoderSize},
    paste,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    screen::{CellWide, Screen, TrackedGridRef},
    style::{Style, StyleColor, Underline},
    terminal::{
        CompressionActivity, CompressionMode, CompressionResult, ConformanceLevel,
        DeviceAttributeFeature, DeviceAttributes, DeviceType, Mode, Point, PointCoordinate,
        PointSpace, PrimaryDeviceAttributes, ScrollViewport, SecondaryDeviceAttributes,
        SizeReportSize, TertiaryDeviceAttributes,
    },
};
use tracing::warn;

use crate::{
    engine::{EngineError, EngineEvent, VtEngine, WheelAction},
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
    compressed_activity: Option<CompressionActivity>,
    reusable_rows: bool,
    pending_shift: i32,
    events: Arc<Mutex<Vec<EngineEvent>>>,
    last_cursor: CursorState,
    last_frame_title: Option<String>,
    last_row_wrapped: Vec<bool>,
    history_epoch: u64,
    last_frame_history_epoch: u64,
    last_scrollback_len: usize,
    oldest_history: Option<TrackedGridRef>,
    output_since_frame: bool,
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
    pub fn new(cols: u16, rows: u16, scrollback_bytes: usize) -> Result<Self, EngineError> {
        <Self as VtEngine>::new(cols, rows, scrollback_bytes)
    }

    fn backend<T>(result: Result<T, libghostty_vt::Error>) -> Result<T, EngineError> {
        result.map_err(|error| EngineError::Backend(error.to_string()))
    }

    fn try_take_frame(&mut self, full: bool) -> Result<FrameUpdate, EngineError> {
        let full = full || self.history_epoch != self.last_frame_history_epoch;
        let snapshot = Self::backend(self.render_state.update(&self.terminal))?;
        let cols = Self::backend(snapshot.cols())?;
        let rows = Self::backend(snapshot.rows())?;
        let dirty = Self::backend(snapshot.dirty())?;
        let shift = (!full
            && self.reusable_rows
            && self.pending_shift != 0
            && self.pending_shift.unsigned_abs() < u32::from(rows))
        .then_some(self.pending_shift);
        let render_all = full || (dirty == Dirty::Full && shift.is_none());
        let cursor = cursor_from_snapshot(&snapshot, self.last_cursor);
        let mut rows_changed = Vec::new();
        let mut row_wrapped = Vec::with_capacity(usize::from(rows));
        let mut row_iteration = Self::backend(self.row_iterator.update(&snapshot))?;
        let mut index = 0_u16;
        while let Some(row) = row_iteration.next() {
            let row_dirty = Self::backend(row.dirty())?;
            let wrapped = Self::backend(Self::backend(row.raw_row())?.is_wrapped())?;
            let wrap_changed = usize::try_from(i32::from(index) + shift.unwrap_or(0))
                .ok()
                .and_then(|source| self.last_row_wrapped.get(source))
                != Some(&wrapped);
            row_wrapped.push(wrapped);
            let exposed = shift.is_some_and(|shift| {
                if shift > 0 {
                    index >= rows - shift as u16
                } else {
                    index < shift.unsigned_abs() as u16
                }
            });
            if render_all || exposed || (shift.is_none() && row_dirty) || wrap_changed {
                let mut cells = Vec::with_capacity(usize::from(cols));
                {
                    let mut cell_iteration = Self::backend(self.cell_iterator.update(row))?;
                    while let Some(cell) = cell_iteration.next() {
                        cells.push(convert_cell(cell)?);
                    }
                }
                rows_changed.push(RowUpdate {
                    index,
                    cells,
                    wrapped,
                });
            }
            Self::backend(row.set_dirty(false))?;
            index = index.saturating_add(1);
        }
        Self::backend(snapshot.set_dirty(Dirty::Clean))?;

        self.cols = cols;
        self.rows = rows;
        self.last_row_wrapped = row_wrapped;
        self.last_cursor = cursor;
        let current_title = self.title();
        let title = if render_all || current_title != self.last_frame_title {
            current_title.clone()
        } else {
            None
        };
        self.last_frame_title = current_title;
        self.last_frame_history_epoch = self.history_epoch;
        self.reusable_rows = true;
        self.pending_shift = 0;

        Ok(FrameUpdate {
            terminal: TerminalId(0),
            seq: 0,
            cols,
            rows,
            full: render_all,
            shift,
            rows_changed,
            cursor,
            viewport: self.viewport_info(),
            modes: self.modes(),
            title,
        })
    }

    fn viewport_info(&self) -> ViewportInfo {
        let scrollback_len = self.terminal.scrollback_rows().unwrap_or_default();
        ViewportInfo {
            scrollback_len,
            offset: self.viewport_offset(),
            history_epoch: self.history_epoch,
        }
    }

    /// Advances the absolute-row epoch when Ghostty can no longer preserve row identity.
    ///
    /// `TrackedGridRef` is the binding's page-list-aware identity primitive: the reference follows
    /// the oldest history cell through normal appends and viewport movement, but loses its value or
    /// moves away from history row zero when that row is pruned/reset. Length shrink catches clear
    /// and resize cases directly. Output without an available tracker conservatively invalidates
    /// identity; the byte budget cannot be compared to a count of history rows.
    fn update_history_epoch(&mut self, had_output: bool) {
        let scrollback_len = self.terminal.scrollback_rows().unwrap_or_default();
        let oldest_rebased = self.oldest_history.as_ref().is_some_and(|oldest| {
            !matches!(
                oldest.point(PointSpace::History),
                Ok(Some(PointCoordinate { x: 0, y: 0 }))
            )
        });
        let tracker_missing = self.last_scrollback_len > 0
            && scrollback_len > 0
            && self.oldest_history.is_none()
            && had_output;
        if history_identity_changed(
            self.last_scrollback_len,
            scrollback_len,
            oldest_rebased,
            tracker_missing,
        ) {
            self.history_epoch = self.history_epoch.saturating_add(1);
            self.oldest_history = None;
            self.reusable_rows = false;
            self.pending_shift = 0;
        }

        if scrollback_len == 0 {
            self.oldest_history = None;
        } else if self.oldest_history.is_none() {
            self.oldest_history = self
                .terminal
                .track_grid_ref(Point::History(PointCoordinate { x: 0, y: 0 }))
                .ok();
        }
        self.last_scrollback_len = scrollback_len;
    }
}

fn history_identity_changed(
    previous_len: usize,
    current_len: usize,
    oldest_rebased: bool,
    untracked_output: bool,
) -> bool {
    current_len < previous_len || oldest_rebased || untracked_output
}

impl VtEngine for GhosttyEngine {
    fn new(cols: u16, rows: u16, scrollback_bytes: usize) -> Result<Self, EngineError> {
        if cols == 0 || rows == 0 {
            return Err(EngineError::InvalidSize { cols, rows });
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Self::backend(Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: scrollback_bytes,
        }))?;
        // Fleet maps the platform Option/Alt modifier to terminal Alt. Match conventional
        // xterm behavior unless the child explicitly changes DEC mode 1036.
        Self::backend(terminal.set_mode(Mode::ALT_ESC_PREFIX, true))?;
        let pty_events = Arc::clone(&events);
        Self::backend(terminal.on_pty_write(move |_, bytes| {
            lock_events(&pty_events).push(EngineEvent::PtyWrite(bytes.to_vec()));
        }))?;
        Self::backend(terminal.on_device_attributes(|_| {
            // Match the VT220/ANSI-color capabilities of the portable xterm-256color TERM
            // advertised by `pty.rs`. An unanswered DA1 makes terminal applications wait for
            // input that never comes; claiming features Fleet does not implement is worse.
            Some(DeviceAttributes {
                primary: PrimaryDeviceAttributes::new(
                    ConformanceLevel::VT220,
                    &[DeviceAttributeFeature::ANSI_COLOR],
                ),
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType::VT220,
                    firmware_version: 0,
                    rom_cartridge: 0,
                },
                tertiary: TertiaryDeviceAttributes { unit_id: 0 },
            })
        }))?;
        Self::backend(
            terminal.on_xtversion(|_| Some(concat!("fleet ", env!("CARGO_PKG_VERSION")))),
        )?;
        Self::backend(terminal.on_size(|terminal| {
            Some(SizeReportSize {
                rows: terminal.rows().ok()?,
                columns: terminal.cols().ok()?,
                // Fleet has no pixel geometry on the daemon side. Unit cells keep pixel reports
                // internally consistent while the character-size report remains exact.
                cell_width: 1,
                cell_height: 1,
            })
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
            compressed_activity: None,
            reusable_rows: false,
            pending_shift: 0,
            events,
            last_cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            last_frame_title: None,
            last_row_wrapped: vec![false; usize::from(rows)],
            history_epoch: 0,
            last_frame_history_epoch: 0,
            last_scrollback_len: 0,
            oldest_history: None,
            output_since_frame: false,
            cols,
            rows,
        })
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.reusable_rows = false;
        self.pending_shift = 0;
        self.output_since_frame |= !bytes.is_empty();
        self.terminal.vt_write(bytes);
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError> {
        if cols == 0 || rows == 0 {
            return Err(EngineError::InvalidSize { cols, rows });
        }
        self.reusable_rows = false;
        self.pending_shift = 0;
        Self::backend(self.terminal.resize(cols, rows, 1, 1))?;
        configure_mouse_size(&mut self.mouse_encoder, cols, rows);
        // A column change reflows the primary screen and its history: wrapped rows merge or
        // split, so absolute scrollback lines no longer name the same text. The tracked oldest
        // cell can survive a reflow at history (0, 0), so this rebase is invisible to
        // `update_history_epoch` and has to be declared here.
        if cols != self.cols {
            self.history_epoch = self.history_epoch.saturating_add(1);
            self.oldest_history = None;
            self.reusable_rows = false;
            self.pending_shift = 0;
        }
        self.cols = cols;
        self.rows = rows;
        self.last_row_wrapped.resize(usize::from(rows), false);
        Ok(())
    }

    fn take_frame(&mut self, full: bool) -> FrameUpdate {
        let had_output = std::mem::take(&mut self.output_since_frame);
        self.update_history_epoch(had_output);
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
                    shift: None,
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
        let before = self.viewport_offset();
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
        let delta =
            before as i128 - self.viewport_offset() as i128 + i128::from(self.pending_shift);
        match i32::try_from(delta) {
            Ok(delta) => self.pending_shift = delta,
            Err(_) => self.reusable_rows = false,
        }
    }

    fn wheel(&mut self, event: &WheelEvent) -> WheelAction {
        let modes = self.modes();
        if !modes.alt_screen && !(event.mods.contains(Modifiers::SHIFT) && modes.mouse_reporting) {
            return WheelAction::Viewport(event.steps);
        }
        let steps = event
            .steps
            .unsigned_abs()
            .min(u32::from(self.rows) * 4)
            .min(1024);
        let bytes = if modes.mouse_reporting {
            self.encode_mouse(&MouseEvent {
                button: if event.steps < 0 {
                    MouseButton::WheelUp
                } else {
                    MouseButton::WheelDown
                },
                kind: MouseEventKind::Press,
                col: event.col.min(self.cols.saturating_sub(1)),
                row: event.row.min(self.rows.saturating_sub(1)),
                mods: event.mods,
            })
        } else if modes.alt_screen && mode(&self.terminal, Mode::ALT_SCROLL) {
            self.encode_key(&KeyEvent {
                key: if event.steps < 0 { Key::Up } else { Key::Down },
                mods: Modifiers::empty(),
                text: None,
                action: KeyAction::Press,
            })
        } else {
            return WheelAction::Drop;
        };
        WheelAction::Pty(bytes.repeat(steps as usize))
    }

    fn compress_idle(&mut self) {
        let Ok(activity) = self.terminal.compression_activity() else {
            return;
        };
        if self.compressed_activity == Some(activity) {
            return;
        }
        match self.terminal.compress(CompressionMode::Incremental) {
            Ok(CompressionResult::Pending) => {}
            _ => self.compressed_activity = Some(activity),
        }
    }

    fn viewport(&self) -> ViewportInfo {
        self.viewport_info()
    }

    fn rows(&self) -> u16 {
        self.rows
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
        self.key_encoder
            .set_options_from_terminal(&self.terminal)
            // GPUI has already classified Option as terminal Alt. The Ghostty encoder resets
            // this process-local preference while importing terminal modes, so restore it.
            .set_macos_option_as_alt(libghostty_vt::key::OptionAsAlt::True);
        let mut output = Vec::new();
        if let Err(error) = self.key_encoder.encode_to_vec(&event, &mut output) {
            warn!(%error, "failed to encode Ghostty key event");
            output.clear();
        }
        output
    }

    fn encode_mouse(&mut self, event: &MouseEvent) -> Vec<u8> {
        let wheel = matches!(
            event.button,
            MouseButton::WheelUp | MouseButton::WheelDown | MouseButton::Other(4 | 5)
        );
        let Ok(event) = ghostty_mouse_event(event) else {
            return Vec::new();
        };
        match event.action() {
            libghostty_vt::mouse::Action::Press if !wheel => self.mouse_button_down = true,
            libghostty_vt::mouse::Action::Release if !wheel => self.mouse_button_down = false,
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
    fn wheel_press_does_not_leave_a_held_button() {
        use fleet_proto::terminal::{Modifiers, MouseButton, MouseEventKind};
        let mut engine = GhosttyEngine::new(80, 24, 1024 * 1024).unwrap();
        engine.feed(b"\x1b[?1002h\x1b[?1006h");
        let mut event = MouseEvent {
            button: MouseButton::WheelUp,
            kind: MouseEventKind::Press,
            col: 2,
            row: 3,
            mods: Modifiers::empty(),
        };
        for button in [
            MouseButton::WheelUp,
            MouseButton::WheelDown,
            MouseButton::Other(4),
            MouseButton::Other(5),
        ] {
            event.button = button;
            assert!(!engine.encode_mouse(&event).is_empty());
            assert!(!engine.mouse_button_down);
        }
        event.button = MouseButton::Left;
        event.kind = MouseEventKind::Move;
        assert!(engine.encode_mouse(&event).is_empty());
        event.kind = MouseEventKind::Press;
        engine.encode_mouse(&event);
        event.button = MouseButton::WheelDown;
        engine.encode_mouse(&event);
        assert!(
            engine.mouse_button_down,
            "wheel must preserve real held buttons"
        );
    }

    #[test]
    fn renders_text_and_sgr_attributes() {
        let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
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
    fn frame_rows_expose_ghostty_soft_wraps() {
        let mut engine = GhosttyEngine::new(4, 3, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine.feed(b"abcdefghij");

        let frame = engine.take_frame(true);
        assert!(frame.rows_changed[0].wrapped);
        assert!(frame.rows_changed[1].wrapped);
        assert!(!frame.rows_changed[2].wrapped);
    }

    #[test]
    fn scrollback_shrink_advances_the_history_epoch() {
        let mut engine = GhosttyEngine::new(8, 2, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine.feed(b"one\r\ntwo\r\nthree\r\nfour");
        let before = engine.take_frame(true).viewport;
        assert!(before.scrollback_len > 0);
        assert_eq!(before.history_epoch, 0);

        engine.feed(b"\x1b[3J");
        let after = engine.take_frame(false).viewport;
        assert!(after.scrollback_len < before.scrollback_len);
        assert_eq!(after.history_epoch, 1);
    }

    #[test]
    fn a_changed_oldest_history_row_advances_the_epoch() {
        assert!(history_identity_changed(20, 20, true, false));
    }

    #[test]
    fn scroll_only_frames_do_not_advance_the_history_epoch() {
        let mut engine = GhosttyEngine::new(8, 2, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine.feed(b"one\r\ntwo\r\nthree\r\nfour");
        let epoch = engine.take_frame(true).viewport.history_epoch;

        engine.scroll(ScrollCommand::Top);
        assert_eq!(engine.take_frame(true).viewport.history_epoch, epoch);
        engine.scroll(ScrollCommand::Bottom);
        assert_eq!(engine.take_frame(true).viewport.history_epoch, epoch);
    }

    #[test]
    fn missing_history_tracker_conservatively_advances_on_output() {
        assert!(history_identity_changed(10, 10, false, true));
    }

    #[test]
    fn narrowing_the_terminal_advances_the_history_epoch() {
        // Reflow rewrites which absolute line holds which text, so every selection anchored
        // in scrollback must be dropped. Losing rows only pushes screen rows *into* history,
        // which keeps every existing line where it was.
        let mut engine = GhosttyEngine::new(16, 4, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        engine.feed(b"a long wrapped line\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
        let epoch = engine.take_frame(true).viewport.history_epoch;

        engine
            .resize(16, 2)
            .unwrap_or_else(|error| panic!("failed to resize engine: {error}"));
        assert_eq!(engine.take_frame(false).viewport.history_epoch, epoch);

        engine
            .resize(8, 2)
            .unwrap_or_else(|error| panic!("failed to resize engine: {error}"));
        assert_eq!(engine.take_frame(false).viewport.history_epoch, epoch + 1);
    }

    #[test]
    fn resizes_grid_and_full_frame() {
        let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
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
        let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
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
        let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
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
    fn terminal_queries_are_answered_for_full_screen_apps() {
        let mut engine = GhosttyEngine::new(80, 24, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));

        engine.feed(b"\x1b[c\x1b[>c\x1b[?7$p\x1b[6n\x1b[?u\x1b[18t\x1b[>q");
        let replies = engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                EngineEvent::PtyWrite(bytes) => Some(bytes),
                _ => None,
            })
            .collect::<Vec<_>>();

        for expected in [
            b"\x1b[?62;22c".as_slice(),
            b"\x1b[>1;0;0c".as_slice(),
            b"\x1b[?7;1$y".as_slice(),
            b"\x1b[1;1R".as_slice(),
            b"\x1b[?0u".as_slice(),
            b"\x1b[8;24;80t".as_slice(),
            b"\x1bP>|fleet 0.1.0\x1b\\".as_slice(),
        ] {
            assert!(
                replies.iter().any(|reply| reply == expected),
                "missing terminal reply {expected:?}; got {replies:?}"
            );
        }
    }

    #[test]
    fn nvim_mode_keys_encode_in_legacy_and_kitty_modes() {
        let mut engine = GhosttyEngine::new(80, 24, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        let key = |key, mods, text: Option<&str>| KeyEvent {
            key,
            mods,
            text: text.map(str::to_owned),
            action: fleet_proto::terminal::KeyAction::Press,
        };

        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Escape,
                fleet_proto::terminal::Modifiers::empty(),
                None,
            )),
            b"\x1b"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char('i'),
                fleet_proto::terminal::Modifiers::empty(),
                Some("i"),
            )),
            b"i"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char(';'),
                fleet_proto::terminal::Modifiers::SHIFT,
                Some(":"),
            )),
            b":"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char('c'),
                fleet_proto::terminal::Modifiers::CTRL,
                None,
            )),
            b"\x03"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char('['),
                fleet_proto::terminal::Modifiers::CTRL,
                None,
            )),
            b"\x1b[91;5u"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char('x'),
                fleet_proto::terminal::Modifiers::ALT,
                Some("x"),
            )),
            b"\x1bx"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Up,
                fleet_proto::terminal::Modifiers::SHIFT,
                None,
            )),
            b"\x1b[1;2A"
        );

        // Nvim enables Kitty's disambiguation flag with CSI > 1 u. The encoder must follow
        // that live terminal mode instead of continuing to emit an incompatible legacy mix.
        engine.feed(b"\x1b[>1u");
        assert_eq!(engine.modes().kitty_keyboard_flags, 1);
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Escape,
                fleet_proto::terminal::Modifiers::empty(),
                None,
            )),
            b"\x1b[27u"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char('i'),
                fleet_proto::terminal::Modifiers::empty(),
                Some("i"),
            )),
            b"i"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char(';'),
                fleet_proto::terminal::Modifiers::SHIFT,
                Some(":"),
            )),
            b":"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char('c'),
                fleet_proto::terminal::Modifiers::CTRL,
                None,
            )),
            b"\x1b[99;5u"
        );
        assert_eq!(
            engine.encode_key(&key(
                fleet_proto::terminal::Key::Char('['),
                fleet_proto::terminal::Modifiers::CTRL,
                None,
            )),
            b"\x1b[91;5u"
        );
    }

    #[test]
    fn modify_other_keys_encodes_ambiguous_control_keys() {
        let mut engine = GhosttyEngine::new(80, 24, 100)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        let shifted_semicolon = KeyEvent {
            key: fleet_proto::terminal::Key::Char(';'),
            mods: fleet_proto::terminal::Modifiers::SHIFT,
            text: Some(":".to_owned()),
            action: fleet_proto::terminal::KeyAction::Press,
        };
        let ctrl_i = KeyEvent {
            key: fleet_proto::terminal::Key::Char('i'),
            mods: fleet_proto::terminal::Modifiers::CTRL,
            text: None,
            action: fleet_proto::terminal::KeyAction::Press,
        };

        assert_eq!(engine.encode_key(&shifted_semicolon), b":");
        assert_eq!(engine.encode_key(&ctrl_i), b"\x1b[105;5u");
        engine.feed(b"\x1b[>4;2m");
        // Shifted punctuation remains its composed text, while an ambiguous C0 control key is
        // disambiguated exactly as xterm modifyOtherKeys level 2 specifies.
        assert_eq!(engine.encode_key(&shifted_semicolon), b":");
        assert_eq!(engine.encode_key(&ctrl_i), b"\x1b[27;5;105~");
    }

    #[test]
    fn collects_terminal_effects() {
        let mut engine = GhosttyEngine::new(8, 3, 100 * 1024)
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

    #[test]
    fn wheel_routing_uses_live_modes() {
        for (modes, shift, expected) in [
            ("", false, "viewport"),
            ("\x1b[?1000h\x1b[?1006h", false, "viewport"),
            ("", true, "viewport"),
            ("\x1b[?1000h\x1b[?1006h", true, "mouse"),
            ("\x1b[?1049h\x1b[?1000h\x1b[?1006h", false, "mouse"),
            ("\x1b[?1049h\x1b[?1000h\x1b[?1006h", true, "mouse"),
            ("\x1b[?1049h\x1b[?1007h\x1b[?1l", false, "arrow"),
            ("\x1b[?1049h\x1b[?1007h\x1b[?1h", false, "app_arrow"),
            ("\x1b[?1049h\x1b[?1007l", false, "drop"),
        ] {
            for steps in [-2, 2] {
                let mut engine = GhosttyEngine::new(80, 24, 1024 * 1024).unwrap();
                engine.feed(modes.as_bytes());
                let mods = if shift {
                    Modifiers::SHIFT
                } else {
                    Modifiers::empty()
                };
                let event = WheelEvent {
                    steps,
                    col: 2,
                    row: 3,
                    mods,
                };
                let expected = match expected {
                    "viewport" => WheelAction::Viewport(steps),
                    "mouse" => WheelAction::Pty(match (steps < 0, shift) {
                        (true, false) => b"\x1b[<64;3;4M".repeat(2),
                        (false, false) => b"\x1b[<65;3;4M".repeat(2),
                        (true, true) => b"\x1b[<68;3;4M".repeat(2),
                        (false, true) => b"\x1b[<69;3;4M".repeat(2),
                    }),
                    "arrow" => WheelAction::Pty(if steps < 0 {
                        b"\x1b[A".repeat(2)
                    } else {
                        b"\x1b[B".repeat(2)
                    }),
                    "app_arrow" => WheelAction::Pty(if steps < 0 {
                        b"\x1bOA".repeat(2)
                    } else {
                        b"\x1bOB".repeat(2)
                    }),
                    _ => WheelAction::Drop,
                };
                assert_eq!(
                    engine.wheel(&event),
                    expected,
                    "modes={modes:?} shift={shift} steps={steps}"
                );
            }
        }
    }

    #[test]
    fn wheel_mouse_coordinates_clamp_at_grid_boundary() {
        let mut engine = GhosttyEngine::new(80, 24, 1024).unwrap();
        engine.feed(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
        for (steps, mods, expected) in [
            (-1, Modifiers::empty(), b"\x1b[<64;80;24M"),
            (1, Modifiers::empty(), b"\x1b[<65;80;24M"),
            (-1, Modifiers::SHIFT, b"\x1b[<68;80;24M"),
            (1, Modifiers::SHIFT, b"\x1b[<69;80;24M"),
        ] {
            assert_eq!(
                engine.wheel(&WheelEvent {
                    steps,
                    col: u16::MAX,
                    row: u16::MAX,
                    mods
                }),
                WheelAction::Pty(expected.to_vec())
            );
        }
    }

    #[test]
    fn pty_wheel_expansion_is_bounded_by_rows_and_absolute_limit() {
        for (rows, limit) in [(24, 96), (300, 1024)] {
            for (modes, up, down) in [
                (
                    b"\x1b[?1049h\x1b[?1000h\x1b[?1006h".as_slice(),
                    b"\x1b[<64;1;1M".as_slice(),
                    b"\x1b[<65;1;1M".as_slice(),
                ),
                (
                    b"\x1b[?1049h\x1b[?1007h\x1b[?1l".as_slice(),
                    b"\x1b[A".as_slice(),
                    b"\x1b[B".as_slice(),
                ),
            ] {
                let mut engine = GhosttyEngine::new(80, rows, 1024).unwrap();
                engine.feed(modes);
                for steps in [0, -1, 1, i32::MIN, i32::MAX] {
                    let count = (steps.unsigned_abs() as usize).min(limit);
                    assert_eq!(
                        engine.wheel(&WheelEvent {
                            steps,
                            col: 0,
                            row: 0,
                            mods: Modifiers::empty()
                        }),
                        WheelAction::Pty(if steps < 0 { up } else { down }.repeat(count))
                    );
                }
            }
        }
        let mut engine = GhosttyEngine::new(80, 24, 1024).unwrap();
        assert_eq!(
            engine.wheel(&WheelEvent {
                steps: i32::MIN,
                col: 0,
                row: 0,
                mods: Modifiers::empty()
            }),
            WheelAction::Viewport(i32::MIN)
        );
    }

    #[test]
    fn output_preserves_history_anchor_and_bottom_follows() {
        let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
        engine.feed("line\r\n".repeat(100).as_bytes());
        assert_eq!(engine.viewport_offset(), 0);
        engine.scroll(ScrollCommand::Lines(-20));
        let before = engine.take_frame(true);
        engine.feed(b"new output\r\nnew output\r\n");
        let after = engine.take_frame(true);
        assert_eq!(before.rows_changed, after.rows_changed);
        assert_eq!(after.viewport.offset, before.viewport.offset + 2);
    }

    #[test]
    fn row_shift_matches_full_snapshot_and_invalidates_on_output_resize() {
        let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
        engine.feed(
            (0..100)
                .map(|i| format!("{i} {}\r\n", "x".repeat(i % 70)))
                .collect::<String>()
                .as_bytes(),
        );
        let mut mirror = engine
            .take_frame(true)
            .rows_changed
            .into_iter()
            .map(|r| (r.cells, r.wrapped))
            .collect::<Vec<_>>();
        for delta in [-3, 1, -4, 2] {
            engine.scroll(ScrollCommand::Lines(delta));
            let frame = engine.take_frame(false);
            assert_eq!(frame.shift, Some(delta));
            assert!(!frame.full);
            assert_eq!(frame.rows_changed.len(), delta.unsigned_abs() as usize);
            if delta > 0 {
                mirror.rotate_left(delta as usize);
            } else {
                mirror.rotate_right(delta.unsigned_abs() as usize);
            }
            for row in frame.rows_changed {
                mirror[usize::from(row.index)] = (row.cells, row.wrapped);
            }
            assert_eq!(
                mirror,
                engine
                    .take_frame(true)
                    .rows_changed
                    .into_iter()
                    .map(|r| (r.cells, r.wrapped))
                    .collect::<Vec<_>>()
            );
        }
        engine.scroll(ScrollCommand::Lines(-1));
        engine.feed(b"new output");
        assert_eq!(engine.take_frame(false).shift, None);
        engine.scroll(ScrollCommand::Lines(-1));
        engine.resize(41, 10).unwrap();
        assert_eq!(engine.take_frame(false).shift, None);
        engine.scroll(ScrollCommand::Lines(-1));
        assert_eq!(engine.take_frame(true).shift, None);
    }

    #[test]
    fn history_epoch_change_disables_pending_shift_and_replaces_every_row() {
        let mut engine = GhosttyEngine::new(8, 3, 1024 * 1024).unwrap();
        engine.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let epoch = engine.take_frame(true).viewport.history_epoch;
        engine.scroll(ScrollCommand::Lines(-1));
        assert_ne!(engine.pending_shift, 0);
        // Emulate the backend pruning history without passing through feed's invalidation.
        engine.terminal.vt_write(b"\x1b[3J");
        let frame = engine.take_frame(false);
        assert!(frame.viewport.history_epoch > epoch);
        assert_eq!(frame.shift, None);
        assert!(frame.full);
        assert_eq!(frame.rows_changed.len(), usize::from(frame.rows));
    }

    #[test]
    fn viewport_frame_timing_200x60() {
        use std::time::Instant;
        let mut engine = GhosttyEngine::new(200, 60, 1_073_741_824).unwrap();
        let output = (0..2000)
            .map(|i| format!("\x1b[3{}m{i:06} {}\x1b[0m\r\n", i % 8, "x".repeat(190)))
            .collect::<String>();
        engine.feed(output.as_bytes());
        engine.take_frame(true);
        for full in [true, false] {
            engine.scroll(ScrollCommand::Bottom);
            engine.take_frame(true);
            let mut samples = Vec::new();
            let mut snapshot_us = 0;
            let mut json_us = 0;
            for _ in 0..100 {
                engine.scroll(ScrollCommand::Lines(-1));
                let start = Instant::now();
                let mut frame = engine.take_frame(full);
                frame.terminal = TerminalId(1);
                frame.seq = 1;
                let snapshot = start.elapsed();
                let bytes = serde_json::to_vec(&frame).unwrap();
                std::hint::black_box(bytes);
                let elapsed = start.elapsed();
                snapshot_us += snapshot.as_micros();
                json_us += (elapsed - snapshot).as_micros();
                samples.push(elapsed.as_micros());
            }
            samples.sort_unstable();
            println!(
                "200x60 viewport frame full={full}: snapshot mean={} us; JSON mean={} us; total median={} us p95={} us max={} us (100 moves)",
                snapshot_us / 100,
                json_us / 100,
                samples[50],
                samples[95],
                samples[99]
            );
        }
    }

    #[test]
    fn scrollback_budget_is_bytes() {
        let mut engine = GhosttyEngine::new(40, 10, 10 * 1024 * 1024)
            .unwrap_or_else(|error| panic!("failed to create engine: {error}"));
        let output = (1..=5_000)
            .map(|line| format!("{line}\r\n"))
            .collect::<String>();

        engine.feed(output.as_bytes());

        assert_eq!(engine.take_frame(true).viewport.scrollback_len, 4_991);
    }

    #[test]
    fn small_byte_budget_bounds_history_and_zero_disables_it() {
        let output = "1234567890\r\n".repeat(10_000);
        for budget in [0, 10_000] {
            let mut engine = GhosttyEngine::new(40, 10, budget).unwrap();
            engine.feed(output.as_bytes());
            let retained = engine.viewport().scrollback_len;
            if budget == 0 {
                assert_eq!(retained, 0);
            } else {
                let mut raw = Terminal::new(TerminalOptions {
                    cols: 40,
                    rows: 10,
                    max_scrollback: budget,
                })
                .unwrap();
                raw.vt_write(output.as_bytes());
                assert_eq!(retained, raw.scrollback_rows().unwrap());
                assert!(
                    retained < 4_991,
                    "byte budget must retain less than the 10 MiB case"
                );
            }
        }
    }
}
