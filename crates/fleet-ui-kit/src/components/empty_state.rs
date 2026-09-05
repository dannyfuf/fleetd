//! `EmptyState` — exactly two centered lines: the fact, then the key.
//!
//! §3.13: rendered inside the affected pane only, never full-screen, so the surrounding panes
//! stay usable. Copy is swarm's, verbatim; the component never invents wording.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme};

/// Two centered lines.
#[derive(IntoElement)]
pub struct EmptyState {
    fact: SharedString,
    action: Option<SharedString>,
}

impl EmptyState {
    /// The fact line, e.g. `No worktrees yet.`
    pub fn new(fact: impl Into<SharedString>) -> Self {
        Self {
            fact: fact.into(),
            action: None,
        }
    }

    /// The faint key line, e.g. `n  create one`.
    pub fn action(mut self, action: impl Into<SharedString>) -> Self {
        self.action = Some(action.into());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let gap = cx.theme().space.xs;
        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(gap)
            .child(Text::ui(self.fact).muted())
            .children(self.action.map(|action| Text::hint(action).faint()))
    }
}
