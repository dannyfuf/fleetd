//! What the sidebar's pointer does (UX-SPEC §3.2, §5.1), and the Agents rows it shows.
//!
//! Every handler ends in code a key already runs: a repository click is `⏎` on that row, an
//! agent click is the palette's `go`, and the edge only stores a width.

use fleet_core::config::Agent;
use fleet_ui_kit::ListPointer;

use super::*;
use crate::{actions::fleet, views::repos_rail::AgentTarget};

impl HubCtx {
    /// The sidebar's pointer contract for this frame.
    pub(super) fn sidebar_handlers(&self, bridge: &Bridge) -> repos_rail::SidebarHandlers {
        let (select, open) = (self.clone(), self.clone());
        let repos = ListPointer::new()
            .on_select(move |ix, _, cx| select.click_rail(ix, cx))
            .on_open(move |_, _, cx| open.open_repo(cx))
            // The right-click has already selected; the item's `ContextMenu` opens the menu.
            .on_menu(|_, _, _, _| {});
        let (go, bridge) = (self.clone(), bridge.clone());
        let agents = ListPointer::new()
            .on_select(move |ix, window, cx| go.go_to_agent(ix, &bridge, window, cx));
        let resize = self.state.clone();
        repos_rail::SidebarHandlers {
            repos,
            agents,
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

    /// A click on agent row `ix`: open its thread the way the palette's `go` does, or show its
    /// agent window the way `a` / `A` does — never hiding one already on screen, which the key
    /// would.
    fn go_to_agent(&self, ix: usize, bridge: &Bridge, window: &mut Window, cx: &mut App) {
        let Some(target) = self.hub.read(cx).agents.get(ix).map(|row| row.target) else {
            return;
        };
        match target {
            AgentTarget::Thread(thread) => {
                dialogs::open_agent_thread(thread, &self.state, bridge, cx)
            }
            AgentTarget::Window(agent) => {
                let showing = self
                    .state
                    .read(cx)
                    .agent_popup
                    .as_ref()
                    .is_some_and(|popup| popup.agent == agent && popup.worktree.is_none());
                if showing {
                    return;
                }
                match agent {
                    Agent::Claude => window.dispatch_action(Box::new(fleet::OpenAgentClaude), cx),
                    Agent::Codex => window.dispatch_action(Box::new(fleet::OpenAgentCodex), cx),
                    Agent::Opencode => {}
                }
            }
        }
    }

    /// Rebuilds the Agents rows from the agent mirror and the snapshot, and keeps the previous
    /// ones when nothing changed. Returns whether the rows changed.
    pub(super) fn refresh_agents(&self, cx: &mut App) -> bool {
        let rows = {
            let state = self.state.read(cx);
            let context = effective_context(state).map(|context| &context.id);
            repos_rail::agent_rows(state, context)
        };
        self.hub.update(cx, |hub, _| {
            if *hub.agents == *rows {
                return false;
            }
            hub.agents = rows.into();
            true
        })
    }
}
