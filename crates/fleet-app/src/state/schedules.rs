//! The schedules mirror: each board's schedules as the daemon last listed them (BOARD §12).
//!
//! One entry per board the app has asked about. The Board settings dialog's Schedules section
//! and the board header's schedules strip both read it; neither asks the daemon from `render`.
//! A `SchedulesChanged { board_id }` event marks that board's entry stale, and the event loop
//! re-asks for every stale entry once per batch.

use std::collections::HashMap;

use fleet_core::{ids::BoardId, schedule::Schedule};

/// One board's schedules and the state of the request that fills them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardSchedules {
    /// The schedules of the last successful `ListSchedules { board_id }`, in daemon order.
    pub schedules: Vec<Schedule>,
    /// Whether a `ListSchedules` for this board is in flight.
    pub loading: bool,
    /// Whether a `SchedulesChanged` arrived since the last load, so the list must be re-read.
    pub stale: bool,
    /// The last load failure, kept until the next successful load.
    pub error: Option<String>,
}

/// Every board's schedules the app has asked for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchedulesMirror {
    /// Keyed by board; a board never asked about is absent.
    by_board: HashMap<BoardId, BoardSchedules>,
    /// Bumped by every change to any entry, so projections that read the mirror (the board
    /// header's strip) can memoise on one integer.
    pub revision: u64,
    /// The board the header last asked about on its own, so a failed load is retried once per
    /// activation of a board rather than on every notify of the observation that calls
    /// [`SchedulesMirror::wants_header_load`].
    activated: Option<BoardId>,
}

impl SchedulesMirror {
    /// The board's schedules, empty when none were loaded.
    #[must_use]
    pub fn for_board(&self, board: &BoardId) -> &[Schedule] {
        self.by_board
            .get(board)
            .map_or(&[], |entry| entry.schedules.as_slice())
    }

    /// The board's whole entry, when the app has asked about it.
    #[must_use]
    pub fn entry(&self, board: &BoardId) -> Option<&BoardSchedules> {
        self.by_board.get(board)
    }

    /// Whether the shown board's header should ask for its schedules now, recording `board` as
    /// the board shown.
    ///
    /// A board never asked about is always wanted. A board whose last load failed is wanted
    /// only when it was not already the board shown: the caller runs from an observation of
    /// `AppState`, and a failure answers with a notify, so retrying there would ask the daemon
    /// again on every round trip for as long as the failure lasted.
    pub fn wants_header_load(&mut self, board: &BoardId) -> bool {
        let activation = self.activated.as_ref() != Some(board);
        if activation {
            self.activated = Some(board.clone());
        }
        match self.by_board.get(board) {
            None => true,
            Some(entry) => activation && entry.error.is_some(),
        }
    }

    /// Marks a load as issued. Returns `false` when one is already in flight, so the caller
    /// sends nothing.
    pub fn begin_load(&mut self, board: &BoardId) -> bool {
        let entry = self.by_board.entry(board.clone()).or_default();
        if entry.loading {
            return false;
        }
        entry.loading = true;
        entry.stale = false;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Adopts the daemon's answer for one board.
    pub fn apply(&mut self, board: &BoardId, schedules: Vec<Schedule>) {
        let entry = self.by_board.entry(board.clone()).or_default();
        entry.schedules = schedules;
        entry.loading = false;
        entry.error = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Records a failed load and releases the in-flight guard so it can be asked again.
    pub fn load_failed(&mut self, board: &BoardId, message: String) {
        let entry = self.by_board.entry(board.clone()).or_default();
        entry.loading = false;
        entry.error = Some(message);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Marks a board's entry stale after `SchedulesChanged`. A board the app never asked about
    /// is ignored; returns whether anything was marked.
    pub fn mark_stale(&mut self, board: &BoardId) -> bool {
        let Some(entry) = self.by_board.get_mut(board) else {
            return false;
        };
        entry.stale = true;
        true
    }

    /// The boards whose entries must be re-read, in no particular order.
    #[must_use]
    pub fn stale_boards(&self) -> Vec<BoardId> {
        self.by_board
            .iter()
            .filter(|(_, entry)| entry.stale && !entry.loading)
            .map(|(board, _)| board.clone())
            .collect()
    }

    /// Forgets everything, for a new connection: another daemon may hold other schedules.
    pub fn clear(&mut self) {
        self.activated = None;
        if !self.by_board.is_empty() {
            self.by_board.clear();
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests;
