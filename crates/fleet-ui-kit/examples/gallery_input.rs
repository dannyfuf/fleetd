//! The visual and behavioural test bench for the **input** group of `fleet-ui-kit`.
//!
//! `TextInput` · `FuzzyList` · `FilterBar` · `Cycler` · `Switch` · `Checkbox` · `SettingsRow` ·
//! `SettingsCard` · `ValueBox` · `SearchField` · `Breadcrumb` · `SegmentedControl` ·
//! `SegmentedTabs` · `Select` · `Callout` · `ConfirmDialog` · `Palette`.
//!
//! The settings grammar (`SettingsRow`, `SettingsCard`, `ValueBox`, `SearchField`,
//! `Breadcrumb`, the inline `Cycler`) lives in `gallery_input/settings.rs` and ends with the
//! acceptance panel: the Settings · Agents pane drawn as its artboard draws it.
//!
//! Every component appears in every state it can be in, in both themes, and the interactive
//! ones are *live*: the inputs really edit, the palette really filters and highlights, the
//! lists really move. If a state is not visible or not operable here, it is not implemented.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_input
//! ```
//!
//! | Key | What |
//! | --- | --- |
//! | `ctrl-t` | toggle light / dark |
//! | `ctrl-i` | focus the branch field · `esc` leaves it |
//! | `/` | focus the filter bar · `esc` leaves it, `esc` again clears it |
//! | `:` | open the palette · `esc` closes it |
//! | `d` / `D` | the compact / expanded confirm · `y` `Y` `n` `esc` answer it, `I` re-checks, and every button clicks |
//! | `ctrl-n` `ctrl-p` `↓` `↑` | move the fuzzy / palette cursor (also while typing) |
//! | `h` `l` | previous / next tab |
//! | `←` `→` | cycle the host value |
//! | `space` | flip the settings switches |
//! | `o` | open / close the select |
//! | `+` `-` | step the `Grace` box by 500 ms (past its clamp the rule replaces the helper) |
//! | `ctrl-e` | open the editing row's box (the focus ring) · `esc` leaves it |
//! | `ctrl-f` | focus the live search field · `esc` leaves it |
//! | `ctrl-q` / `cmd-q` | quit |
//!
//! Bare-letter keys are bound only in the `GalleryNormal` context. While a field owns the
//! keyboard the root switches to `GalleryTyping`, so `h`, `y`, `o` and friends are typed
//! instead of fired — the same rule §3.10 states for the real Hub.

pub mod support;
// The example root resolves modules beside it, so name the settings panels' file explicitly.
#[path = "gallery_input/settings.rs"]
mod settings;
const LAYOUT: support::layout::GalleryLayout = support::layout::GalleryLayout {
    label_width: 170.0,
    column: true,
    divided: false,
    compact: true,
};
use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EntityInputHandler, FocusHandle, Focusable, KeyBinding,
    ScrollHandle, SharedString, Subscription, WeakEntity, Window, actions, div, px,
};

actions!(
    gallery_input,
    [
        ToggleTheme,
        Quit,
        FocusEditor,
        Escape,
        Accept,
        FocusFilter,
        OpenPalette,
        ConfirmCompact,
        ConfirmExpanded,
        ConfirmYes,
        ConfirmStrong,
        ConfirmNo,
        ConfirmRecheck,
        CursorNext,
        CursorPrev,
        NextTab,
        PrevTab,
        CycleNext,
        CyclePrev,
        ToggleCheck,
        ToggleSelect,
        Increment,
        Decrement,
        FocusValueBox,
        FocusSearch,
    ]
);

/// Which confirm the overlay is showing, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmDemo {
    None,
    Compact,
    Expanded,
}

/// Which surface currently owns the keyboard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Capture {
    /// Nothing is typing: bare letters are actions.
    None,
    /// The filter bar input.
    Filter,
    /// The palette query.
    Palette,
}

/// The candidate set the fuzzy list and the filter bar rank over.
const BRANCHES: &[(&str, &str)] = &[
    ("origin/main", "default"),
    ("origin/release-2026", ""),
    ("origin/feat/payroll-import", "ahead 3"),
    ("origin/fix-rut-validator", ""),
    ("pull/412/head", "previous base"),
    ("origin/chore/deps", ""),
];

/// The palette's `GO` objects, `DO` commands and `CONTEXT` switches.
const GO_ROWS: &[(&str, &str)] = &[
    ("payroll#feat-payroll-fix", "session attached"),
    ("payroll#fix-rut-validator", "sleeping"),
    ("#412 Fix RUT validation on import", "PR \u{b7} mine"),
    ("buk/payroll", "repo"),
];
const DO_ROWS: &[(&str, &str, bool)] = &[
    ("Prune worktrees \u{b7} buk/payroll", "x", true),
    ("Cancel job: clone nixos", "J", false),
    ("Clone repo", "n", false),
    ("Sleep this worktree", "s", false),
];
const CONTEXT_PREFIX: &str = "Switch to context: ";
const CONTEXT_ROWS: &[(&str, &str)] = &[("personal", "2"), ("buk", "1")];

/// A forgiving subsequence match: returns the matched character indices, or `None`.
fn subsequence(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    if needle.is_empty() {
        return Some(Vec::new());
    }
    let lowered: Vec<char> = haystack.to_lowercase().chars().collect();
    let mut hits = Vec::new();
    let mut cursor = 0usize;
    for wanted in needle.to_lowercase().chars() {
        if wanted.is_whitespace() {
            continue;
        }
        let found = lowered[cursor..].iter().position(|ch| *ch == wanted)?;
        hits.push(cursor + found);
        cursor += found + 1;
    }
    Some(hits)
}

/// The exact branch-name rule a value breaks, if any (§3.8.1: fail before a job starts).
fn branch_error(value: &str) -> Option<SharedString> {
    if value.is_empty() {
        return None;
    }
    if value.contains("..") {
        return Some("branch cannot contain \"..\"".into());
    }
    if value.starts_with('/') || value.ends_with('/') {
        return Some("branch cannot start or end with \"/\"".into());
    }
    if value.contains(' ') {
        return Some("branch cannot contain a space".into());
    }
    None
}

struct InputGallery {
    focus_handle: FocusHandle,
    /// The branch field whose preview line turns into a validation line in the same slot.
    branch: Entity<TextInput>,
    live_empty: Entity<TextInput>,
    live_filled: Entity<TextInput>,
    live_focused: Entity<TextInput>,
    live_selection: Entity<TextInput>,
    live_marked: Entity<TextInput>,
    live_invalid: Entity<TextInput>,
    live_read_only: Entity<TextInput>,
    live_numeric: Entity<TextInput>,
    /// The editor a number `ValueBox` draws in place of its value while it is being typed into.
    number_row_editor: Entity<TextInput>,
    /// The editor the editing `SettingsRow` hands its `ValueBox` (`ctrl-e`).
    value_row_editor: Entity<TextInput>,
    /// The editor the tall row's multi-line `ValueBox` grows to eight rows with.
    multiline_row_editor: Entity<TextInput>,
    /// The editor of the stand-alone multi-line `ValueBox` panel.
    multiline_box_editor: Entity<TextInput>,
    /// An editor handed to a box but not focused: a hooks row at rest.
    hooks_row_editor: Entity<TextInput>,
    /// The `SearchField` queries: empty, live (`ctrl-f`), and one matching nothing.
    search_empty: Entity<TextInput>,
    search_live: Entity<TextInput>,
    search_no_match: Entity<TextInput>,
    /// The Agents pane's cursor row, and the values its controls choose.
    settings_cursor: usize,
    agent: usize,
    access: usize,
    model: usize,
    effort: usize,
    live_labeled: Entity<TextInput>,
    live_multiline_min: Entity<TextInput>,
    live_multiline_grown: Entity<TextInput>,
    filter: Entity<TextInput>,
    filter_no_match: Entity<TextInput>,
    palette_query: Entity<TextInput>,
    /// Re-ranking on every keystroke: a list that re-ranked must not keep a stale cursor.
    _query_subscriptions: Vec<Subscription>,
    capture: Capture,
    filter_focused: bool,
    fuzzy_cursor: usize,
    /// The long list's cursor and scroll position: a click moves the one and reveals it in
    /// the other.
    long_cursor: usize,
    long_scroll: ScrollHandle,
    palette_cursor: usize,
    palette_open: bool,
    tab: usize,
    host: usize,
    step: usize,
    checked: bool,
    grace: i64,
    select_open: bool,
    confirm: ConfirmDemo,
    answer: Option<SharedString>,
}

const HOSTS: &[&str] = &["local", "devbox", "ci-runner"];
/// A five-step set: one more than a segmented control holds.
const STEPS: &[&str] = &["1 min", "5 min", "10 min", "30 min", "1 h"];

impl InputGallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let branch = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_label(Some("branch".into()), cx);
            input.set_placeholder("feat/rut-validator", cx);
            input.set_icon(Some(Icon::GitBranchPlus), cx);
            input.set_mono(true, cx);
            input.set_text("feat/rut-validator", cx);
            input
        });
        let live_empty = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_placeholder("empty with placeholder", cx);
            input
        });
        let live_filled = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("feat/worktree-boards", cx);
            input
        });
        let live_focused = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("caret lives here", cx);
            input.move_to_end(cx);
            input
        });
        let live_selection = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("selected value", cx);
            input.select_all(cx);
            input
        });
        let live_marked = cx.new(|cx| TextInput::new(InputMode::SingleLine, cx));
        live_marked.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "IME marked text", None, window, cx);
        });
        let live_invalid = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("feat/../boards", cx);
            input.set_invalid(Some("branch cannot contain \"..\"".into()), cx);
            input
        });
        let live_read_only = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("generated by config", cx);
            input.set_read_only(true, cx);
            input
        });
        let live_numeric = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("2000", cx);
            input.set_filter(Some(|character| character.is_ascii_digit()), cx);
            input
        });
        let number_row_editor = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_mono(true, cx);
            input.set_hide_status_line(true, cx);
            input.set_filter(Some(|character| character.is_ascii_digit()), cx);
            // A settings row draws the box; the editor is only the line and its caret.
            input.set_embedded(true, cx);
            input.set_text("2500", cx);
            input
        });
        let value_row_editor = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_mono(true, cx);
            input.set_hide_status_line(true, cx);
            input.set_embedded(true, cx);
            input.set_text("claude-opus-5", cx);
            input
        });
        let multiline_editor = |text: &str, mono: bool, cx: &mut Context<TextInput>| {
            let mut input = TextInput::new(
                InputMode::Multiline {
                    min_rows: 3,
                    max_rows: VALUE_BOX_MAX_ROWS,
                },
                cx,
            );
            input.set_mono(mono, cx);
            input.set_hide_status_line(true, cx);
            input.set_embedded(true, cx);
            input.set_text(text, cx);
            input
        };
        let multiline_row_editor = cx.new(|cx| {
            multiline_editor(
                "Review pull request {pr_url} for correctness and test coverage. Leave one \
                 summary comment; do not push.",
                false,
                cx,
            )
        });
        let multiline_box_editor =
            cx.new(|cx| multiline_editor("PORT=3001\nRUST_LOG=info", true, cx));
        let hooks_row_editor = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_mono(true, cx);
            input.set_hide_status_line(true, cx);
            input.set_embedded(true, cx);
            input.set_placeholder("Type the next command\u{2026}", cx);
            input
        });
        let search_editor = |text: &str, cx: &mut Context<TextInput>| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            // The field is the chrome; the editor is only the line and its caret.
            input.set_embedded(true, cx);
            input.set_placeholder("Search settings", cx);
            input.set_text(text, cx);
            input
        };
        let search_empty = cx.new(|cx| search_editor("", cx));
        let search_live = cx.new(|cx| search_editor("refresh", cx));
        let search_no_match = cx.new(|cx| search_editor("zzz", cx));
        let live_labeled = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_label(Some("branch".into()), cx);
            input.set_icon(Some(Icon::GitBranchPlus), cx);
            input.set_mono(true, cx);
            input.set_text("feat/worktree-boards", cx);
            input
        });
        let live_multiline_min = cx.new(|cx| {
            let mut input = TextInput::new(
                InputMode::Multiline {
                    min_rows: 3,
                    max_rows: 6,
                },
                cx,
            );
            input.set_text(
                "A focused description can stay one logical paragraph while it wraps naturally across the editor's available width.",
                cx,
            );
            input
        });
        let live_multiline_grown = cx.new(|cx| {
            let mut input = TextInput::new(
                InputMode::Multiline {
                    min_rows: 3,
                    max_rows: 6,
                },
                cx,
            );
            input.set_text(
                "The first paragraph is deliberately long enough to wrap without a hard break, so selection geometry crosses a soft boundary.\nA second explicit line also wraps inside the same box and pushes the editor past its visual-row cap.\nThird line.\nFourth line.",
                cx,
            );
            input.select_all(cx);
            input
        });
        let filter = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            // The bar lives in a 30 px pane header, so the editor brings no box of its own.
            input.set_embedded(true, cx);
            input.set_placeholder("filter branches", cx);
            input.set_text("rut", cx);
            input
        });
        let filter_no_match = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_embedded(true, cx);
            input.set_text("zzz", cx);
            input
        });
        let palette_query = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_embedded(true, cx);
            input.set_placeholder("go to, or do", cx);
            input.set_text("pay", cx);
            input
        });
        // A re-ranked list must never leave the cursor past its end, and the branch field's
        // preview and validation line are derived from what was just typed into it.
        let query_subscriptions = vec![
            cx.subscribe(&branch, |gallery, _, event, cx| {
                if *event == TextInputEvent::Changed {
                    gallery.refresh_branch_status(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&filter, |gallery, _, event, cx| {
                if *event == TextInputEvent::Changed {
                    gallery.fuzzy_cursor = 0;
                    cx.notify();
                }
            }),
            cx.subscribe(&palette_query, |gallery, _, event, cx| {
                if *event == TextInputEvent::Changed {
                    gallery.palette_cursor = 0;
                    cx.notify();
                }
            }),
            // The live search's count is derived from its query.
            cx.subscribe(&search_live, |_, _, event, cx| {
                if *event == TextInputEvent::Changed {
                    cx.notify();
                }
            }),
        ];
        let mut gallery = Self {
            focus_handle: cx.focus_handle(),
            branch,
            live_empty,
            live_filled,
            live_focused,
            live_selection,
            live_marked,
            live_invalid,
            live_read_only,
            live_numeric,
            live_labeled,
            live_multiline_min,
            live_multiline_grown,
            number_row_editor,
            value_row_editor,
            multiline_row_editor,
            multiline_box_editor,
            hooks_row_editor,
            search_empty,
            search_live,
            search_no_match,
            settings_cursor: 4,
            agent: 0,
            access: 0,
            model: 1,
            effort: 0,
            filter,
            filter_no_match,
            palette_query,
            _query_subscriptions: query_subscriptions,
            capture: Capture::None,
            filter_focused: true,
            fuzzy_cursor: 0,
            long_cursor: 0,
            long_scroll: ScrollHandle::new(),
            palette_cursor: 0,
            palette_open: false,
            tab: 0,
            host: 0,
            step: 2,
            checked: true,
            grace: 2_000,
            select_open: true,
            confirm: ConfirmDemo::None,
            answer: None,
        };
        gallery.refresh_branch_status(cx);
        gallery
    }

    /// Keep the preview and the validation line in step with the value (§3.8.1).
    fn refresh_branch_status(&mut self, cx: &mut Context<Self>) {
        let value = self.branch.read(cx).text().to_string();
        let error = branch_error(&value);
        let preview = (!value.is_empty() && error.is_none())
            .then(|| SharedString::from(format!("\u{2192} buk/payroll#{value}")));
        self.branch.update(cx, |input, cx| {
            input.set_invalid(error, cx);
            input.set_preview(preview, cx);
        });
    }

    fn live_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.branch.read(cx).focus_handle().is_focused(window)
            || self.live_empty.read(cx).focus_handle().is_focused(window)
            || self.live_filled.read(cx).focus_handle().is_focused(window)
            || self.live_focused.read(cx).focus_handle().is_focused(window)
            || self
                .live_selection
                .read(cx)
                .focus_handle()
                .is_focused(window)
            || self.live_marked.read(cx).focus_handle().is_focused(window)
            || self.live_invalid.read(cx).focus_handle().is_focused(window)
            || self
                .live_read_only
                .read(cx)
                .focus_handle()
                .is_focused(window)
            || self.live_numeric.read(cx).focus_handle().is_focused(window)
            || self.live_labeled.read(cx).focus_handle().is_focused(window)
            || self
                .live_multiline_min
                .read(cx)
                .focus_handle()
                .is_focused(window)
            || self
                .live_multiline_grown
                .read(cx)
                .focus_handle()
                .is_focused(window)
            || [
                &self.number_row_editor,
                &self.value_row_editor,
                &self.multiline_row_editor,
                &self.multiline_box_editor,
                &self.hooks_row_editor,
                &self.search_empty,
                &self.search_live,
                &self.search_no_match,
            ]
            .into_iter()
            .any(|editor| editor.read(cx).focus_handle().is_focused(window))
    }

    /// The rows the fuzzy list shows for the current filter query.
    fn ranked_branches(&self, cx: &App) -> Vec<(&'static str, &'static str, Vec<usize>)> {
        let query = self.filter.read(cx).text().to_owned();
        BRANCHES
            .iter()
            .filter_map(|(name, detail)| {
                subsequence(name, &query).map(|hits| (*name, *detail, hits))
            })
            .collect()
    }

    /// The palette's three sections for the current query, already ranked.
    fn palette_sections(&self, cx: &App) -> (Vec<PaletteSection>, usize, usize) {
        let query = self.palette_query.read(cx).text();
        let go: Vec<PaletteRow> = GO_ROWS
            .iter()
            .filter_map(|(label, detail)| {
                subsequence(label, query).map(|hits| {
                    PaletteRow::new(*label).detail(*detail).matches(hits).icon(
                        if detail.starts_with("PR") {
                            Icon::GitPullRequest
                        } else if *detail == "repo" {
                            Icon::FolderGit2
                        } else if *detail == "sleeping" {
                            Icon::Moon
                        } else {
                            Icon::CircleDot
                        },
                    )
                })
            })
            .collect();
        let do_rows: Vec<PaletteRow> = DO_ROWS
            .iter()
            .filter_map(|(label, key, destructive)| {
                subsequence(label, query).map(|hits| {
                    PaletteRow::new(*label)
                        .kbd(chip(key))
                        .when(*destructive, |row| row.detail("asks first"))
                        .matches(hits)
                        .destructive(*destructive)
                        .icon(if *destructive {
                            Icon::Scissors
                        } else {
                            Icon::Command
                        })
                })
            })
            .collect();
        let contexts: Vec<PaletteRow> = CONTEXT_ROWS
            .iter()
            .filter_map(|(label, digit)| {
                subsequence(label, query).map(|hits| {
                    PaletteRow::new(format!("{CONTEXT_PREFIX}{label}"))
                        .kbd(chip(digit))
                        .matches(
                            hits.into_iter()
                                .map(|ix| ix + CONTEXT_PREFIX.chars().count()),
                        )
                        .icon(Icon::Boxes)
                })
            })
            .collect();

        let matched = go.len() + do_rows.len() + contexts.len();
        let total = GO_ROWS.len() + DO_ROWS.len() + CONTEXT_ROWS.len();
        let sections = vec![
            PaletteSection::new("Go to", go),
            PaletteSection::new("Commands", do_rows),
            PaletteSection::new("Contexts", contexts),
        ];
        (sections, matched, total)
    }

    /// How many palette rows the current query matches, without building any of them.
    fn palette_matches(&self, cx: &App) -> usize {
        let query = self.palette_query.read(cx).text();
        GO_ROWS
            .iter()
            .map(|(label, _)| *label)
            .chain(DO_ROWS.iter().map(|(label, _, _)| *label))
            .chain(CONTEXT_ROWS.iter().map(|(label, _)| *label))
            .filter(|label| subsequence(label, query).is_some())
            .count()
    }

    /// How many rows the cursor may land on right now.
    fn cursor_len(&self, cx: &App) -> usize {
        if self.palette_open {
            self.palette_matches(cx)
        } else {
            self.ranked_branches(cx).len()
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn focus_editor(&mut self, _: &FocusEditor, window: &mut Window, cx: &mut Context<Self>) {
        self.capture = Capture::None;
        self.branch.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn focus_value_box(&mut self, _: &FocusValueBox, window: &mut Window, cx: &mut Context<Self>) {
        self.value_row_editor
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search_live
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.capture = Capture::Filter;
        self.filter_focused = true;
        self.filter.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.capture = Capture::Palette;
        self.palette_cursor = 0;
        self.palette_query
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// One `Esc` unwinds exactly one layer, and never quits the app ([D-15]).
    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm != ConfirmDemo::None {
            self.confirm = ConfirmDemo::None;
            self.answer = Some("cancelled".into());
        } else if self.palette_open {
            self.palette_open = false;
            self.capture = Capture::None;
            window.focus(&self.focus_handle, cx);
        } else if self.capture == Capture::Filter {
            // Stage one: leave the input, keep the filter.
            self.capture = Capture::None;
            self.filter_focused = false;
            window.focus(&self.focus_handle, cx);
        } else if !self.filter_focused && !self.filter.read(cx).is_empty() {
            // Stage two: clear it.
            self.filter.update(cx, |input, cx| input.clear(cx));
            self.filter_focused = true;
        } else {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    fn accept(&mut self, _: &Accept, _window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm == ConfirmDemo::Compact {
            // `Enter` is accepted exactly where `y` is.
            self.confirm = ConfirmDemo::None;
            self.answer = Some("confirmed with \u{23ce}".into());
        } else if self.palette_open {
            self.palette_open = false;
            self.capture = Capture::None;
            self.answer = Some("ran the palette row".into());
        }
        cx.notify();
    }

    fn cursor_next(&mut self, _: &CursorNext, _window: &mut Window, cx: &mut Context<Self>) {
        let len = self.cursor_len(cx);
        if self.palette_open {
            self.palette_cursor = FuzzyList::next_cursor(self.palette_cursor, len);
        } else {
            self.fuzzy_cursor = FuzzyList::next_cursor(self.fuzzy_cursor, len);
        }
        cx.notify();
    }

    fn cursor_prev(&mut self, _: &CursorPrev, _window: &mut Window, cx: &mut Context<Self>) {
        let len = self.cursor_len(cx);
        if self.palette_open {
            self.palette_cursor = FuzzyList::prev_cursor(self.palette_cursor, len);
        } else {
            self.fuzzy_cursor = FuzzyList::prev_cursor(self.fuzzy_cursor, len);
        }
        cx.notify();
    }

    fn next_tab(&mut self, _: &NextTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.tab = SegmentedTabs::next_index(self.tab, 3);
        cx.notify();
    }

    fn prev_tab(&mut self, _: &PrevTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.tab = SegmentedTabs::prev_index(self.tab, 3);
        cx.notify();
    }

    fn cycle_next(&mut self, _: &CycleNext, _window: &mut Window, cx: &mut Context<Self>) {
        self.host = (self.host + 1).min(HOSTS.len() - 1);
        cx.notify();
    }

    fn cycle_prev(&mut self, _: &CyclePrev, _window: &mut Window, cx: &mut Context<Self>) {
        self.host = self.host.saturating_sub(1);
        cx.notify();
    }

    fn toggle_check(&mut self, _: &ToggleCheck, _window: &mut Window, cx: &mut Context<Self>) {
        self.checked = !self.checked;
        cx.notify();
    }

    fn toggle_select(&mut self, _: &ToggleSelect, _window: &mut Window, cx: &mut Context<Self>) {
        self.select_open = !self.select_open;
        cx.notify();
    }

    fn increment(&mut self, _: &Increment, _window: &mut Window, cx: &mut Context<Self>) {
        self.grace += 500;
        cx.notify();
    }

    fn decrement(&mut self, _: &Decrement, _window: &mut Window, cx: &mut Context<Self>) {
        self.grace -= 500;
        cx.notify();
    }

    fn confirm_compact(
        &mut self,
        _: &ConfirmCompact,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm = ConfirmDemo::Compact;
        self.answer = None;
        cx.notify();
    }

    fn confirm_expanded(
        &mut self,
        _: &ConfirmExpanded,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm = ConfirmDemo::Expanded;
        self.answer = None;
        cx.notify();
    }

    /// `y`, or a click on the primary `Delete`: answers only the compact confirm.
    fn confirm_yes(&mut self, _: &ConfirmYes, _window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm == ConfirmDemo::Compact {
            self.answer = Some("confirmed with y".into());
            self.confirm = ConfirmDemo::None;
        }
        cx.notify();
    }

    /// `Y`, or a click on the red `Delete anyway`: answers either confirm.
    fn confirm_strong(&mut self, _: &ConfirmStrong, _window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm != ConfirmDemo::None {
            self.answer = Some("confirmed with Y".into());
            self.confirm = ConfirmDemo::None;
        }
        cx.notify();
    }

    /// `I`, or a click on `Re-check`.
    fn confirm_recheck(
        &mut self,
        _: &ConfirmRecheck,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm != ConfirmDemo::None {
            self.answer = Some("re-checked".into());
        }
        cx.notify();
    }

    fn confirm_no(&mut self, _: &ConfirmNo, _window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm != ConfirmDemo::None {
            self.confirm = ConfirmDemo::None;
            self.answer = Some("cancelled".into());
        }
        cx.notify();
    }
}

impl Focusable for InputGallery {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.live_focused.read(cx).focus_handle()
    }
}

/// A bordered surface, so a component that paints on `surface` is judged on the right ground.
fn card(theme: &Theme, width: gpui::Pixels, child: impl IntoElement) -> AnyElement {
    div()
        .w(width)
        .rounded(theme.radii.sm)
        .bg(theme.colors.surface)
        .border(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

fn branch_section(gallery: &InputGallery, theme: &Theme, cx: &App) -> AnyElement {
    let input = gallery.branch.read(cx);
    let caret = input.buffer().caret();
    let value_len = input.text().len();
    LAYOUT.section(
        "text input \u{b7} preview and validation (^i to focus)",
        theme,
        vec![
            LAYOUT.labeled(
                "branch field",
                theme,
                div().w(px(380.0)).child(gallery.branch.clone()),
            ),
            LAYOUT.labeled(
                "state",
                theme,
                Text::hint(format!(
                    "caret {caret} of {value_len} bytes \u{b7} the validation line replaces the preview in the same 18 px slot, so a failing name costs zero layout shift"
                ))
                .faint(),
            ),
        ],
    )
}

fn text_input_section(gallery: &InputGallery, theme: &Theme) -> AnyElement {
    let width = px(420.0);
    LAYOUT.section(
        "text input · live entity",
        theme,
        vec![
            LAYOUT.labeled(
                "single · empty + placeholder",
                theme,
                div().w(width).child(gallery.live_empty.clone()),
            ),
            LAYOUT.labeled(
                "single · filled",
                theme,
                div().w(width).child(gallery.live_filled.clone()),
            ),
            LAYOUT.labeled(
                "single · focused + caret",
                theme,
                div().w(width).child(gallery.live_focused.clone()),
            ),
            LAYOUT.labeled(
                "single · selection",
                theme,
                div().w(width).child(gallery.live_selection.clone()),
            ),
            LAYOUT.labeled(
                "single · marked (IME)",
                theme,
                div().w(width).child(gallery.live_marked.clone()),
            ),
            LAYOUT.labeled(
                "single · invalid + message",
                theme,
                div().w(width).child(gallery.live_invalid.clone()),
            ),
            LAYOUT.labeled(
                "single · read-only",
                theme,
                div().w(width).child(gallery.live_read_only.clone()),
            ),
            LAYOUT.labeled(
                "single · numeric filter",
                theme,
                div().w(width).child(gallery.live_numeric.clone()),
            ),
            LAYOUT.labeled(
                "single · label + leading icon",
                theme,
                div().w(width).child(gallery.live_labeled.clone()),
            ),
            LAYOUT.labeled(
                "multi · minimum rows",
                theme,
                div().w(width).child(gallery.live_multiline_min.clone()),
            ),
            LAYOUT.labeled(
                "multi · max rows + scroll + selection",
                theme,
                div().w(width).child(gallery.live_multiline_grown.clone()),
            ),
        ],
    )
}

/// How many rows the long list holds: well past any cap, so only the wheel reaches its end.
const LONG_ROWS: usize = 24;

fn fuzzy_section(
    gallery: &InputGallery,
    this: WeakEntity<InputGallery>,
    theme: &Theme,
    cx: &App,
) -> AnyElement {
    let ranked = gallery.ranked_branches(cx);
    let items = ranked.iter().map(|(name, detail, hits)| {
        let mut item = FuzzyItem::new(*name).matches(hits.clone());
        if !detail.is_empty() {
            item = item.trailing(*detail);
        }
        item
    });

    LAYOUT.section(
        "fuzzy list",
        theme,
        vec![
            LAYOUT.labeled(
                "ranked + highlighted",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new("gallery-fuzzy-ranked", items)
                        .cap(6)
                        .cursor(gallery.fuzzy_cursor)
                        .under_text_field(true)
                        .empty(EmptyState::new(format!(
                            "Nothing matches \"{}\".",
                            gallery.filter.read(cx).text()
                        ))),
                ),
            ),
            LAYOUT.labeled(
                "two-line + disabled + key",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new(
                        "gallery-fuzzy-two-line",
                        [
                            FuzzyItem::new("buk/payroll")
                                .secondary("Chilean payroll engine \u{b7} updated 2h ago")
                                .matches([4, 5, 6])
                                .trailing("2h"),
                            FuzzyItem::new("buk/payroll-legacy")
                                .secondary("archived")
                                .disabled(true),
                            FuzzyItem::new("Prune worktrees").destructive(true).key("x"),
                        ],
                    )
                    .cap(8)
                    .cursor(0),
                ),
            ),
            LAYOUT.labeled(
                "scrolls past 6 rows · click runs a row",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new(
                        "gallery-fuzzy-long",
                        (0..LONG_ROWS).map(|ix| {
                            let item = FuzzyItem::new(format!("origin/feat/branch-{ix:02}"));
                            if ix == gallery.long_cursor {
                                item.trailing("clicked")
                            } else {
                                item
                            }
                        }),
                    )
                    .visible_rows(6)
                    .track_scroll(&gallery.long_scroll)
                    .cursor(gallery.long_cursor)
                    .on_click(move |ix, _window, cx| {
                        let Some(gallery) = this.upgrade() else {
                            return;
                        };
                        gallery.update(cx, |gallery, cx| {
                            gallery.long_cursor = ix;
                            FuzzyList::reveal(&gallery.long_scroll, ix);
                            cx.notify();
                        });
                    }),
                ),
            ),
            LAYOUT.labeled(
                "badges + chosen check (a choice, not a launcher)",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new(
                        "gallery-fuzzy-choice",
                        [
                            FuzzyItem::new("origin/main").badge("default").checked(true),
                            FuzzyItem::new("origin/release/2.4").badge("previous base"),
                            FuzzyItem::new("origin/feat/payroll-export"),
                        ],
                    )
                    .cursor(0),
                ),
            ),
            LAYOUT.labeled(
                "empty",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new("gallery-fuzzy-empty", [])
                        .empty(Text::ui("Nothing matches \"gpu\".").muted()),
                ),
            ),
        ],
    )
}

fn filter_section(gallery: &InputGallery, theme: &Theme, cx: &App) -> AnyElement {
    let shown = gallery.ranked_branches(cx).len();
    let total = BRANCHES.len();
    LAYOUT.section(
        "filter bar",
        theme,
        vec![
            LAYOUT.labeled(
                "live, with its clear \u{2715} (/ to focus; clear it to see the empty bar)",
                theme,
                card(
                    theme,
                    px(460.0),
                    FilterBar::new(gallery.filter.clone(), shown, total),
                ),
            ),
            // Stage two of the two-stage `Esc` is the retained chip, which belongs to
            // `PaneHeader`; the bar itself exists only while the input owns the keyboard.
            LAYOUT.labeled(
                "no match",
                theme,
                card(
                    theme,
                    px(460.0),
                    FilterBar::new(gallery.filter_no_match.clone(), 0, 12),
                ),
            ),
        ],
    )
}

fn choice_section(
    gallery: &InputGallery,
    this: WeakEntity<InputGallery>,
    theme: &Theme,
) -> AnyElement {
    let host = HOSTS[gallery.host];
    // Every click below writes the same field its key does, so the pointer and the keyboard
    // never disagree about the value.
    let set = move |apply: fn(&mut InputGallery, usize)| {
        let this = this.clone();
        move |ix: usize, _: &mut Window, cx: &mut App| {
            this.update(cx, |gallery, cx| {
                apply(gallery, ix);
                cx.notify();
            })
            .ok();
        }
    };
    let set_host = set(|gallery, ix| gallery.host = ix);
    let set_step = set(|gallery, ix| gallery.step = ix);
    LAYOUT.section(
        "cycler \u{b7} switch \u{b7} checkbox \u{b7} callout",
        theme,
        vec![
            LAYOUT.labeled(
                "cycler (\u{2190} \u{2192})",
                theme,
                card(
                    theme,
                    px(460.0),
                    div()
                        .flex()
                        .flex_col()
                        // Three listed options: side by side, clickable.
                        .child(
                            Cycler::labeled("host", host)
                                .label_width(px(140.0))
                                .options(HOSTS.iter().copied())
                                .on_select(set_host)
                                .focused(true)
                                .has_prev(gallery.host > 0)
                                .has_next(gallery.host + 1 < HOSTS.len()),
                        )
                        // Five listed options: a dropdown whose list opens on a click.
                        .child(
                            Cycler::labeled("keep jobs for", STEPS[gallery.step])
                                .label_width(px(140.0))
                                .options(STEPS.iter().copied())
                                .on_select(set_step)
                                .has_prev(gallery.step > 0)
                                .has_next(gallery.step + 1 < STEPS.len()),
                        )
                        // Options not listed by the caller: the field states the value only.
                        .child(
                            Cycler::labeled("on switch", "sleep")
                                .label_width(px(140.0))
                                .has_prev(false),
                        )
                        .child(
                            Cycler::labeled("theme", "system")
                                .label_width(px(140.0))
                                .options(["light", "dark", "system"])
                                .disabled(true),
                        )
                        // A persisted value that is no longer one of the configured steps: it
                        // is none of the segments, so it draws as a field that can show it, and
                        // the next `\u{2190}` / `\u{2192}` lands back on a known step.
                        .child(
                            Cycler::labeled("host", "devbox (removed)")
                                .label_width(px(140.0))
                                .options(HOSTS.iter().copied())
                                .off_grid(true)
                                .has_prev(false)
                                .has_next(false),
                        ),
                ),
            ),
            LAYOUT.labeled(
                "switch",
                theme,
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.md)
                    .child(Switch::new("switch-on", true).name("on"))
                    .child(Switch::new("switch-off", false).name("off"))
                    .child(Switch::new("switch-disabled-on", true).disabled(true))
                    .child(Switch::new("switch-disabled-off", false).disabled(true)),
            ),
            LAYOUT.labeled(
                "checkbox: checked, unchecked, disabled",
                theme,
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.lg)
                    .child(Checkbox::new("checkbox-on", "Open after creating", true))
                    .child(Checkbox::new("checkbox-off", "Open after creating", false))
                    .child(Checkbox::new("checkbox-disabled", "Locked", true).disabled(true)),
            ),
            LAYOUT.labeled(
                "callout: success, warning, with actions",
                theme,
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space.sm)
                    .w(px(460.0))
                    .child(
                        Callout::new(
                            Tone::Success,
                            Icon::Zap,
                            "Prepared copy ready \u{2014} about 2 s",
                        )
                        .detail("Hooks: pnpm install (run in background)"),
                    )
                    .child(
                        Callout::new(
                            Tone::Warning,
                            Icon::Hourglass,
                            "No prepared copy \u{2014} the first create copies the repo (~40 s) in the background",
                        )
                        .detail("Hooks: none"),
                    )
                    .child(
                        Callout::new(
                            Tone::Danger,
                            Icon::CloudOff,
                            "Sync failed: acli is not signed in",
                        )
                        .actions(Button::new("callout-settings", "Board settings").size(ButtonSize::Compact)),
                    ),
            ),
        ],
    )
}

fn tabs_and_select_section(
    gallery: &InputGallery,
    this: WeakEntity<InputGallery>,
    theme: &Theme,
) -> AnyElement {
    let options = FuzzyList::new(
        "gallery-select-options",
        BRANCHES.iter().map(|(name, detail)| {
            let mut item = FuzzyItem::new(*name);
            if !detail.is_empty() {
                item = item.trailing(*detail);
            }
            item
        }),
    )
    .cap(6)
    .cursor(0)
    .under_text_field(false);

    LAYOUT.section(
        "segmented control \u{b7} tabs \u{b7} select",
        theme,
        vec![
            LAYOUT.labeled(
                "tabs (h / l)",
                theme,
                SegmentedTabs::new([
                    SegmentedTab::new("Mine", 7),
                    SegmentedTab::new("Waiting for my review", 2).attention(true),
                    SegmentedTab::new("Closed", 0),
                ])
                .active(gallery.tab),
            ),
            LAYOUT.labeled(
                "tabs \u{b7} refreshing, zero attention",
                theme,
                SegmentedTabs::new([
                    SegmentedTab::new("Mine", 7).loading(true),
                    SegmentedTab::new("Waiting for my review", 0).attention(true),
                ])
                .active(0),
            ),
            LAYOUT.labeled(
                "tabs \u{b7} bare",
                theme,
                SegmentedTabs::new([SegmentedTab::bare("Keys"), SegmentedTab::bare("Glossary")])
                    .active(1),
            ),
            LAYOUT.labeled(
                "segmented control (h / l, click)",
                theme,
                SegmentedControl::new(
                    "gallery-segmented",
                    [
                        Segment::new("Worktrees"),
                        Segment::new("Pull requests").count(Some(4)),
                        Segment::new("Board")
                            .count(Some(0))
                            .loading(gallery.tab == 2),
                    ],
                )
                .active(Some(gallery.tab))
                .on_select({
                    let this = this.clone();
                    move |ix, _, cx| {
                        this.update(cx, |gallery, cx| {
                            gallery.tab = ix;
                            cx.notify();
                        })
                        .ok();
                    }
                }),
            ),
            LAYOUT.labeled(
                "segmented \u{b7} icons",
                theme,
                SegmentedControl::new(
                    "gallery-segmented-icons",
                    [
                        Segment::new("Claude").icon(Icon::Sparkles),
                        Segment::new("Codex")
                            .icon(Icon::SquareTerminal)
                            .kbd(Kbd::parse("ctrl-s A").ok()),
                    ],
                ),
            ),
            LAYOUT.labeled(
                "segmented \u{b7} none raised, disabled",
                theme,
                div()
                    .flex()
                    .gap(theme.space.md)
                    .child(
                        SegmentedControl::new(
                            "gallery-segmented-none",
                            [
                                Segment::new("Low"),
                                Segment::new("Medium"),
                                Segment::new("High"),
                                Segment::new("Max"),
                            ],
                        )
                        .active(None),
                    )
                    .child(
                        SegmentedControl::new(
                            "gallery-segmented-disabled",
                            [Segment::new("ssh"), Segment::new("https")],
                        )
                        .disabled(true),
                    ),
            ),
            LAYOUT.labeled(
                "segmented \u{b7} one option unavailable (dimmed, not clickable)",
                theme,
                SegmentedControl::new(
                    "gallery-segmented-unavailable",
                    [
                        Segment::new("local"),
                        Segment::new("devbox"),
                        Segment::new("archdev").disabled(true),
                    ],
                )
                .active(Some(0))
                .on_select(|_, _, _| {}),
            ),
            LAYOUT.labeled(
                "segmented \u{b7} full width",
                theme,
                div().w(px(420.0)).child(
                    SegmentedControl::new(
                        "gallery-segmented-wide",
                        [Segment::new("Claude"), Segment::new("Codex")],
                    )
                    .active(Some(1))
                    .full_width(),
                ),
            ),
            LAYOUT.labeled(
                "segmented \u{b7} one option unavailable (dimmed, not clickable)",
                theme,
                SegmentedControl::new(
                    "gallery-segmented-unavailable",
                    [
                        Segment::new("local"),
                        Segment::new("devbox"),
                        Segment::new("archdev").disabled(true),
                    ],
                )
                .active(Some(0))
                .on_select(|_, _, _| {}),
            ),
            LAYOUT.labeled(
                "select, open (o)",
                theme,
                div().w(px(420.0)).child(
                    Select::new("origin/main")
                        .label("base")
                        .focused(true)
                        .hint("\u{23ce}")
                        .open(gallery.select_open)
                        .options(options),
                ),
            ),
            LAYOUT.labeled(
                "select \u{b7} placeholder / invalid / disabled",
                theme,
                div()
                    .w(px(420.0))
                    .flex()
                    .flex_col()
                    .gap(theme.space.sm)
                    .child(
                        Select::new("")
                            .label("context")
                            .placeholder("pick a context"),
                    )
                    .child(
                        Select::new("origin/gone")
                            .label("base")
                            .invalid("that base no longer exists on origin"),
                    )
                    .child(Select::new("buk").label("owner").disabled(true)),
            ),
        ],
    )
}

fn confirm_hint_section(gallery: &InputGallery, theme: &Theme) -> AnyElement {
    LAYOUT.section(
        "confirm \u{b7} palette",
        theme,
        vec![
            LAYOUT.labeled(
                "open one",
                theme,
                KeyHintRow::new()
                    .key("d", "compact confirm (y)")
                    .key("D", "expanded confirm (Y)")
                    .key(":", "palette"),
            ),
            LAYOUT.labeled(
                "last answer",
                theme,
                Text::ui(gallery.answer.clone().unwrap_or_else(|| "\u{2014}".into())).muted(),
            ),
        ],
    )
}

fn compact_confirm() -> ConfirmDialog {
    ConfirmDialog::new(
        "Delete worktree fix-rut-validator?",
        FactList::from_facts([
            Fact::safe("clean"),
            Fact::safe("merged into origin/main"),
            Fact::safe("no session"),
        ]),
    )
    .target("buk/payroll \u{b7} ~/worktrees/buk/payroll/fix-rut-validator")
    .icon(Icon::Trash)
    .stamp(FreshnessStamp::new("Checked", 8))
    .recheck_action(Box::new(ConfirmRecheck))
    .consequence("Moves the copy to trash, then removes it in the background.")
    .dismiss_action(Box::new(ConfirmNo))
    .accept_actions(Box::new(ConfirmYes), Box::new(ConfirmStrong))
    .action_label("Delete")
}

fn expanded_confirm() -> ConfirmDialog {
    ConfirmDialog::new(
        "Delete worktree feat-payroll-fix?",
        FactList::from_facts([
            Fact::risk("12 uncommitted files").strong("12 uncommitted files"),
            Fact::risk("3 commits not on origin/main").strong("3 commits"),
            Fact::risk("session attached \u{b7} claude, :3000 running"),
            Fact::unknown("unique commit count unavailable (gh unavailable)"),
            Fact::safe("PR #412 open (not merged)"),
        ]),
    )
    .target("buk/payroll \u{b7} ~/worktrees/buk/payroll/feat-payroll-fix")
    .stamp(FreshnessStamp::new("Checked", 190))
    .recheck_action(Box::new(ConfirmRecheck))
    .consequence(
        "The session is killed and the copy moves to the trash. The 3 unpushed commits and 12 \
         uncommitted files exist only here and will be lost.",
    )
    .dismiss_action(Box::new(ConfirmNo))
    .accept_actions(Box::new(ConfirmYes), Box::new(ConfirmStrong))
    .action_label("Delete")
}

impl Render for InputGallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let typing = self.live_input_focused(window, cx) || self.capture != Capture::None;

        let sections = vec![
            branch_section(self, &theme, cx),
            text_input_section(self, &theme),
            fuzzy_section(self, cx.entity().downgrade(), &theme, cx),
            filter_section(self, &theme, cx),
            choice_section(self, cx.entity().downgrade(), &theme),
            settings::row_section(self, cx.entity().downgrade(), &theme),
            settings::card_section(&theme),
            settings::value_box_section(self, &theme),
            settings::search_section(self, cx.entity().downgrade(), &theme, cx),
            settings::breadcrumb_section(&theme),
            settings::inline_cycler_section(self, cx.entity().downgrade(), &theme),
            settings::agents_pane_section(self, cx.entity().downgrade(), &theme),
            tabs_and_select_section(self, cx.entity().downgrade(), &theme),
            confirm_hint_section(self, &theme),
        ];

        let (palette_sections, matched, total) = self.palette_sections(cx);
        let palette_query = self.palette_query.clone();
        let palette_cursor = self.palette_cursor;
        let palette_empty = self.palette_query.read(cx).text().is_empty();

        div()
            .id("gallery-input")
            .key_context(if typing {
                "GalleryTyping"
            } else {
                "GalleryNormal"
            })
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::focus_editor))
            .on_action(cx.listener(Self::focus_filter))
            .on_action(cx.listener(Self::focus_value_box))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::accept))
            .on_action(cx.listener(Self::cursor_next))
            .on_action(cx.listener(Self::cursor_prev))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::cycle_next))
            .on_action(cx.listener(Self::cycle_prev))
            .on_action(cx.listener(Self::toggle_check))
            .on_action(cx.listener(Self::toggle_select))
            .on_action(cx.listener(Self::increment))
            .on_action(cx.listener(Self::decrement))
            .on_action(cx.listener(Self::confirm_compact))
            .on_action(cx.listener(Self::confirm_expanded))
            .on_action(cx.listener(Self::confirm_yes))
            .on_action(cx.listener(Self::confirm_no))
            .on_action(cx.listener(Self::confirm_strong))
            .on_action(cx.listener(Self::confirm_recheck))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.bg)
            .text_color(theme.colors.text)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(theme.metrics.title_bar_h)
                    .px(theme.space.lg)
                    .gap(theme.space.md)
                    .bg(theme.colors.surface)
                    .border_b(theme.metrics.hairline)
                    .border_color(theme.colors.border)
                    .child(Text::label("fleet-ui-kit \u{b7} input group"))
                    .child(
                        KeyHintRow::new()
                            .key(
                                "^t",
                                if theme.mode.is_dark() {
                                    "light"
                                } else {
                                    "dark"
                                },
                            )
                            .key("^i", "editor")
                            .key("/", "filter")
                            .key(":", "palette")
                            .key("^q", "quit"),
                    ),
            )
            .child(
                div()
                    .id("gallery-input-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(theme.space.xl)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.md)
                    .h(theme.metrics.status_bar_h)
                    .px(theme.space.lg)
                    .bg(theme.colors.surface)
                    .border_t(theme.metrics.hairline)
                    .border_color(theme.colors.border)
                    .child(Text::hint(if typing {
                        "typing \u{b7} bare letters go to the field"
                    } else {
                        "normal \u{b7} bare letters are actions"
                    }))
                    .child(Text::hint(format!("{matched} of {total} palette candidates")).faint()),
            )
            .when(self.palette_open, |el| {
                el.child(
                    Overlay::new().content(
                        palette_sections.into_iter().fold(
                            Palette::new(palette_query)
                                .cursor(palette_cursor)
                                .scope("All")
                                .when(palette_empty, |palette| {
                                    palette.prefix_hint([
                                        (">", "commands"),
                                        ("@", "worktrees"),
                                        ("#", "cards"),
                                    ])
                                })
                                .empty("Nothing matches that query."),
                            Palette::section,
                        ),
                    ),
                )
            })
            .when(self.confirm == ConfirmDemo::Compact, |el| {
                el.child(compact_confirm())
            })
            .when(self.confirm == ConfirmDemo::Expanded, |el| {
                el.child(expanded_confirm())
            })
    }
}

fn main() {
    support::runtime::run_with_window(
        "fleet-ui-kit · input gallery",
        (1180.0, 880.0),
        Quit,
        |cx| {
            cx.bind_keys(support::input::bindings());
            cx.bind_keys(menu_key_bindings());
            cx.bind_keys([
                // Always available, in both modes.
                KeyBinding::new("ctrl-t", ToggleTheme, None),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-i", FocusEditor, None),
                KeyBinding::new("ctrl-e", FocusValueBox, None),
                KeyBinding::new("ctrl-f", FocusSearch, None),
                KeyBinding::new("escape", Escape, None),
                KeyBinding::new("enter", Accept, None),
                KeyBinding::new("ctrl-n", CursorNext, None),
                KeyBinding::new("ctrl-p", CursorPrev, None),
                KeyBinding::new("down", CursorNext, None),
                KeyBinding::new("up", CursorPrev, None),
                // Bare letters exist only while no field owns the keyboard.
                KeyBinding::new("/", FocusFilter, Some("GalleryNormal")),
                KeyBinding::new(":", OpenPalette, Some("GalleryNormal")),
                KeyBinding::new("d", ConfirmCompact, Some("GalleryNormal")),
                KeyBinding::new("shift-d", ConfirmExpanded, Some("GalleryNormal")),
                KeyBinding::new("y", ConfirmYes, Some("GalleryNormal")),
                KeyBinding::new("shift-y", ConfirmStrong, Some("GalleryNormal")),
                KeyBinding::new("shift-i", ConfirmRecheck, Some("GalleryNormal")),
                KeyBinding::new("n", ConfirmNo, Some("GalleryNormal")),
                KeyBinding::new("h", PrevTab, Some("GalleryNormal")),
                KeyBinding::new("l", NextTab, Some("GalleryNormal")),
                KeyBinding::new("left", CyclePrev, Some("GalleryNormal")),
                KeyBinding::new("right", CycleNext, Some("GalleryNormal")),
                KeyBinding::new("space", ToggleCheck, Some("GalleryNormal")),
                KeyBinding::new("o", ToggleSelect, Some("GalleryNormal")),
                KeyBinding::new("+", Increment, Some("GalleryNormal")),
                KeyBinding::new("-", Decrement, Some("GalleryNormal")),
            ]);
        },
        InputGallery::new,
    );
}

/// A gallery key chip. The app resolves chips from its keymap; the gallery has none.
fn chip(keys: &str) -> Kbd {
    Kbd::parse(keys).unwrap_or_else(|error| panic!("{keys:?}: {error}"))
}
