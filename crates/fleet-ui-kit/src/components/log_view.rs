//! `LogView` — the tail of a job log, with a follow toggle.
//!
//! §3.7: last 200 lines, mono 11.5, 16 ms batching, `f` toggles follow, `j`/`k` scroll,
//! `G` re-enables it. Batching and file tailing belong to the caller — the component renders
//! whatever lines it is given and states the cap it expects.
//!
//! **Follow is a mode, and a mode must be visible.** A log that silently stopped following is
//! indistinguishable from a job that stopped producing output, so the view always states which
//! of the two it is in the corner badge.

use gpui::{
    App, ElementId, FocusHandle, KeyDownEvent, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use std::rc::Rc;

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// How many lines the spec keeps in view.
pub const LOG_TAIL_LINES: usize = 200;

/// What the caller has to record after a key the view handled.
///
/// The view owns the scrolling (it holds the handle) but it does **not** own `following` or
/// the top line: those live in the caller's state, because the same `following` flag decides
/// whether the caller keeps appending lines at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogCommand {
    /// `f` — flip follow.
    ToggleFollow,
    /// `G` — jump to the tail and follow again.
    Follow,
    /// `j` / `k` — the user scrolled by hand to this top line, which always ends follow.
    ScrollTo(usize),
}

/// A scrolling log tail.
#[derive(IntoElement)]
pub struct LogView {
    id: ElementId,
    lines: Rc<Vec<SharedString>>,
    tones: Rc<Vec<Tone>>,
    following: bool,
    top: usize,
    scroll: Option<UniformListScrollHandle>,
    focus: Option<FocusHandle>,
    empty: SharedString,
    show_badge: bool,
    #[allow(clippy::type_complexity)]
    on_command: Option<Rc<dyn Fn(LogCommand, &mut Window, &mut App) + 'static>>,
}

impl LogView {
    /// A log view over already-tailed lines.
    pub fn new(id: impl Into<ElementId>, lines: impl IntoIterator<Item = SharedString>) -> Self {
        Self {
            id: id.into(),
            lines: Rc::new(lines.into_iter().collect()),
            tones: Rc::new(Vec::new()),
            following: true,
            top: 0,
            scroll: None,
            focus: None,
            empty: SharedString::new_static("No output yet."),
            show_badge: true,
            on_command: None,
        }
    }

    /// Optional per-line semantic tones; omitted entries use normal text contrast.
    pub fn line_tones(mut self, tones: impl IntoIterator<Item = Tone>) -> Self {
        self.tones = Rc::new(tones.into_iter().collect());
        self
    }

    /// Whether the view is pinned to the tail.
    pub fn following(mut self, following: bool) -> Self {
        self.following = following;
        self
    }

    /// The index of the first visible line, when the caller drives scrolling with `j`/`k`.
    ///
    /// gpui's uniform list only exposes its scroll position to tests, so the top line is state
    /// the caller holds — the same place `following` already lives. Ignored while following.
    pub fn top(mut self, top: usize) -> Self {
        self.top = top;
        self
    }

    /// Track scrolling, so `G` can jump back to the tail.
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// Handle `f` / `j` / `k` / `G` here instead of in the caller's key context.
    ///
    /// The view scrolls itself and reports the follow change through
    /// [`LogView::on_command`]; without a [`LogView::track_scroll`] handle `j`/`k` do nothing,
    /// because there is no scroll position to move.
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// What to do when a handled key changes the follow state.
    pub fn on_command(
        mut self,
        on_command: impl Fn(LogCommand, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_command = Some(Rc::new(on_command));
        self
    }

    /// The line shown when there is nothing to show.
    pub fn empty(mut self, empty: impl Into<SharedString>) -> Self {
        self.empty = empty.into();
        self
    }

    /// Hide the `FOLLOWING` / `PAUSED` corner badge.
    pub fn show_badge(mut self, show: bool) -> Self {
        self.show_badge = show;
        self
    }

    /// Whether the view is currently following.
    pub fn is_following(&self) -> bool {
        self.following
    }

    /// How many lines the view holds. Over [`LOG_TAIL_LINES`] the caller is keeping more than
    /// the spec asks for, which is allowed but is not what the 16 ms batch budget was sized
    /// against.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

impl RenderOnce for LogView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let pad = theme.space.md;
        let lines = self.lines.clone();
        let count = lines.len();
        let tones = self.tones.clone();
        let scroll = self.scroll.clone();
        let following = self.following;

        if count == 0 {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Text::data_small(self.empty).faint())
                .into_any_element();
        }

        // Following means "the newest line is the one on screen". Doing it on every render is
        // what makes a 16 ms-batched tail look like a live console rather than a list that
        // jumps once a second.
        if following && let Some(handle) = scroll.as_ref() {
            handle.scroll_to_item(count - 1, ScrollStrategy::Top);
        }

        let list = uniform_list(self.id.clone(), count, move |range, _window, _cx| {
            range
                .map(|ix| {
                    div().px(pad).whitespace_nowrap().child(
                        Text::data_small(lines[ix].clone())
                            .tone(tones.get(ix).copied().unwrap_or_default()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .size_full();
        let list = match scroll.as_ref() {
            Some(handle) => list.track_scroll(handle),
            None => list,
        };

        let badge = self.show_badge.then(|| {
            let (word, tone) = if following {
                ("following", Tone::Secondary)
            } else {
                ("paused", Tone::Warning)
            };
            div()
                .absolute()
                .bottom(theme.space.sm)
                .right(theme.space.sm)
                .px(theme.space.xs)
                .rounded(theme.radii.sm)
                .bg(theme.colors.elevated)
                .child(Text::label(word).tone(tone))
        });

        let keys = self.on_command.clone();
        let key_scroll = scroll.clone();
        let key_top = self.top;

        div()
            .relative()
            .size_full()
            .when_some(self.focus, |el, focus| {
                el.track_focus(&focus)
                    .on_key_down(move |event, window, cx| {
                        if let Some(command) =
                            handle_key(event, key_scroll.as_ref(), key_top, count)
                            && let Some(on_command) = keys.as_ref()
                        {
                            on_command(command, window, cx);
                        }
                    })
            })
            .child(list)
            .children(badge)
            .into_any_element()
    }
}

/// Apply a key to the scroll handle and report what the caller has to record.
///
/// `j`/`k` move by one line and always end follow: a user who scrolled by hand did not ask to
/// be dragged back to the tail by the next 16 ms batch.
fn handle_key(
    event: &KeyDownEvent,
    scroll: Option<&UniformListScrollHandle>,
    top: usize,
    count: usize,
) -> Option<LogCommand> {
    let modifiers = event.keystroke.modifiers;
    // A chord belongs to the app's key context (`^s J`, `cmd-q`); only bare keys are the log's.
    if modifiers.control || modifiers.platform || modifiers.alt {
        return None;
    }
    match event.keystroke.key.as_str() {
        "f" => Some(LogCommand::ToggleFollow),
        "g" if modifiers.shift => {
            if let Some(scroll) = scroll {
                scroll.scroll_to_bottom();
            }
            Some(LogCommand::Follow)
        }
        key @ ("j" | "k") => {
            let scroll = scroll?;
            let next = if key == "j" {
                (top + 1).min(count.saturating_sub(1))
            } else {
                top.saturating_sub(1)
            };
            scroll.scroll_to_item_strict(next, ScrollStrategy::Top);
            Some(LogCommand::ScrollTo(next))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_tail_is_two_hundred_lines() {
        assert_eq!(LOG_TAIL_LINES, 200);
    }

    #[test]
    fn a_fresh_view_follows() {
        let view = LogView::new("log", [SharedString::new_static("one")]);
        assert!(view.is_following());
        assert_eq!(view.line_count(), 1);
        assert!(!view.following(false).is_following());
    }
}
