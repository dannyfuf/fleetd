use super::*;

/// The client-side mirror of one daemon-owned terminal.
///
/// The daemon sends a full frame on attach and dirty-row diffs afterwards; this applies both
/// and is the only thing the terminal element paints from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorGrid {
    /// Column count of the mirrored grid.
    pub cols: u16,
    /// Row count of the mirrored grid.
    pub rows: u16,
    /// One row of cells per grid row, always exactly `rows` long.
    pub lines: Vec<Vec<Cell>>,
    /// Soft-wrap continuation flag for each grid row.
    pub wrapped: Vec<bool>,
    /// The cursor as of the last applied frame.
    pub cursor: CursorState,
    /// The scrollback viewport as of the last applied frame.
    pub viewport: ViewportInfo,
    /// The terminal modes as of the last applied frame.
    pub modes: TerminalModes,
    /// The most recent PTY title, when one was reported.
    pub title: Option<String>,
    /// The sequence number of the last applied frame.
    pub seq: u64,
    /// Whether a full frame has been applied yet.
    pub primed: bool,
    /// Whether frames were dropped since the last full one.
    ///
    /// A diff only describes the rows that changed *since the previous frame*, so once one is
    /// missed the mirror can never catch up on its own: the rows changed inside the gap are
    /// never re-sent. While this is set the last good frame stays on screen — it is still the
    /// best answer available — and every diff is refused until a full frame re-primes it.
    pub desynced: bool,
    /// `Some(code)` once the PTY exited; the grid then freezes at its last frame.
    pub exit_code: Option<Option<i32>>,
}

impl MirrorGrid {
    /// An empty grid of the given size.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            lines: vec![Vec::new(); rows as usize],
            wrapped: vec![false; rows as usize],
            cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len: 0,
                offset: 0,
                history_epoch: 0,
            },
            modes: TerminalModes::default(),
            title: None,
            seq: 0,
            primed: false,
            desynced: false,
            exit_code: None,
        }
    }

    /// Applies a frame update, returning whether anything changed.
    ///
    /// A diff frame that arrives before the first full frame, out of sequence, or after a
    /// dropped one, is refused: only a full frame can re-prime the mirror.
    pub fn apply(&mut self, frame: &FrameUpdate) -> bool {
        if self.primed && frame.seq <= self.seq {
            return false;
        }
        if !frame.full && self.primed && frame.seq > self.seq.saturating_add(1) {
            self.desynced = true;
        }
        // A shift can reuse rows only within the same grid and history identity. Recover
        // through a full frame rather than rotating stale cells across an epoch boundary.
        if !frame.full
            && frame.shift.is_some()
            && (frame.viewport.history_epoch != self.viewport.history_epoch
                || frame.cols != self.cols
                || frame.rows != self.rows
                || frame.modes.alt_screen != self.modes.alt_screen)
        {
            self.desynced = true;
        }
        if !frame.full && (!self.primed || self.desynced) {
            return false;
        }
        if frame.full {
            self.primed = true;
            self.desynced = false;
        }
        if frame.cols != self.cols || frame.rows != self.rows || frame.full {
            self.resize(frame.cols, frame.rows);
        }
        if !frame.full
            && let Some(shift) = frame.shift
        {
            let count = (shift.unsigned_abs() as usize).min(self.lines.len());
            if shift > 0 {
                self.lines.rotate_left(count);
                self.wrapped.rotate_left(count);
                self.wrapped[self.lines.len() - count..].fill(false);
                let start = self.lines.len() - count;
                for row in &mut self.lines[start..] {
                    row.clear();
                }
            } else {
                self.lines.rotate_right(count);
                self.wrapped.rotate_right(count);
                self.wrapped[..count].fill(false);
                for row in &mut self.lines[..count] {
                    row.clear();
                }
            }
        }
        for row in &frame.rows_changed {
            if let Some(line) = self.lines.get_mut(row.index as usize) {
                line.clone_from(&row.cells);
            }
            if let Some(wrapped) = self.wrapped.get_mut(row.index as usize) {
                *wrapped = row.wrapped;
            }
        }
        self.cursor = frame.cursor;
        self.viewport = frame.viewport;
        self.modes = frame.modes;
        if let Some(title) = &frame.title {
            self.title = Some(title.clone());
        }
        self.seq = frame.seq;
        true
    }

    /// Resizes the mirror, keeping the rows that survive.
    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.lines.resize(rows as usize, Vec::new());
        self.wrapped.resize(rows as usize, false);
    }

    /// The plain text of one row, used by selection, search and tests.
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        self.lines
            .get(row as usize)
            .map_or_else(String::new, |line| {
                line.iter()
                    .filter(|cell| cell.width != CellWidth::Spacer)
                    .map(|cell| cell.text.as_str())
                    .collect()
            })
    }
}

impl AppState {
    /// The mirror grid of the popup agent's first terminal.
    #[must_use]
    pub fn agent_popup_grid(&self) -> Option<&MirrorGrid> {
        let terminal = self.agent_popup_session()?.terminals.first()?.id;
        self.grids.get(&terminal)
    }

    /// Applies one terminal frame to its mirror grid, creating the grid on the first frame.
    pub fn apply_frame(&mut self, frame: &FrameUpdate) -> bool {
        let grid = self
            .grids
            .entry(frame.terminal)
            .or_insert_with(|| MirrorGrid::new(frame.cols, frame.rows));
        grid.apply(frame)
    }

    /// Records that a terminal's PTY exited; the grid freezes at its last frame (§3.6).
    pub fn apply_terminal_exit(&mut self, terminal: TerminalId, code: Option<i32>) {
        if let Some(grid) = self.grids.get_mut(&terminal) {
            grid.exit_code = Some(code);
        }
    }

    /// Records a terminal title change.
    pub fn apply_terminal_title(&mut self, terminal: TerminalId, title: String) {
        if let Some(grid) = self.grids.get_mut(&terminal) {
            grid.title = Some(title);
        }
    }

    /// Records that the user named this terminal, so its name outranks the program's title.
    pub fn mark_renamed(&mut self, terminal: TerminalId) {
        self.renamed_terminals.insert(terminal);
    }

    /// The mirror grid of the active session's active terminal.
    #[must_use]
    pub fn active_grid(&self) -> Option<&MirrorGrid> {
        let terminal = self.active_session()?.active_terminal?;
        self.grids.get(&terminal)
    }

    /// Marks every mirror as having missed frames.
    ///
    /// The Shell re-primes them with `RequestFullFrame`: `fleet-app` mirrors raw
    /// `Event::TerminalFrame`s itself instead of going through `fleet-client`'s
    /// `TerminalHandle`, so it owns this recovery.
    pub fn desync_grids(&mut self) {
        for grid in self.grids.values_mut() {
            grid.desynced = true;
        }
    }
}

#[cfg(test)]
mod tests;
