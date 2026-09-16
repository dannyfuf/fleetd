use super::*;

impl WorkspaceScreen {
    /// Applies a pure-text bridge batch only to the active native-agent view.
    ///
    /// `AppState` already owns the authoritative projection. Passing this borrow and the
    /// reducer descriptions keeps the shell, chrome, inactive tabs, and grouping projection out
    /// of the streaming path.
    pub(crate) fn sync_agent_text(
        &self,
        state: &Entity<AppState>,
        applied: &HashMap<ThreadId, Vec<Applied>>,
        cx: &mut App,
    ) {
        state.update(cx, |app, cx| {
            let Some(thread) = app.active_agent_thread() else {
                return;
            };
            let (Some(changes), Some(projection), Some(view)) = (
                applied.get(&thread),
                app.agents.projection(thread),
                self.agent_view(thread),
            ) else {
                return;
            };
            view.update(cx, |view, cx| view.sync_batch(projection, changes, cx));
        });
    }
}
