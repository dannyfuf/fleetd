//! Ghostty-backed virtual-terminal engine integration.

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
    row_wrapped: Vec<bool>,
    graphemes: Vec<char>,
    history_epoch: u64,
    last_frame_history_epoch: u64,
    last_scrollback_len: usize,
    oldest_history: Option<TrackedGridRef>,
    output_since_frame: bool,
    snapshot_invalid: bool,
    cols: u16,
    rows: u16,
    #[cfg(test)]
    fail_next: Option<TestFailure>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestFailure {
    Snapshot,
    Compression,
    Key,
    Mouse,
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

    #[cfg(test)]
    pub(crate) fn fail_next_snapshot(&mut self) {
        self.fail_next = Some(TestFailure::Snapshot);
    }

    fn snapshot_frame(&mut self, full: bool) -> Result<FrameUpdate, EngineError> {
        let full =
            full || self.snapshot_invalid || self.history_epoch != self.last_frame_history_epoch;
        let snapshot = backend(self.render_state.update(&self.terminal))?;
        let cols = backend(snapshot.cols())?;
        let rows = backend(snapshot.rows())?;
        let dirty = backend(snapshot.dirty())?;
        let shift = (!full
            && self.reusable_rows
            && self.pending_shift != 0
            && self.pending_shift.unsigned_abs() < u32::from(rows))
        .then_some(self.pending_shift);
        let render_all = full || (dirty == Dirty::Full && shift.is_none());
        let cursor = cursor_from_snapshot(&snapshot, self.last_cursor);
        let mut rows_changed = Vec::with_capacity(if render_all {
            usize::from(rows)
        } else {
            shift.map_or(0, |shift| shift.unsigned_abs() as usize)
        });
        self.row_wrapped.clear();
        let mut row_iteration = backend(self.row_iterator.update(&snapshot))?;
        let mut index = 0_u16;
        while let Some(row) = row_iteration.next() {
            let row_dirty = backend(row.dirty())?;
            let wrapped = backend(backend(row.raw_row())?.is_wrapped())?;
            let wrap_changed = usize::try_from(i32::from(index) + shift.unwrap_or(0))
                .ok()
                .and_then(|source| self.last_row_wrapped.get(source))
                != Some(&wrapped);
            self.row_wrapped.push(wrapped);
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
                    let mut cell_iteration = backend(self.cell_iterator.update(row))?;
                    while let Some(cell) = cell_iteration.next() {
                        cells.push(convert_cell(cell, &mut self.graphemes)?);
                    }
                }
                rows_changed.push(RowUpdate {
                    index,
                    cells,
                    wrapped,
                });
            }
            backend(row.set_dirty(false))?;
            index = index.saturating_add(1);
        }
        backend(snapshot.set_dirty(Dirty::Clean))?;

        self.cols = cols;
        self.rows = rows;
        std::mem::swap(&mut self.last_row_wrapped, &mut self.row_wrapped);
        self.last_cursor = cursor;
        let current_title = self.terminal.title().ok().filter(|title| !title.is_empty());
        let title_changed = current_title != self.last_frame_title.as_deref();
        if title_changed {
            self.last_frame_title = current_title.map(str::to_owned);
        }
        let title = if render_all || title_changed {
            self.last_frame_title.clone()
        } else {
            None
        };
        self.last_frame_history_epoch = self.history_epoch;
        self.reusable_rows = true;
        self.pending_shift = 0;
        self.snapshot_invalid = false;

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

    pub(crate) fn compression_pending(&self) -> bool {
        self.try_compression_pending().unwrap_or(true)
    }

    pub(crate) fn try_compression_pending(&self) -> Result<bool, EngineError> {
        backend(self.terminal.compression_activity())
            .map(|activity| self.compressed_activity != Some(activity))
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
        if scrollback_len < self.last_scrollback_len || oldest_rebased || tracker_missing {
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

impl VtEngine for GhosttyEngine {
    fn new(cols: u16, rows: u16, scrollback_bytes: usize) -> Result<Self, EngineError> {
        if cols == 0 || rows == 0 {
            return Err(EngineError::InvalidSize { cols, rows });
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = backend(Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: scrollback_bytes,
        }))?;
        // Fleet maps the platform Option/Alt modifier to terminal Alt. Match conventional
        // xterm behavior unless the child explicitly changes DEC mode 1036.
        backend(terminal.set_mode(Mode::ALT_ESC_PREFIX, true))?;
        let pty_events = Arc::clone(&events);
        backend(terminal.on_pty_write(move |_, bytes| {
            lock_events(&pty_events).push(EngineEvent::PtyWrite(bytes.to_vec()));
        }))?;
        backend(terminal.on_device_attributes(|_| {
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
        backend(terminal.on_xtversion(|_| Some(concat!("fleet ", env!("CARGO_PKG_VERSION")))))?;
        backend(terminal.on_size(|terminal| {
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
        backend(terminal.on_title_changed(move |terminal| {
            if let Ok(title) = terminal.title() {
                lock_events(&title_events).push(EngineEvent::Title(title.to_owned()));
            }
        }))?;
        let cwd_events = Arc::clone(&events);
        backend(terminal.on_pwd_changed(move |terminal| {
            if let Ok(cwd) = terminal.pwd() {
                lock_events(&cwd_events).push(EngineEvent::Cwd(cwd.to_owned()));
            }
        }))?;
        let bell_events = Arc::clone(&events);
        backend(terminal.on_bell(move |_| {
            lock_events(&bell_events).push(EngineEvent::Bell);
        }))?;
        let clipboard_events = Arc::clone(&events);
        backend(terminal.on_clipboard_write(move |_, write| {
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

        let render_state = backend(RenderState::new())?;
        let row_iterator = backend(RowIterator::new())?;
        let cell_iterator = backend(CellIterator::new())?;
        let key_encoder = backend(libghostty_vt::key::Encoder::new())?;
        let mut mouse_encoder = backend(MouseEncoder::new())?;
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
            row_wrapped: Vec::with_capacity(usize::from(rows)),
            graphemes: Vec::new(),
            history_epoch: 0,
            last_frame_history_epoch: 0,
            last_scrollback_len: 0,
            oldest_history: None,
            output_since_frame: false,
            snapshot_invalid: false,
            cols,
            rows,
            #[cfg(test)]
            fail_next: None,
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
        backend(self.terminal.resize(cols, rows, 1, 1))?;
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
        match <Self as VtEngine>::try_take_frame(self, full) {
            Ok(frame) => frame,
            Err(error) => {
                warn!(%error, "failed to snapshot Ghostty terminal");
                FrameUpdate {
                    terminal: TerminalId(0),
                    seq: 0,
                    cols: self.cols,
                    rows: self.rows,
                    full: true,
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

    fn try_take_frame(&mut self, full: bool) -> Result<FrameUpdate, EngineError> {
        let had_output = std::mem::take(&mut self.output_since_frame);
        self.update_history_epoch(had_output);
        #[cfg(test)]
        if self.fail_next == Some(TestFailure::Snapshot) {
            self.fail_next = None;
            self.snapshot_invalid = true;
            return Err(EngineError::Backend("injected snapshot failure".to_owned()));
        }
        match self.snapshot_frame(full) {
            Ok(frame) => Ok(frame),
            Err(error) => {
                self.snapshot_invalid = true;
                self.reusable_rows = false;
                self.pending_shift = 0;
                Err(error)
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
        match <Self as VtEngine>::try_wheel(self, event) {
            Ok(action) => action,
            Err(error) => {
                warn!(%error, "failed to encode Ghostty wheel event");
                WheelAction::Drop
            }
        }
    }

    fn try_wheel(&mut self, event: &WheelEvent) -> Result<WheelAction, EngineError> {
        let modes = self.modes();
        if !modes.alt_screen && !(event.mods.contains(Modifiers::SHIFT) && modes.mouse_reporting) {
            return Ok(WheelAction::Viewport(event.steps));
        }
        let steps = event
            .steps
            .unsigned_abs()
            .min(u32::from(self.rows) * 4)
            .min(1024);
        let bytes = if modes.mouse_reporting {
            self.try_encode_mouse(&MouseEvent {
                button: if event.steps < 0 {
                    MouseButton::WheelUp
                } else {
                    MouseButton::WheelDown
                },
                kind: MouseEventKind::Press,
                col: event.col.min(self.cols.saturating_sub(1)),
                row: event.row.min(self.rows.saturating_sub(1)),
                mods: event.mods,
            })?
        } else if modes.alt_screen && mode(&self.terminal, Mode::ALT_SCROLL) {
            self.try_encode_key(&KeyEvent {
                key: if event.steps < 0 { Key::Up } else { Key::Down },
                mods: Modifiers::empty(),
                text: None,
                action: KeyAction::Press,
            })?
        } else {
            return Ok(WheelAction::Drop);
        };
        Ok(WheelAction::Pty(bytes.repeat(steps as usize)))
    }

    fn compress_idle(&mut self) {
        if let Err(error) = <Self as VtEngine>::try_compress_idle(self) {
            warn!(%error, "failed to compress Ghostty history");
        }
    }

    fn try_compress_idle(&mut self) -> Result<(), EngineError> {
        let activity = backend(self.terminal.compression_activity())?;
        if self.compressed_activity == Some(activity) {
            return Ok(());
        }
        #[cfg(test)]
        if self.fail_next == Some(TestFailure::Compression) {
            self.fail_next = None;
            return Err(EngineError::Backend(
                "injected compression failure".to_owned(),
            ));
        }
        match backend(self.terminal.compress(CompressionMode::Incremental))? {
            CompressionResult::Pending => {}
            CompressionResult::Complete | CompressionResult::Unsupported => {
                self.compressed_activity = Some(activity);
            }
        }
        Ok(())
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
        match <Self as VtEngine>::try_encode_key(self, event) {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(%error, "failed to encode Ghostty key event");
                Vec::new()
            }
        }
    }

    fn try_encode_key(&mut self, event: &KeyEvent) -> Result<Vec<u8>, EngineError> {
        #[cfg(test)]
        if self.fail_next == Some(TestFailure::Key) {
            self.fail_next = None;
            return Err(EngineError::Backend("injected key failure".to_owned()));
        }
        let event = backend(ghostty_key_event(event))?;
        self.key_encoder
            .set_options_from_terminal(&self.terminal)
            // GPUI has already classified Option as terminal Alt. The Ghostty encoder resets
            // this process-local preference while importing terminal modes, so restore it.
            .set_macos_option_as_alt(libghostty_vt::key::OptionAsAlt::True);
        let mut output = Vec::new();
        backend(self.key_encoder.encode_to_vec(&event, &mut output))?;
        Ok(output)
    }

    fn encode_mouse(&mut self, event: &MouseEvent) -> Vec<u8> {
        match <Self as VtEngine>::try_encode_mouse(self, event) {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(%error, "failed to encode Ghostty mouse event");
                Vec::new()
            }
        }
    }

    fn try_encode_mouse(&mut self, event: &MouseEvent) -> Result<Vec<u8>, EngineError> {
        let wheel = matches!(
            event.button,
            MouseButton::WheelUp | MouseButton::WheelDown | MouseButton::Other(4 | 5)
        );
        #[cfg(test)]
        if self.fail_next == Some(TestFailure::Mouse) {
            self.fail_next = None;
            return Err(EngineError::Backend("injected mouse failure".to_owned()));
        }
        let event = backend(ghostty_mouse_event(event))?;
        let mut button_down = self.mouse_button_down;
        if !wheel {
            match event.action() {
                libghostty_vt::mouse::Action::Press => button_down = true,
                libghostty_vt::mouse::Action::Release => button_down = false,
                _ => {}
            }
        }
        self.mouse_encoder
            .set_options_from_terminal(&self.terminal)
            .set_any_button_pressed(button_down);
        let mut output = Vec::new();
        backend(self.mouse_encoder.encode_to_vec(&event, &mut output))?;
        self.mouse_button_down = button_down;
        Ok(output)
    }

    fn encode_paste(&self, text: &str) -> Vec<u8> {
        match <Self as VtEngine>::try_encode_paste(self, text) {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(%error, "failed to encode Ghostty paste");
                Vec::new()
            }
        }
    }

    fn try_encode_paste(&self, text: &str) -> Result<Vec<u8>, EngineError> {
        let mut input = text.as_bytes().to_vec();
        let mut output = vec![0_u8; input.len().saturating_add(12)];
        let written = backend(paste::encode(
            &mut input,
            self.modes().bracketed_paste,
            &mut output,
        ))?;
        output.truncate(written);
        Ok(output)
    }

    fn take_events(&mut self) -> Vec<EngineEvent> {
        std::mem::take(&mut *lock_events(&self.events))
    }
}

fn backend<T>(result: Result<T, libghostty_vt::Error>) -> Result<T, EngineError> {
    result.map_err(|error| EngineError::Backend(error.to_string()))
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
        _ => CursorShape::Block,
    }
}

fn convert_cell(
    cell: &libghostty_vt::render::CellIteration<'_, '_>,
    graphemes: &mut Vec<char>,
) -> Result<Cell, EngineError> {
    graphemes.resize(backend(cell.graphemes_len())?, '\0');
    backend(cell.graphemes_buf(graphemes))?;
    let style = backend(cell.style())?;
    let width = backend(cell.raw_cell()).and_then(|cell| backend(cell.wide()))?;
    Ok(Cell {
        text: graphemes.iter().copied().collect(),
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
        _ => attrs.insert(CellAttrs::UNDERLINE),
    }
    attrs
}

#[cfg(test)]
mod tests;
