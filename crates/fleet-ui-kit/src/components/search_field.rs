//! `SearchField` — a dialog header's search box: a magnifier, the live query, and at its end
//! either the key that focuses it or the match count and a clear `✕`.
//!
//! ## Purpose
//!
//! Settings searches every section from its header (`/`). The field is always live: the query
//! is a [`TextInput`] the caller owns and builds embedded ([`TextInput::set_embedded`]) with its
//! placeholder (`Search settings`), so the field is the chrome and the caller's subscription to
//! the editor is the whole search. Use a [`super::FilterField`] instead for a page toolbar whose
//! filter is a mode the list enters, and a [`super::FilterBar`] for a dense pane header.
//!
//! ## Anatomy
//!
//! ```text
//! [ ⌕  Search settings                     / ]   empty: the key chip
//! [ ⌕  refresh|                   3/41   ✕ ]   a query: the count, then the clear button
//! ```
//!
//! `button_h` tall, `control` fill inside a `control_border` hairline, `radii.control` corners,
//! `sm` before the glyph and `xs` after the button, `sm` between parts. The glyph is a small,
//! muted `search`; the editor takes the rest (`flex_1 min_w_0`).
//!
//! ## API
//!
//! [`SearchField::new`] takes the id and the editor. [`SearchField::kbd`] is the chip drawn
//! while the query is empty, [`SearchField::count`] the `shown/total` caption drawn while it is
//! not, [`SearchField::on_clear`] runs after the `✕` empties the query, [`SearchField::width`]
//! overrides `filter_field_w`, and [`SearchField::harness_clear`] names the `✕` for the harness.
//!
//! ## States
//!
//! empty (placeholder, the chip) · a query (count, `✕`) · a query matching nothing (the count
//! turns amber: the rows did not vanish, the query hides them) · focused (the hairline is
//! `focus_ring`) · unfocused.
//!
//! ## Pointer and keyboard (ADR 0023)
//!
//! A press anywhere on the field focuses the editor, the pointer twin of the key its chip
//! names. The `✕` empties the editor — the edit `ctrl-u` makes, so the owner hears it as an
//! ordinary change — and is drawn only while there is something to clear. The field decodes no
//! keys itself; the editor's key table does.
//!
//! ## Usage rule
//!
//! One per dialog, in the `Dialog`'s header actions. The owner binds the key that focuses it
//! and passes the same key to [`SearchField::kbd`], so the chip and the binding cannot drift.

use gpui::{App, ElementId, Entity, MouseButton, Pixels, Window, div, prelude::*};

use super::{ButtonSize, IconButton, TextInput, kbd::Kbd};
use crate::{
    harness::HarnessTargetExt as _,
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
    tone::Tone,
};

type ClearHandler = dyn Fn(&mut Window, &mut App);

/// What the field draws after the query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchEnd {
    /// An empty query with no key to name: nothing.
    Nothing,
    /// An empty query: the chip of the key that focuses the field.
    Kbd,
    /// A query: the clear button, after the `shown/total` count when there is one.
    Clear {
        /// Whether the count is drawn before the button.
        count: bool,
    },
}

/// A live search box for a dialog header. See the module doc.
#[derive(IntoElement)]
pub struct SearchField {
    id: ElementId,
    input: Entity<TextInput>,
    kbd: Option<Kbd>,
    count: Option<(usize, usize)>,
    on_clear: Option<Box<ClearHandler>>,
    width: Option<Pixels>,
    harness_clear: Option<&'static str>,
}

impl SearchField {
    /// A search field over `input`. The caller builds the editor embedded and sets its
    /// placeholder.
    pub fn new(id: impl Into<ElementId>, input: Entity<TextInput>) -> Self {
        Self {
            id: id.into(),
            input,
            kbd: None,
            count: None,
            on_clear: None,
            width: None,
            harness_clear: None,
        }
    }

    /// The key that focuses the field, drawn at its end while the query is empty. Pass the chip
    /// from the key table the owner binds, so the two cannot drift.
    pub fn kbd(mut self, kbd: impl Into<Option<Kbd>>) -> Self {
        self.kbd = kbd.into();
        self
    }

    /// `shown/total`: how many of the searched items the query keeps, drawn while it is
    /// non-empty.
    pub fn count(mut self, shown: usize, total: usize) -> Self {
        self.count = Some((shown, total));
        self
    }

    /// Run `handler` after the `✕` has emptied the query. The field clears the editor itself;
    /// this is for anything else the owner does on a clear.
    pub fn on_clear(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_clear = Some(Box::new(handler));
        self
    }

    /// Override the `filter_field_w` width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Name the `✕` for the harness target recorder.
    pub fn harness_clear(mut self, name: &'static str) -> Self {
        self.harness_clear = Some(name);
        self
    }

    /// Whether the query is empty.
    pub fn is_empty(&self, cx: &App) -> bool {
        self.input.read(cx).is_empty()
    }

    /// What the field draws after the query, given whether the query is empty.
    pub fn end(&self, empty: bool) -> SearchEnd {
        match (empty, self.kbd.is_some()) {
            (true, true) => SearchEnd::Kbd,
            (true, false) => SearchEnd::Nothing,
            (false, _) => SearchEnd::Clear {
                count: self.count.is_some(),
            },
        }
    }

    /// The tone of the count: amber when the query hides every item.
    pub fn count_tone(&self) -> Tone {
        match self.count {
            Some((0, total)) if total > 0 => Tone::Warning,
            _ => Tone::Muted,
        }
    }
}

impl RenderOnce for SearchField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (empty, focused) = {
            let input = self.input.read(cx);
            (input.is_empty(), input.focus_handle().is_focused(window))
        };
        let end = self.end(empty);
        let count_tone = self.count_tone();
        let chip = self.kbd.filter(|_| end == SearchEnd::Kbd);
        let count = self
            .count
            .filter(|_| matches!(end, SearchEnd::Clear { count: true }))
            .map(|(shown, total)| {
                Text::caption(format!("{shown}/{total}"))
                    .tone(count_tone)
                    .flex_none()
            });
        let clear = matches!(end, SearchEnd::Clear { .. }).then(|| {
            let input = self.input.clone();
            let on_clear = self.on_clear;
            IconButton::new("search-field-clear", Icon::X, "Clear search")
                .size(ButtonSize::Compact)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    input.update(cx, |input, cx| input.set_text("", cx));
                    if let Some(on_clear) = &on_clear {
                        on_clear(window, cx);
                    }
                })
                .harness_target_named(self.harness_clear)
        });

        let theme = cx.theme();
        let focus_input = self.input.clone();
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.sm)
            .w(self.width.unwrap_or(theme.metrics.filter_field_w))
            .h(theme.metrics.button_h)
            .pl(theme.space.sm)
            .pr(theme.space.xs)
            .rounded(theme.radii.control)
            .bg(theme.colors.control)
            .border(theme.metrics.hairline)
            .border_color(if focused {
                theme.colors.focus_ring
            } else {
                theme.colors.control_border
            })
            .cursor_text()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus_input.update(cx, |input, cx| input.focus(window, cx));
            })
            .child(
                Icon::Search
                    .el()
                    .size(IconSize::Small)
                    .color(theme.colors.text_muted),
            )
            .child(
                styled_with(div(), TextRole::Ui.style(theme), theme)
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h(theme.text.ui.line_height)
                    .text_color(theme.colors.text)
                    .child(self.input),
            )
            .children(chip)
            .children(count)
            .children(clear)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext, point, px};

    use super::*;
    use crate::components::InputMode;

    fn input(cx: &mut TestAppContext, text: &str) -> Entity<TextInput> {
        cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_embedded(true, cx);
            input.set_text(text, cx);
            input
        })
    }

    fn slash() -> Kbd {
        Kbd::parse("/").unwrap_or_else(|error| panic!("{error}"))
    }

    #[gpui::test]
    fn an_empty_query_shows_the_key_and_a_query_shows_the_count_and_the_clear(
        cx: &mut TestAppContext,
    ) {
        let field = SearchField::new("s", input(cx, ""))
            .kbd(slash())
            .count(3, 41);
        assert_eq!(field.end(true), SearchEnd::Kbd);
        assert_eq!(field.end(false), SearchEnd::Clear { count: true });

        let bare = SearchField::new("s", input(cx, ""));
        assert_eq!(bare.end(true), SearchEnd::Nothing, "no key, no chip");
        assert_eq!(bare.end(false), SearchEnd::Clear { count: false });
        let unbound = SearchField::new("s", input(cx, "")).kbd(None);
        assert_eq!(unbound.end(true), SearchEnd::Nothing);
    }

    #[gpui::test]
    fn emptiness_is_the_editors(cx: &mut TestAppContext) {
        let empty = SearchField::new("s", input(cx, ""));
        let full = SearchField::new("s", input(cx, "refresh"));
        cx.update(|cx| {
            assert!(empty.is_empty(cx));
            assert!(!full.is_empty(cx));
        });
    }

    #[gpui::test]
    fn a_query_that_matches_nothing_turns_the_count_amber(cx: &mut TestAppContext) {
        let tone = |cx: &mut TestAppContext, shown, total| {
            SearchField::new("s", input(cx, "x"))
                .count(shown, total)
                .count_tone()
        };
        assert_eq!(tone(cx, 0, 41), Tone::Warning);
        assert_eq!(tone(cx, 3, 41), Tone::Muted);
        assert_eq!(
            tone(cx, 0, 0),
            Tone::Muted,
            "nothing to search is not a miss"
        );
    }

    struct Host {
        input: Entity<TextInput>,
        cleared: usize,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            crate::harness::begin_frame(window);
            let host = cx.entity().downgrade();
            div().size_full().child(
                SearchField::new("search", self.input.clone())
                    .kbd(slash())
                    .count(1, 4)
                    .harness_clear("test.search.clear")
                    .on_clear(move |_, cx| {
                        if let Some(host) = host.upgrade() {
                            host.update(cx, |host, _| host.cleared += 1);
                        }
                    }),
            )
        }
    }

    #[gpui::test]
    fn the_clear_button_empties_the_query_runs_on_clear_and_then_hides(cx: &mut TestAppContext) {
        crate::harness::set_recording(true);
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let query = input(cx, "refresh");
        let editor = query.clone();
        let window = cx.add_window(|_, _| Host {
            input: editor,
            cleared: 0,
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let names = |visual: &mut VisualTestContext| -> Vec<String> {
            visual
                .update(|window, _| crate::harness::painted(window))
                .into_iter()
                .map(|target| target.name.to_string())
                .collect()
        };
        let clear = visual
            .update(|window, _| crate::harness::painted(window))
            .into_iter()
            .find(|target| target.name == "test.search.clear")
            .unwrap_or_else(|| panic!("a query shows its clear button"))
            .rect;
        let at = point(px(clear.x + clear.w / 2.0), px(clear.y + clear.h / 2.0));
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();

        query.read_with(&visual, |input, _| assert_eq!(input.text(), ""));
        let host = window.root(&mut visual).expect("test host");
        host.read_with(&visual, |host, _| {
            assert_eq!(host.cleared, 1, "on_clear runs once, after the clear")
        });

        visual.update(|window, cx| window.draw(cx).clear(cx));
        let painted = names(&mut visual);
        crate::harness::set_recording(false);
        assert!(
            !painted.iter().any(|name| name == "test.search.clear"),
            "an empty query has nothing to clear"
        );
    }
}
