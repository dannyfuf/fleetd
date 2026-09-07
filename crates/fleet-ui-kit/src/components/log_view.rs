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
/// The view owns the scrolling (it always holds a handle) but it does **not** own `following` or
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
    lines: std::sync::Arc<[SharedString]>,
    tones: std::sync::Arc<[Tone]>,
    following: bool,
    top: usize,
    scroll: UniformListScrollHandle,
    focus: Option<FocusHandle>,
    empty: SharedString,
    show_badge: bool,
    #[allow(clippy::type_complexity)]
    on_command: Option<Rc<dyn Fn(LogCommand, &mut Window, &mut App) + 'static>>,
}

impl LogView {
    /// A log view over immutable storage. An `Arc<[SharedString]>` is taken as is, without
    /// collecting or cloning every line.
    pub fn from_shared(
        id: impl Into<ElementId>,
        lines: impl Into<std::sync::Arc<[SharedString]>>,
    ) -> Self {
        Self {
            id: id.into(),
            lines: lines.into(),
            tones: std::sync::Arc::default(),
            following: true,
            top: 0,
            scroll: UniformListScrollHandle::new(),
            focus: None,
            empty: SharedString::new_static("No output yet."),
            show_badge: true,
            on_command: None,
        }
    }

    /// Reuse immutable per-line tones alongside shared log storage.
    pub fn shared_line_tones(mut self, tones: std::sync::Arc<[Tone]>) -> Self {
        self.tones = tones;
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

    /// Reuse a persistent caller-owned scroll handle across renders.
    ///
    /// Without this override the view still has an internal handle and can truthfully reach the
    /// tail; interactive callers should provide their persistent handle so wheel position is
    /// retained and reported through [`LogView::on_command`].
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll = handle.clone();
        self
    }

    /// Handle `f` / `j` / `k` / `G` here instead of in the caller's key context.
    ///
    /// The view scrolls itself and reports the follow change through
    /// [`LogView::on_command`].
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
        let lines = self.lines;
        let count = lines.len();
        let tones = self.tones;
        let scroll = self.scroll;
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
        if following {
            follow_tail(&scroll);
        }

        let list = uniform_list(self.id, count, move |range, _window, _cx| {
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
        let list = list.track_scroll(&scroll);

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

        let keys = self.on_command;
        let wheel_keys = keys.clone();
        let wheel_scroll = scroll.clone();
        let key_scroll = scroll;
        let key_top = self.top;

        div()
            .relative()
            .size_full()
            .on_scroll_wheel(move |_, window, cx| {
                let Some(on_command) = wheel_keys.as_ref() else {
                    return;
                };
                if let Some(command) = wheel_command(&wheel_scroll, count) {
                    on_command(command, window, cx);
                    cx.stop_propagation();
                }
            })
            .when_some(self.focus, |el, focus| {
                el.track_focus(&focus)
                    .on_key_down(move |event, window, cx| {
                        let Some(on_command) = keys.as_ref() else {
                            return;
                        };
                        if let Some(command) = handle_key(event, &key_scroll, key_top, count) {
                            on_command(command, window, cx);
                            cx.stop_propagation();
                        }
                    })
            })
            .child(list)
            .children(badge)
            .into_any_element()
    }
}

fn follow_tail(scroll: &UniformListScrollHandle) {
    scroll.scroll_to_bottom();
}

fn wheel_command(scroll: &UniformListScrollHandle, count: usize) -> Option<LogCommand> {
    matches!(scroll.is_scrolled_to_end(), Some(false))
        .then(|| LogCommand::ScrollTo(top_index(scroll, count)))
}

/// Index of the topmost visible line, honoring a pending request for a real line.
fn top_index(scroll: &UniformListScrollHandle, count: usize) -> usize {
    let state = scroll.0.borrow();
    let top = state
        .deferred_scroll_to_item
        .as_ref()
        .filter(|deferred| {
            deferred.strategy != ScrollStrategy::Bottom && deferred.item_index < count
        })
        .map(|deferred| deferred.item_index)
        .unwrap_or_else(|| state.base_handle.logical_scroll_top().0);
    top.min(count.saturating_sub(1))
}

/// Apply a key to the scroll handle and report what the caller has to record.
///
/// `j`/`k` move by one line and always end follow: a user who scrolled by hand did not ask to
/// be dragged back to the tail by the next 16 ms batch.
fn handle_key(
    event: &KeyDownEvent,
    scroll: &UniformListScrollHandle,
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
            scroll.scroll_to_bottom();
            Some(LogCommand::Follow)
        }
        key @ ("j" | "k") => {
            let next = if key == "j" {
                top.saturating_add(1).min(count.saturating_sub(1))
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

    struct TestLog {
        focus: FocusHandle,
        scroll: UniformListScrollHandle,
        following: bool,
        top: usize,
        commands: Vec<LogCommand>,
        ancestor_dispatches: usize,
    }

    impl TestLog {
        fn new(cx: &mut gpui::Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                following: true,
                top: 99,
                commands: Vec::new(),
                ancestor_dispatches: 0,
            }
        }
    }

    impl Render for TestLog {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let command_target = cx.weak_entity();
            let ancestor_target = cx.weak_entity();
            div()
                .size_full()
                .on_key_down(move |_, _, cx| {
                    let _ = ancestor_target.update(cx, |view, _| view.ancestor_dispatches += 1);
                })
                .child(
                    LogView::from_shared(
                        "test-log",
                        (0..100)
                            .map(|index| SharedString::from(format!("line {index}")))
                            .collect::<Vec<_>>(),
                    )
                    .following(self.following)
                    .top(self.top)
                    .track_scroll(&self.scroll)
                    .focus(&self.focus)
                    .on_command(move |command, _, cx| {
                        let _ = command_target.update(cx, |view, cx| {
                            view.commands.push(command);
                            match command {
                                LogCommand::ToggleFollow => view.following = !view.following,
                                LogCommand::Follow => {
                                    view.following = true;
                                    view.top = 99;
                                }
                                LogCommand::ScrollTo(top) => {
                                    view.following = false;
                                    view.top = top;
                                }
                            }
                            cx.notify();
                        });
                    }),
                )
        }
    }

    fn test_log_window(
        cx: &mut gpui::TestAppContext,
    ) -> (
        gpui::WindowHandle<TestLog>,
        gpui::VisualTestContext,
        gpui::Entity<TestLog>,
    ) {
        use gpui::AppContext;
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(TestLog::new))
                .unwrap_or_else(|error| panic!("test window: {error}"))
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        let view = window
            .root(&mut visual)
            .unwrap_or_else(|error| panic!("test log: {error}"));
        visual.update(|window, cx| {
            view.update(cx, |view, cx| window.focus(&view.focus, cx));
        });
        visual.run_until_parked();
        (window, visual, view)
    }

    #[test]
    fn shared_log_construction_retains_lines_and_tones() {
        let lines: std::sync::Arc<[SharedString]> = vec!["first".into(), "second".into()].into();
        let tones: std::sync::Arc<[Tone]> = vec![Tone::Warning].into();
        let view = LogView::from_shared("log", lines.clone()).shared_line_tones(tones.clone());
        assert!(std::sync::Arc::ptr_eq(&view.lines, &lines));
        assert!(std::sync::Arc::ptr_eq(&view.tones, &tones));
        assert_eq!(view.line_count(), 2);
    }

    #[test]
    fn a_fresh_view_follows() {
        let view = LogView::from_shared("log", [SharedString::new_static("one")]);
        assert!(view.following);
        assert_eq!(view.line_count(), 1);
        assert!(!view.following(false).following);
    }

    #[test]
    fn following_always_reaches_the_tail() {
        let view = LogView::from_shared("log", [SharedString::new_static("one")]);
        follow_tail(&view.scroll);
        let state = view.scroll.0.borrow();
        let deferred = state
            .deferred_scroll_to_item
            .unwrap_or_else(|| panic!("tail scroll was requested"));
        assert_eq!(deferred.item_index, usize::MAX);
        assert_eq!(deferred.strategy, ScrollStrategy::Bottom);
    }

    #[test]
    fn pending_bottom_is_not_reported_as_a_line_index() {
        let view = LogView::from_shared("log", [SharedString::new_static("one")]);
        follow_tail(&view.scroll);
        assert_eq!(top_index(&view.scroll, view.line_count()), 0);
    }

    #[test]
    fn down_navigation_saturates_an_invalid_top() {
        let scroll = UniformListScrollHandle::new();
        let event = KeyDownEvent {
            keystroke: gpui::Keystroke::parse("j")
                .unwrap_or_else(|error| panic!("test key: {error}")),
            is_held: false,
            prefer_character_input: false,
        };

        assert_eq!(
            handle_key(&event, &scroll, usize::MAX, 3),
            Some(LogCommand::ScrollTo(2))
        );
    }

    #[gpui::test]
    fn wheel_pause_is_not_undone_by_rerender(cx: &mut gpui::TestAppContext) {
        use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
        let (_window, mut visual, view) = test_log_window(cx);
        visual.simulate_event(ScrollWheelEvent {
            position: point(px(20.0), px(20.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
            ..Default::default()
        });
        visual.run_until_parked();

        view.read_with(&visual, |view, _| {
            assert!(!view.following);
            assert!(matches!(
                view.commands.last(),
                Some(LogCommand::ScrollTo(_))
            ));
        });
    }

    #[gpui::test]
    fn handled_navigation_stops_ancestor_dispatch(cx: &mut gpui::TestAppContext) {
        let (_window, mut visual, view) = test_log_window(cx);
        visual.simulate_keystrokes("j");

        view.read_with(&visual, |view, _| {
            assert_eq!(view.commands.len(), 1);
            assert!(matches!(view.commands[0], LogCommand::ScrollTo(_)));
            assert_eq!(view.ancestor_dispatches, 0);
        });
    }
}
