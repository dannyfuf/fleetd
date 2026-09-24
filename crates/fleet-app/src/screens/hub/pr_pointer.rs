//! The pull requests screen's pointer: what a click on a tab or a row does (UX-SPEC §5.1).
//!
//! Each handler runs the same Hub state change its key does, so the pointer and the keyboard
//! cannot drift: a tab click is `Tab` / `h` / `l`, a row press is `j` / `k` landing on that row,
//! and a double-click is `⏎`.

use std::rc::Rc;

use super::*;

impl HubCtx {
    /// The handlers the PR screen hands its tabs and rows.
    pub(super) fn pr_handlers(&self) -> prs_screen::PrHandlers {
        let tab = self.clone();
        let select = self.clone();
        let open = self.clone();
        prs_screen::PrHandlers {
            select_tab: Rc::new(move |pr_tab, _, cx| {
                tab.focus_pr_list(cx);
                tab.switch_tab(pr_tab, cx);
            }),
            select_row: Rc::new(move |ix, _, cx| select.select_pr_row(ix, cx)),
            open_row: Rc::new(move |_, _, cx| open.open_pr(true, true, cx)),
        }
    }

    /// Puts the keyboard on the PR list, as clicking anywhere in it says the person means to.
    fn focus_pr_list(&self, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            if state.hub_pane != HubPane::List {
                state.hub_pane = HubPane::List;
                cx.notify();
            }
        });
    }

    /// A press on row `ix`: the cursor lands there exactly as a `j` / `k` would put it.
    fn select_pr_row(&self, ix: usize, cx: &mut App) {
        self.focus_pr_list(cx);
        let model = self.model(cx);
        let len = model.prs.len();
        if ix >= len {
            return;
        }
        let moving_down = ix >= self.cursor_index(cx);
        self.set_cursor(ix, len, moving_down, cx);
    }
}
