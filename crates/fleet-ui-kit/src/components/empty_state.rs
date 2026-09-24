//! `EmptyState` — exactly two centered lines: the fact, then the key.
//!
//! §3.13: rendered inside the affected pane only, never full-screen, so the surrounding panes
//! stay usable. Copy is swarm's, verbatim; the component never invents wording.
//!
//! On a redesigned page the second line is a control instead of a key line:
//! [`EmptyState::button`] puts the page's one way forward (`New worktree  n`) under the fact,
//! so the key is taught where it is used (ADR 0023).

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme};

/// Two centered lines.
#[derive(IntoElement)]
pub struct EmptyState {
    fact: SharedString,
    action: Option<SharedString>,
    button: Option<AnyElement>,
}

impl EmptyState {
    /// The fact line, e.g. `No worktrees yet.`
    pub fn new(fact: impl Into<SharedString>) -> Self {
        Self {
            fact: fact.into(),
            action: None,
            button: None,
        }
    }

    /// The faint key line, e.g. `n  create one`.
    pub fn action(mut self, action: impl Into<SharedString>) -> Self {
        self.action = Some(action.into());
        self
    }

    /// A control under the fact, normally a [`super::Button`] showing its key. It takes the
    /// place of the key line; pass one or the other.
    pub fn button(mut self, button: impl IntoElement) -> Self {
        self.button = Some(button.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let gap = if self.button.is_some() {
            theme.space.md
        } else {
            theme.space.xs
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(gap)
            .child(Text::ui(self.fact).muted())
            .children(self.action.map(|action| Text::hint(action).faint()))
            .children(self.button)
    }
}
