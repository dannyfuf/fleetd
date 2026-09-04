//! `LogView` — the tail of a job log, with a follow toggle.
//!
//! §3.7: last 200 lines, mono 11.5, 16 ms batching, `f` toggles follow, `G` re-enables it.
//! Batching and file tailing belong to the caller; the component renders whatever lines it is
//! given and states the cap it expects.

use gpui::{
    App, ElementId, SharedString, UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use std::rc::Rc;

use crate::{text::Text, theme::ActiveTheme};

/// How many lines the spec keeps in view.
pub const LOG_TAIL_LINES: usize = 200;

/// A scrolling log tail.
#[derive(IntoElement)]
pub struct LogView {
    id: ElementId,
    lines: Rc<Vec<SharedString>>,
    following: bool,
    scroll: Option<UniformListScrollHandle>,
}

impl LogView {
    /// A log view over already-tailed lines.
    pub fn new(id: impl Into<ElementId>, lines: impl IntoIterator<Item = SharedString>) -> Self {
        Self {
            id: id.into(),
            lines: Rc::new(lines.into_iter().collect()),
            following: true,
            scroll: None,
        }
    }

    /// Whether the view is pinned to the tail.
    pub fn following(mut self, following: bool) -> Self {
        self.following = following;
        self
    }

    /// Track scrolling, so `G` can jump back to the tail.
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// Whether the view is currently following.
    pub fn is_following(&self) -> bool {
        self.following
    }
}

impl RenderOnce for LogView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pad = cx.theme().space.md;
        let lines = self.lines.clone();
        let count = lines.len();
        if count == 0 {
            return div().size_full().into_any_element();
        }
        let list = uniform_list(self.id, count, move |range, _window, _cx| {
            range
                .map(|ix| {
                    div()
                        .px(pad)
                        .child(Text::data_small(lines[ix].clone()))
                })
                .collect::<Vec<_>>()
        })
        .size_full();

        match self.scroll {
            Some(handle) => list.track_scroll(&handle).into_any_element(),
            None => list.into_any_element(),
        }
    }
}
