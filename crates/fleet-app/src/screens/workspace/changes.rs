//! The Changes panel's lifecycle: the git worker behind it, and the verbs its controls run.
//!
//! The panel reads git on this machine, through [`ChangesWorker`], exactly as the embedded
//! Lazygit pane does: one worker while the panel is open on a local worktree, dropped — which
//! ends its thread — the moment the panel closes or the Workspace moves on. Readings land in
//! [`crate::state::ChangesPanel`], already turned into rows; render only lays them out.

use std::path::PathBuf;

use fleet_lazygit::changes::{ChangesEvent, ChangesWorker};

use super::*;
use crate::state::ReadingBody;

/// The worker reading one worktree, and the foreground loop that applies what it reports.
pub(super) struct ChangesSource {
    worktree: WorktreeId,
    base: String,
    worker: ChangesWorker,
    /// Drains the worker's events into `AppState`; dropped with the worker.
    _drain: Task<()>,
}

impl WorkspaceScreen {
    /// Starts, keeps or drops the Changes worker so it reads exactly the open panel's worktree.
    ///
    /// Runs on the update path, after [`WorkspaceScreen::inspect_git`]: a panel opened by the
    /// frame that also first inspects the worktree reads once, from its own worker's start.
    pub(super) fn sync_changes(&self, model: &Model, state: &Entity<AppState>, cx: &mut App) {
        let Some(wanted) = model.changes.as_ref() else {
            self.local.borrow_mut().state.changes = None;
            if state.read(cx).changes.reading().is_some() {
                state.update(cx, |app, _| app.changes.begin(None));
            }
            return;
        };
        let current = self
            .local
            .borrow()
            .state
            .changes
            .as_ref()
            .is_some_and(|source| source.worktree == wanted.worktree && source.base == wanted.base);
        let remote_shown = wanted.path.is_none()
            && state
                .read(cx)
                .changes
                .reading_for(&wanted.worktree)
                .is_some_and(|reading| reading.body == ReadingBody::Remote);
        if current || remote_shown {
            return;
        }
        let Some(path) = wanted.path.clone() else {
            // §12: a remote worktree's path names a directory on the other machine.
            self.local.borrow_mut().state.changes = None;
            let begin = (
                wanted.worktree.clone(),
                wanted.base.clone(),
                ReadingBody::Remote,
            );
            state.update(cx, |app, _| app.changes.begin(Some(begin)));
            return;
        };

        let worker = ChangesWorker::start(path, wanted.base.clone());
        let events = worker.events();
        let weak = state.downgrade();
        let worktree = wanted.worktree.clone();
        let drain = cx.spawn(async move |cx| {
            while let Ok(event) = events.recv().await {
                let applied = weak.update(cx, |app, cx| {
                    let changed = match event {
                        ChangesEvent::Loaded(result) => app.changes.apply(&worktree, result),
                        ChangesEvent::FileDiff { path, result } => {
                            app.changes.apply_diff(&worktree, &path, result)
                        }
                    };
                    if changed {
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    return;
                }
            }
        });
        let begin = (
            wanted.worktree.clone(),
            wanted.base.clone(),
            ReadingBody::Loading,
        );
        state.update(cx, |app, _| app.changes.begin(Some(begin)));
        self.local.borrow_mut().state.changes = Some(ChangesSource {
            worktree: wanted.worktree.clone(),
            base: wanted.base.clone(),
            worker,
            _drain: drain,
        });
    }
}

impl WorkspaceScreen {
    /// The panel beside the body, when this worktree has it open and the pane is not zoomed.
    pub(super) fn changes_panel(
        &self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        cx: &App,
    ) -> Option<AnyElement> {
        let wanted = model.changes.as_ref().filter(|_| !model.zoomed)?;
        let reading = state
            .read(cx)
            .changes
            .reading_for(&wanted.worktree)?
            .clone();
        let rows = match &reading.body {
            ReadingBody::Ready(model) => Some(Rc::clone(model)),
            _ => None,
        };
        let on_file = {
            let (local, state) = (Rc::clone(&self.local), state.clone());
            Rc::new(move |ix: usize, _: &mut Window, cx: &mut App| {
                if let Some(file) = rows.as_ref().and_then(|rows| rows.files.get(ix)) {
                    open_diff(&local, &state, file.path.clone(), cx);
                }
            })
        };
        let on_lazygit = {
            let (local, bridge, state) = self.handles(bridge, state);
            Rc::new(move |_: &mut Window, cx: &mut App| {
                open_lazygit_tab(&local, &bridge, &state, cx);
            })
        };
        let local = self.local.borrow();
        Some(crate::views::changes_panel::render(
            crate::views::changes_panel::ChangesProps {
                reading: &reading,
                scroll: &local.state.changes_scroll,
                on_file,
                on_lazygit,
            },
            cx,
        ))
    }
}

/// Asks the open panel's worker to read again, on the Workspace's git-status cadence.
pub(super) fn refresh(local: &Rc<RefCell<Local>>, worktree: &WorktreeId) {
    if let Some(source) = local
        .borrow()
        .state
        .changes
        .as_ref()
        .filter(|source| &source.worktree == worktree)
    {
        source.worker.refresh();
    }
}

/// `^s g` and the tab strip's Changes button: shows or hides the panel for this worktree.
pub(super) fn toggle(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.leave_prefix();
        let worktree = app
            .active_session()
            .and_then(|session| match &session.kind {
                SessionKind::Worktree(worktree) => Some(worktree.clone()),
                SessionKind::Agent { .. } => None,
            });
        match worktree {
            Some(worktree) => {
                app.changes.toggle(&worktree);
            }
            None => app.toast_short(
                "Changes are shown for worktree sessions",
                Icon::Info,
                Instant::now(),
            ),
        }
        cx.notify();
    });
}

/// A click on a file: its diff against the base, in the Changes diff sheet.
pub(super) fn open_diff(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    path: PathBuf,
    cx: &mut App,
) {
    let local = local.borrow();
    let Some(source) = local.state.changes.as_ref() else {
        return;
    };
    source.worker.file_diff(path.clone());
    let (worktree, base) = (source.worktree.clone(), source.base.clone());
    drop(local);
    state.update(cx, |app, cx| {
        app.changes.begin_diff(worktree, path, base.into());
        app.open_overlay(Overlay::Dialog(Dialogs::ChangesDiff));
        cx.notify();
    });
}
