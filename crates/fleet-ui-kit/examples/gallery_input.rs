//! The visual and behavioural test bench for the **input** group of `fleet-ui-kit`.
//!
//! `TextField` · `TextFieldState` · `TextInput` · `FuzzyList` · `FilterBar` · `Cycler` ·
//! `Toggle` · `NumberField` · `SegmentedTabs` · `Select` · `ConfirmDialog` · `Palette`.
//!
//! Every component appears in every state it can be in, in both themes, and the interactive
//! ones are *live*: the text field really edits, the palette really filters and highlights, the
//! lists really move. If a state is not visible or not operable here, it is not implemented.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_input
//! ```
//!
//! | Key | What |
//! | --- | --- |
//! | `ctrl-t` | toggle light / dark |
//! | `ctrl-i` | focus the live editor · `esc` leaves it |
//! | `/` | focus the filter bar · `esc` leaves it, `esc` again clears it |
//! | `:` | open the palette · `esc` closes it |
//! | `d` / `D` | the compact / expanded confirm · `y` `Y` `n` `esc` answer it |
//! | `ctrl-n` `ctrl-p` `↓` `↑` | move the fuzzy / palette cursor (also while typing) |
//! | `h` `l` | previous / next tab |
//! | `←` `→` | cycle the host value |
//! | `space` | toggle the focused checkbox |
//! | `o` | open / close the select |
//! | `+` `-` | change the focused number field |
//! | `ctrl-q` / `cmd-q` | quit |
//!
//! Bare-letter keys are bound only in the `GalleryNormal` context. While a field owns the
//! keyboard the root switches to `GalleryTyping`, so `h`, `y`, `o` and friends are typed
//! instead of fired — the same rule §3.10 states for the real Hub.

use fleet_ui_kit::KitAssets;
use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, KeyDownEvent,
    Menu, MenuItem, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, actions,
    div, px, size,
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
        ConfirmNo,
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

// ---------------------------------------------------------------------------------- fixtures

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

// ------------------------------------------------------------------------------------- view

struct InputGallery {
    focus_handle: FocusHandle,
    editor: Entity<TextInput>,
    filter: TextFieldState,
    palette_query: TextFieldState,
    capture: Capture,
    filter_focused: bool,
    fuzzy_cursor: usize,
    palette_cursor: usize,
    palette_open: bool,
    tab: usize,
    host: usize,
    checked: bool,
    grace: i64,
    select_open: bool,
    confirm: ConfirmDemo,
    answer: Option<SharedString>,
}

const HOSTS: &[&str] = &["local", "devbox", "ci-runner"];

impl InputGallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            TextInput::new(cx)
                .with_label("branch")
                .with_placeholder("feat/rut-validator")
                .with_icon(Icon::GitBranchPlus)
                .with_mono(true)
                .with_text("feat/rut-validator")
        });
        cx.subscribe(&editor, Self::on_editor_event).detach();
        let mut gallery = Self {
            focus_handle: cx.focus_handle(),
            editor,
            filter: TextFieldState::from_text("rut"),
            palette_query: TextFieldState::from_text("pay"),
            capture: Capture::None,
            filter_focused: true,
            fuzzy_cursor: 0,
            palette_cursor: 0,
            palette_open: false,
            tab: 0,
            host: 0,
            checked: true,
            grace: 2_000,
            select_open: true,
            confirm: ConfirmDemo::None,
            answer: None,
        };
        gallery.refresh_editor_status(cx);
        gallery
    }

    /// Keep the preview and the validation line in step with the value.
    fn on_editor_event(
        &mut self,
        _editor: Entity<TextInput>,
        _event: &TextInputEvent,
        cx: &mut Context<Self>,
    ) {
        self.refresh_editor_status(cx);
        cx.notify();
    }

    fn refresh_editor_status(&mut self, cx: &mut Context<Self>) {
        let value = self.editor.read(cx).text().to_string();
        let error = branch_error(&value);
        let preview = (!value.is_empty() && error.is_none())
            .then(|| SharedString::from(format!("\u{2192} buk/payroll#{value}")));
        self.editor.update(cx, |editor, cx| {
            editor.set_invalid(error, cx);
            editor.set_preview(preview, cx);
        });
    }

    /// The rows the fuzzy list shows for the current filter query.
    fn ranked_branches(&self) -> Vec<(&'static str, &'static str, Vec<usize>)> {
        BRANCHES
            .iter()
            .filter_map(|(name, detail)| {
                subsequence(name, self.filter.text()).map(|hits| (*name, *detail, hits))
            })
            .collect()
    }

    /// The palette's three sections for the current query, already ranked.
    fn palette_sections(&self) -> (Vec<PaletteSection>, usize, usize) {
        let query = self.palette_query.text();
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
                        .key(*key)
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
                    PaletteRow::new(format!("Switch to context: {label}"))
                        .key(*digit)
                        .matches(hits.into_iter().map(|ix| ix + 22))
                        .icon(Icon::Boxes)
                })
            })
            .collect();

        let matched = go.len() + do_rows.len() + contexts.len();
        let total = GO_ROWS.len() + DO_ROWS.len() + CONTEXT_ROWS.len();
        let sections = vec![
            PaletteSection::new(PaletteSectionKind::Go, go),
            PaletteSection::new(PaletteSectionKind::Do, do_rows),
            PaletteSection::new(PaletteSectionKind::Context, contexts),
        ];
        (sections, matched, total)
    }

    /// How many rows the cursor may land on right now.
    fn cursor_len(&self) -> usize {
        if self.palette_open {
            let (sections, matched, _) = self.palette_sections();
            let _ = sections;
            matched.min(10)
        } else {
            self.ranked_branches().len()
        }
    }

    // ------------------------------------------------------------------ actions

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn focus_editor(&mut self, _: &FocusEditor, window: &mut Window, cx: &mut Context<Self>) {
        self.capture = Capture::None;
        let handle = self.editor.focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.capture = Capture::Filter;
        self.filter_focused = true;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.capture = Capture::Palette;
        self.palette_cursor = 0;
        window.focus(&self.focus_handle, cx);
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
        } else if self.capture == Capture::Filter {
            // Stage one: leave the input, keep the filter.
            self.capture = Capture::None;
            self.filter_focused = false;
        } else if !self.filter_focused && !self.filter.is_empty() {
            // Stage two: clear it.
            self.filter.clear();
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
        let len = self.cursor_len();
        if self.palette_open {
            self.palette_cursor = FuzzyList::next_cursor(self.palette_cursor, len);
        } else {
            self.fuzzy_cursor = FuzzyList::next_cursor(self.fuzzy_cursor, len);
        }
        cx.notify();
    }

    fn cursor_prev(&mut self, _: &CursorPrev, _window: &mut Window, cx: &mut Context<Self>) {
        let len = self.cursor_len();
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

    fn confirm_yes(&mut self, _: &ConfirmYes, _window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm != ConfirmDemo::None {
            self.answer = Some(match self.confirm {
                ConfirmDemo::Expanded => "confirmed with Y".into(),
                _ => SharedString::from("confirmed with y"),
            });
            self.confirm = ConfirmDemo::None;
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

    /// Feed keystrokes to whichever presentational field owns the keyboard.
    ///
    /// This is the pattern §3.10 describes for the real Hub: `FilterBar` and `Palette` render
    /// the caret, and the view owns a [`TextFieldState`] that implements the edit set.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.capture {
            Capture::Filter => &mut self.filter,
            Capture::Palette => &mut self.palette_query,
            Capture::None => return,
        };
        if target.handle_keystroke(&event.keystroke) {
            cx.stop_propagation();
            // A re-ranked list must never leave the cursor past its end.
            self.fuzzy_cursor = 0;
            self.palette_cursor = 0;
            cx.notify();
        }
    }
}

impl Focusable for InputGallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// --------------------------------------------------------------------------- layout helpers

fn section(title: &str, theme: &Theme, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.md)
        .pb(theme.space.xl)
        .child(SectionHeader::new(title.to_string()))
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(theme.space.sm)
                .children(children),
        )
        .into_any_element()
}

fn labeled(label: &str, theme: &Theme, child: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_start()
        .w_full()
        .gap(theme.space.md)
        .child(Text::hint(label.to_string()).faint().w(px(170.0)))
        .child(div().flex().flex_1().min_w_0().flex_col().child(child))
        .into_any_element()
}

/// A bordered surface, so a component that paints on `surface` is judged on the right ground.
fn card(theme: &Theme, width: gpui::Pixels, child: impl IntoElement) -> AnyElement {
    div()
        .w(width)
        .rounded(theme.radii.sm)
        .bg(theme.colors.surface)
        .border_1()
        .border_color(theme.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

// -------------------------------------------------------------------------------- sections

fn text_field_section(theme: &Theme) -> AnyElement {
    let width = px(380.0);
    section(
        "text field \u{b7} presentational",
        theme,
        vec![
            labeled(
                "focused + preview",
                theme,
                div().w(width).child(
                    TextField::new("feat/rut-validator")
                        .label("branch")
                        .mono(true)
                        .focused(true)
                        .caret(4)
                        .preview("\u{2192} buk/payroll#feat-rut-validator"),
                ),
            ),
            labeled(
                "invalid (same slot)",
                theme,
                div().w(width).child(
                    TextField::new("feat/../rut")
                        .label("branch")
                        .mono(true)
                        .focused(true)
                        .caret(7)
                        .preview("\u{2192} never shown while invalid")
                        .invalid("branch cannot contain \"..\""),
                ),
            ),
            labeled(
                "unfocused",
                theme,
                div()
                    .w(width)
                    .child(TextField::new("origin/main").label("base").mono(true)),
            ),
            labeled(
                "placeholder + icon",
                theme,
                div().w(width).child(
                    TextField::new("")
                        .placeholder("Type to search GitHub repos in buk's owners.")
                        .icon(Icon::Search)
                        .focused(true),
                ),
            ),
            labeled(
                "44 px, no status slot",
                theme,
                div().w(width).child(
                    TextField::new("pay fix")
                        .icon(Icon::Command)
                        .focused(true)
                        .height(px(44.0))
                        .hide_status_line(true),
                ),
            ),
        ],
    )
}

fn live_editor_section(gallery: &InputGallery, theme: &Theme, cx: &App) -> AnyElement {
    let state = gallery.editor.read(cx);
    let caret = state.state().caret_chars();
    let value_len = state.text().chars().count();
    section(
        "text input \u{b7} live (ctrl-i to focus)",
        theme,
        vec![
            labeled(
                "real editor",
                theme,
                div().w(px(380.0)).child(gallery.editor.clone()),
            ),
            labeled(
                "state",
                theme,
                Text::hint(format!(
                    "caret {caret} of {value_len} \u{b7} printable \u{b7} backspace \u{b7} delete \u{b7} ^w \u{b7} ^u \u{b7} ^k \u{b7} ^a \u{b7} ^e \u{b7} \u{2190} \u{2192} \u{b7} IME-composed text is underlined"
                ))
                .faint(),
            ),
        ],
    )
}

fn fuzzy_section(gallery: &InputGallery, theme: &Theme) -> AnyElement {
    let ranked = gallery.ranked_branches();
    let items = ranked.iter().map(|(name, detail, hits)| {
        let mut item = FuzzyItem::new(*name).matches(hits.clone());
        if !detail.is_empty() {
            item = item.trailing(*detail);
        }
        item
    });

    section(
        "fuzzy list",
        theme,
        vec![
            labeled(
                "ranked + highlighted",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new(items)
                        .cap(6)
                        .cursor(gallery.fuzzy_cursor)
                        .under_text_field(true)
                        .empty(EmptyState::new(format!(
                            "Nothing matches \"{}\".",
                            gallery.filter.text()
                        ))),
                ),
            ),
            labeled(
                "two-line + disabled + key",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new([
                        FuzzyItem::new("buk/payroll")
                            .secondary("Chilean payroll engine \u{b7} updated 2h ago")
                            .matches([4, 5, 6])
                            .trailing("2h"),
                        FuzzyItem::new("buk/payroll-legacy")
                            .secondary("archived")
                            .disabled(true),
                        FuzzyItem::new("Prune worktrees").destructive(true).key("x"),
                    ])
                    .cap(8)
                    .cursor(0),
                ),
            ),
            labeled(
                "empty",
                theme,
                card(
                    theme,
                    px(420.0),
                    FuzzyList::new([]).empty(Text::ui("Nothing matches \"gpu\".").muted()),
                ),
            ),
        ],
    )
}

fn filter_section(gallery: &InputGallery, theme: &Theme) -> AnyElement {
    let shown = gallery.ranked_branches().len();
    let total = BRANCHES.len();
    section(
        "filter bar",
        theme,
        vec![
            labeled(
                "live (/ to focus)",
                theme,
                card(
                    theme,
                    px(460.0),
                    FilterBar::new(gallery.filter.shared_text(), shown, total)
                        .caret(gallery.filter.caret_chars())
                        .focused(gallery.capture == Capture::Filter)
                        .placeholder("filter branches"),
                ),
            ),
            labeled(
                "retained (input exited)",
                theme,
                card(
                    theme,
                    px(460.0),
                    FilterBar::new("rut", 2, 12).focused(false),
                ),
            ),
            labeled(
                "no match",
                theme,
                card(theme, px(460.0), FilterBar::new("zzz", 0, 12)),
            ),
        ],
    )
}

fn choice_section(gallery: &InputGallery, theme: &Theme) -> AnyElement {
    let host = HOSTS[gallery.host];
    section(
        "cycler \u{b7} toggle \u{b7} number field",
        theme,
        vec![
            labeled(
                "cycler (\u{2190} \u{2192})",
                theme,
                card(
                    theme,
                    px(420.0),
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            Cycler::labeled("host", host)
                                .label_width(px(140.0))
                                .focused(true)
                                .has_prev(gallery.host > 0)
                                .has_next(gallery.host + 1 < HOSTS.len()),
                        )
                        .child(
                            Cycler::labeled("on switch", "sleep")
                                .label_width(px(140.0))
                                .has_prev(false),
                        )
                        .child(
                            Cycler::labeled("theme", "system")
                                .label_width(px(140.0))
                                .disabled(true),
                        ),
                ),
            ),
            labeled(
                "toggles (space)",
                theme,
                card(
                    theme,
                    px(420.0),
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            Toggle::labeled("Sleep on switch", gallery.checked)
                                .label_width(px(200.0))
                                .focused(true),
                        )
                        .child(
                            Toggle::labeled("Warn before quitting", false).label_width(px(200.0)),
                        )
                        .child(
                            Toggle::labeled("Claude keep-alive", true)
                                .label_width(px(200.0))
                                .detail("matching 2 processes now"),
                        )
                        .child(
                            Toggle::labeled("Managed by config.json", true)
                                .label_width(px(200.0))
                                .disabled(true),
                        ),
                ),
            ),
            labeled(
                "number fields (+ \u{2212})",
                theme,
                card(
                    theme,
                    px(460.0),
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            NumberField::labeled("grace", gallery.grace)
                                .label_width(px(150.0))
                                .unit("ms")
                                .range(0, 60_000)
                                .focused(true),
                        )
                        .child(
                            NumberField::labeled("status refresh", 200)
                                .label_width(px(150.0))
                                .unit("ms")
                                .min(500),
                        )
                        .child(
                            NumberField::labeled("pool size", 3)
                                .label_width(px(150.0))
                                .range(1, 8),
                        )
                        .child(
                            NumberField::labeled("pr ttl", 90)
                                .label_width(px(150.0))
                                .unit("s")
                                .invalid("the daemon refused this value"),
                        ),
                ),
            ),
        ],
    )
}

fn tabs_and_select_section(gallery: &InputGallery, theme: &Theme) -> AnyElement {
    let options = FuzzyList::new(BRANCHES.iter().map(|(name, detail)| {
        let mut item = FuzzyItem::new(*name);
        if !detail.is_empty() {
            item = item.trailing(*detail);
        }
        item
    }))
    .cap(6)
    .cursor(0)
    .under_text_field(false);

    section(
        "segmented tabs \u{b7} select",
        theme,
        vec![
            labeled(
                "tabs (h / l)",
                theme,
                SegmentedTabs::new([
                    SegmentedTab::new("mine", 7),
                    SegmentedTab::new("review", 4).loading(true),
                    SegmentedTab::new("closed", 0),
                ])
                .active(gallery.tab),
            ),
            labeled(
                "tabs \u{b7} bare",
                theme,
                SegmentedTabs::new([SegmentedTab::bare("keys"), SegmentedTab::bare("glossary")])
                    .active(1),
            ),
            labeled(
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
            labeled(
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
    section(
        "confirm \u{b7} palette",
        theme,
        vec![
            labeled(
                "open one",
                theme,
                KeyHintRow::new()
                    .key("d", "compact confirm (y)")
                    .key("D", "expanded confirm (Y)")
                    .key(":", "palette"),
            ),
            labeled(
                "last answer",
                theme,
                Text::ui(gallery.answer.clone().unwrap_or_else(|| "\u{2014}".into())).muted(),
            ),
        ],
    )
}

// --------------------------------------------------------------------------------- overlays

fn compact_confirm() -> ConfirmDialog {
    ConfirmDialog::new(
        "Delete buk/payroll#fix-rut-validator?",
        FactList::from_facts([
            Fact::safe("clean"),
            Fact::safe("merged into origin/main"),
            Fact::safe("no session"),
        ]),
    )
    .target("buk/payroll#fix-rut-validator")
    .icon(Icon::Trash)
    .stamp(FreshnessStamp::new("checked", 8).action("I", "re-check"))
    .consequence("Moves the copy to trash, then removes it in the background.")
    .hints(KeyHintRow::new().key("I", "re-check"))
    .action_label("Delete")
}

fn expanded_confirm() -> ConfirmDialog {
    ConfirmDialog::new(
        "Delete worktree",
        FactList::from_facts([
            Fact::risk("12 uncommitted files"),
            Fact::risk("3 commits not on origin/main"),
            Fact::risk("session attached \u{b7} claude, :3000 running"),
            Fact::unknown("unique commit count unavailable (gh unavailable)"),
            Fact::safe("PR #412 open (not merged)"),
        ]),
    )
    .target("buk/payroll#feat-payroll-fix")
    .stamp(FreshnessStamp::new("checked", 190).action("I", "re-check"))
    .consequence(
        "Deleting kills the session and moves the copy to trash; commits that exist only here are lost.",
    )
    .hints(KeyHintRow::new().key("I", "re-check"))
    .action_label("Delete")
}

impl Render for InputGallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let editor_focused = self.editor.focus_handle(cx).is_focused(window);
        let typing = editor_focused || self.capture != Capture::None;

        let sections = vec![
            text_field_section(&theme),
            live_editor_section(self, &theme, cx),
            fuzzy_section(self, &theme),
            filter_section(self, &theme),
            choice_section(self, &theme),
            tabs_and_select_section(self, &theme),
            confirm_hint_section(self, &theme),
        ];

        let (palette_sections, matched, total) = self.palette_sections();
        let palette_query = self.palette_query.shared_text();
        let palette_caret = self.palette_query.caret_chars();
        let palette_cursor = self.palette_cursor;

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
            .on_key_down(cx.listener(Self::on_key_down))
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
                    .h(theme.metrics.context_bar_h)
                    .px(theme.space.lg)
                    .gap(theme.space.md)
                    .bg(theme.colors.surface)
                    .border_b(px(1.0))
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
                    .border_t(px(1.0))
                    .border_color(theme.colors.border)
                    .child(ModeWord::new(if typing {
                        Mode::Filter
                    } else {
                        Mode::Normal
                    }))
                    .child(Text::hint(if typing {
                        "typing \u{b7} bare letters go to the field"
                    } else {
                        "normal \u{b7} bare letters are actions"
                    }))
                    .child(Text::hint(format!("{matched} of {total} palette candidates")).faint()),
            )
            .when(self.palette_open, |el| {
                el.child(
                    Overlay::new().child(
                        palette_sections.into_iter().fold(
                            Palette::new(palette_query)
                                .caret(palette_caret)
                                .cursor(palette_cursor)
                                .total(total)
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
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(|cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            cx.bind_keys([
                // Always available, in both modes.
                KeyBinding::new("ctrl-t", ToggleTheme, None),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-i", FocusEditor, None),
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
                KeyBinding::new("shift-y", ConfirmYes, Some("GalleryNormal")),
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
            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.set_menus(vec![Menu {
                name: "fleet-ui-kit".into(),
                items: vec![MenuItem::action("Quit", Quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx: &mut App, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1180.0), px(880.0)), cx);
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("fleet-ui-kit \u{b7} input gallery".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_window, cx| {
                    let view: Entity<InputGallery> = cx.new(InputGallery::new);
                    view
                },
            );

            if let Ok(window) = window {
                window
                    .update(cx, |view, window, cx| {
                        window.focus(&view.focus_handle(cx), cx);
                    })
                    .ok();
            }

            cx.activate(true);
        });
}
