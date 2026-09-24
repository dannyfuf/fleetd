//! What the repository sidebar's pointer does (UX-SPEC §3.2, §5.1).
//!
//! Every handler ends in code a key already runs: a repository click is `⏎` on that row, and the
//! edge only stores a width.

use fleet_ui_kit::ListPointer;

use super::*;

impl HubCtx {
    /// The sidebar's pointer contract for this frame.
    pub(super) fn sidebar_handlers(&self) -> repos_rail::SidebarHandlers {
        let (select, open) = (self.clone(), self.clone());
        let repos = ListPointer::new()
            .on_select(move |ix, _, cx| select.click_rail(ix, cx))
            .on_open(move |_, _, cx| open.open_repo(cx))
            // The right-click has already selected; the item's `ContextMenu` opens the menu.
            .on_menu(|_, _, _, _| {});
        let resize = self.state.clone();
        repos_rail::SidebarHandlers {
            repos,
            resize: Rc::new(move |width, _, cx| {
                resize.update(cx, |state, cx| {
                    if state.sidebar_w != Some(width) {
                        state.sidebar_w = Some(width);
                        cx.notify();
                    }
                });
            }),
        }
    }

    /// Put the rail's cursor on row `ix` and give the rail the keyboard: what every press on a
    /// repository row does first.
    pub(super) fn select_rail(&self, ix: usize, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            if matches!(state.screen, Screen::Hub { .. }) && state.hub_pane != HubPane::Repos {
                state.hub_pane = HubPane::Repos;
                cx.notify();
            }
        });
        let model = self.model(cx);
        let len = model.rail.len();
        if ix >= len {
            return;
        }
        let moving_down = ix >= self.state.read(cx).cursors.repos;
        self.set_cursor(ix, len, moving_down, cx);
    }

    /// A press on a repository selects it: the cursor goes there, the rail keeps the keyboard,
    /// and the list is scoped to it — so the row's menu, opened by the same press on `⋯` or by
    /// a right-click, shows and runs the rail's own keys. `⏎` or a double-click then hands the
    /// list the keyboard. A failed clone has no worktrees to scope to; its press only moves the
    /// cursor, and opening it (`⏎`, a double-click) shows its job.
    fn click_rail(&self, ix: usize, cx: &mut App) {
        self.select_rail(ix, cx);
        let Some(row) = self.selected_rail_row(cx) else {
            return;
        };
        let scope = match (row.kind, row.repo) {
            (RailKind::CloneFailed, _) => return,
            (RailKind::All, _) | (_, None) => RepoScope::All,
            (_, Some(repo)) => RepoScope::Repo(repo),
        };
        if self.state.read(cx).scope == scope {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.scope = scope;
            state.cursors.worktrees = 0;
            cx.notify();
        });
        self.hub.update(cx, |hub, _| hub.selection.worktree = None);
        self.schedule_inspection(cx);
    }
}
