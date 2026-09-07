use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

type Lines = Arc<[gpui::SharedString]>;

pub(super) struct ConflictView {
    file: Arc<fleet_git::ConflictFile>,
    section: usize,
    lines: Option<(Lines, Lines)>,
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for ConflictView {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Lazygit {
    pub(super) fn prepare_conflict(&mut self, cx: &mut Context<Self>) {
        let MainContent::Conflict {
            file: Some(file),
            section,
            ..
        } = &self.state.main
        else {
            self.conflict_view = None;
            return;
        };
        if !self.active
            || crate::panels::side_ratio(self.state.screen_mode, self.state.focused) == 1.0
        {
            self.conflict_view = None;
            return;
        }
        let section = (*section).min(file.conflicts.len().saturating_sub(1));
        if self
            .conflict_view
            .as_ref()
            .is_some_and(|view| Arc::ptr_eq(&view.file, file) && view.section == section)
        {
            return;
        }
        self.conflict_view = None;
        let source = file.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let task = cx.spawn(async move |this, cx| {
            let work_cancel = cancellation.clone();
            let lines = cx
                .background_spawn(async move {
                    let conflict = source.conflicts.get(section)?;
                    let prepare = |bytes: &[u8]| -> Option<Lines> {
                        let text = String::from_utf8_lossy(bytes);
                        let mut lines = Vec::new();
                        for line in text.lines() {
                            if work_cancel.load(Ordering::Relaxed) {
                                return None;
                            }
                            lines.push(gpui::SharedString::new(line));
                        }
                        Some(lines.into())
                    };
                    Some((prepare(&conflict.ours)?, prepare(&conflict.theirs)?))
                })
                .await;
            if cancellation.load(Ordering::Relaxed) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = &mut this.conflict_view {
                    view.lines = lines;
                    cx.notify();
                }
            });
        });
        self.conflict_view = Some(ConflictView {
            file: file.clone(),
            section,
            lines: None,
            cancelled,
            _task: task,
        });
    }

    pub(crate) fn conflict_lines(
        &self,
        file: &Arc<fleet_git::ConflictFile>,
        section: usize,
    ) -> Option<(Lines, Lines)> {
        self.conflict_view
            .as_ref()
            .filter(|view| Arc::ptr_eq(&view.file, file) && view.section == section)?
            .lines
            .clone()
    }
}
