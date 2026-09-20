use std::{collections::VecDeque, time::Instant};

/// Maximum number of snapshots retained in each undo or redo direction.
pub const HISTORY_CAP: usize = 100;

/// Maximum pause between adjacent typing inserts that still belong to one undo step.
pub const TYPING_GROUP_WINDOW: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) text: String,
    pub(super) caret: usize,
    pub(super) anchor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    Typing,
    Other,
}

#[derive(Clone, Debug, Default)]
pub(super) struct InputHistory {
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
    last_typing_at: Option<Instant>,
    explicit_group: bool,
    explicit_group_recorded: bool,
}

impl InputHistory {
    pub(super) fn record(&mut self, before: Snapshot, kind: EditKind, now: Instant) {
        let should_push = if self.explicit_group {
            !self.explicit_group_recorded
        } else {
            !matches!(kind, EditKind::Typing) || !self.typing_continues(now)
        };

        if should_push {
            Self::push_capped(&mut self.undo, before);
        }
        self.redo.clear();

        if self.explicit_group {
            self.explicit_group_recorded = true;
            self.last_typing_at = None;
        } else if matches!(kind, EditKind::Typing) {
            self.last_typing_at = Some(now);
        } else {
            self.last_typing_at = None;
        }
    }

    pub(super) fn break_typing_group(&mut self) {
        self.last_typing_at = None;
    }

    pub(super) fn begin_group(&mut self) {
        self.break_typing_group();
        self.explicit_group = true;
        self.explicit_group_recorded = false;
    }

    pub(super) fn end_group(&mut self) {
        self.explicit_group = false;
        self.explicit_group_recorded = false;
        self.break_typing_group();
    }

    pub(super) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.end_group();
    }

    pub(super) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(super) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub(super) fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.end_group();
        let snapshot = self.undo.pop_back()?;
        Self::push_capped(&mut self.redo, current);
        Some(snapshot)
    }

    pub(super) fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.end_group();
        let snapshot = self.redo.pop_back()?;
        Self::push_capped(&mut self.undo, current);
        Some(snapshot)
    }

    fn typing_continues(&self, now: Instant) -> bool {
        self.last_typing_at.is_some_and(|previous| {
            now.checked_duration_since(previous)
                .is_some_and(|elapsed| elapsed <= TYPING_GROUP_WINDOW)
        })
    }

    fn push_capped(stack: &mut VecDeque<Snapshot>, snapshot: Snapshot) {
        if stack.len() == HISTORY_CAP {
            stack.pop_front();
        }
        stack.push_back(snapshot);
    }
}
