//! §3.8.7 Help (`?`) — every context side by side, grouped by mode.

use std::collections::HashMap;

use fleet_ui_kit::{Icon, TextRole, prelude::*, styled_with};
use gpui::{
    AnyElement, App, Entity, EntityId, FocusHandle, Global, HighlightStyle, ScrollHandle,
    StyledText, Window, div, point, px,
};

use crate::{
    actions::dialog,
    dialogs::{notify, root},
    keymap,
    presentation::{age_secs, humanize, now_unix, pretty_keys},
    state::{AppState, Screen},
};

/// The wire protocol this build speaks, for the Settings About section.
#[must_use]
pub(super) const fn protocol() -> u32 {
    fleet_proto::PROTOCOL_VERSION
}

/// The width of the key column (§3.8.7: 68 px mono).
const KEY_COLUMN: f32 = 68.0;

/// How many columns §3.8.7 gives the grid.
///
/// Six groups over three columns, not six columns: in an 880 px card six side-by-side
/// columns leave ~70 px for the action label, which ellipsized nearly every one of them
/// (`half p…`, `select…` nine rows running). Three columns leave ~190 px, which is what makes
/// the labels readable — and readable labels are the entire job of a keymap.
const COLUMNS: usize = 3;
const KEYBOARD_SCROLL_STEP: f32 = 48.0;

#[derive(Default)]
struct HelpScrollRegistry(HashMap<EntityId, ScrollHandle>);

impl Global for HelpScrollRegistry {}

fn help_scroll(state: &Entity<AppState>, cx: &mut App) -> ScrollHandle {
    let id = state.entity_id();
    if let Some(scroll) = cx.default_global::<HelpScrollRegistry>().0.get(&id) {
        return scroll.clone();
    }
    let scroll = ScrollHandle::new();
    cx.default_global::<HelpScrollRegistry>()
        .0
        .insert(id, scroll.clone());
    cx.observe_release(state, move |_, cx| {
        cx.default_global::<HelpScrollRegistry>().0.remove(&id);
    })
    .detach();
    scroll
}

fn next_scroll_offset(current: f32, maximum: f32, delta: f32) -> f32 {
    (current - delta).clamp(-maximum.max(0.0), 0.0)
}

fn scroll_by(scroll: &ScrollHandle, delta: f32) {
    let offset = scroll.offset();
    let next = next_scroll_offset(f32::from(offset.y), f32::from(scroll.max_offset().y), delta);
    scroll.set_offset(point(offset.x, px(next)));
}

/// The paragraph §3.8.7 calls "the single most valuable paragraph in the app", in the spec's
/// own markdown.
///
/// The backticks are **markup**, not characters: [`key_paragraph`] strips them and reports the
/// ranges so the keys render in the same mono face as the key column below. Every key the spec
/// backticks is backticked here, so the four key names in the sentence are styled alike.
const WHAT_KEEPS_RUNNING: &str = "What keeps running. Jobs and sessions live in fleetd. \
Closing a dialog, leaving a screen or quitting Fleet (`ctrl-q`) never stops them. Only `c` in \
the Jobs panel, `K`, and `ctrl-shift-q` stop things. Terminals survive a daemon restart and \
reattach on their own.";

/// The terminal clipboard behaviour that is not expressible as a keymap row.
const TERMINAL_CLIPBOARD: &str = "Terminal clipboard. Drag selects, double-click selects a \
word, and triple-click selects a line; selection copies immediately. `cmd-c` copies the \
selection; `cmd-v` pastes. `ctrl-c` and `ctrl-v` stay terminal keys.";

/// Splits a backticked sentence into the plain text and the byte ranges of its key names.
///
/// The app has no markdown renderer and needs none: the one paragraph it shows uses backticks
/// for exactly one thing, and printing them verbatim is how the help came to read
/// "Only `c` in the Jobs panel".
#[must_use]
fn key_paragraph(markdown: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut text = String::with_capacity(markdown.len());
    let mut keys = Vec::new();
    let mut rest = markdown;
    while let Some(open) = rest.find('`') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            // An unpaired backtick is a character, not markup: print what is there.
            text.push_str(&rest[open..]);
            return (text, keys);
        };
        let start = text.len();
        text.push_str(&after[..close]);
        keys.push(start..text.len());
        rest = &after[close + 1..];
    }
    text.push_str(rest);
    (text, keys)
}

/// One key context's rows inside a [`Group`].
///
/// A group can cover several mutually exclusive key contexts — `Dialogs & filter` covers
/// thirteen — and a row must never mix keys from two of them: `esc / q / ? close` is true in no
/// context at all, while `Esc` alone is true in the palette and `q` alone in a confirm. Rows are
/// therefore merged **within** a section, and the section says which context it documents.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    /// The gpui key-context predicate these rows are scoped to.
    context: &'static str,
    /// The sub-head, or `None` when the group documents a single context and the group title
    /// already names it.
    title: Option<gpui::SharedString>,
    /// Its rows, in registration order.
    rows: Vec<(gpui::SharedString, gpui::SharedString)>,
}

/// One column of the help grid.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    /// The mode this column documents.
    title: &'static str,
    /// One section per contributing key context, in `GROUPS` order.
    sections: Vec<Section>,
}

impl Group {
    /// How many rows the group holds across every section.
    #[must_use]
    fn row_count(&self) -> usize {
        self.sections.iter().map(|section| section.rows.len()).sum()
    }

    /// How many lines the group occupies: its head, every sub-head, and every row.
    #[must_use]
    fn height(&self) -> usize {
        1 + self.row_count()
            + self
                .sections
                .iter()
                .filter(|section| section.title.is_some())
                .count()
    }
}

/// The sub-head a key context gets: its own name, without the parent the group already says.
///
/// `Dialog > Confirm` is `confirm`, `Hub > Worktrees` is `worktrees`, `Workspace > Prefix` is
/// `prefix` — which is also the word that tells the reader those keys follow `^s`.
#[must_use]
fn context_label(context: &str) -> String {
    humanize(context.rsplit(" > ").next().unwrap_or(context))
}

/// Which key contexts feed each group.
const GROUPS: &[(&str, &[&str])] = &[
    ("Hub", &["Fleet", "Hub"]),
    (
        "Worktrees & PRs",
        &["Hub > Repos", "Hub > Worktrees", "Hub > Prs"],
    ),
    (
        "Terminal (^s)",
        &[
            "Workspace > Terminal",
            "Workspace > Native",
            "Workspace > Prefix",
        ],
    ),
    ("Scroll", &["Workspace > Scroll"]),
    (
        "Agent popup (^s)",
        &[
            "Agent",
            "Agent > Terminal",
            "Agent > Prefix",
            "Agent > Scroll",
            "Agent > AgentIdle",
            "Agent > AgentWorking",
            "Agent > AgentDecision > AgentPermission",
            "Agent > AgentDecision > AgentQuestion",
            "Agent > AgentDecision > AgentPlan",
            "Agent > AgentNativeScroll",
            "Agent > AgentNativeScroll > AgentRow",
        ],
    ),
    (
        "Board",
        &[
            "Hub > Board",
            "Filter > BoardFilter",
            "Dialog > CardDetail",
            "Dialog > CardCreate",
            "Dialog > CardPicker",
            "Dialog > BoardSettings",
        ],
    ),
    (
        "Dialogs & filter",
        &[
            "Filter",
            "Palette",
            "Jobs",
            "Jobs > Log",
            "Dialog",
            "Dialog > Create",
            "Dialog > Confirm",
            "Dialog > Context",
            "Dialog > Assign",
            "Dialog > Settings",
            "Dialog > Help",
            "Dialog > Quit",
            "Dialog > QuitDaemon",
        ],
    ),
];

/// Builds every column from the binding table.
///
/// Two bindings share a row only when they share **both** the action label and the key context:
/// merging on the label alone produced rows like `⏎ / y / Y accept` out of four different
/// dialogs' `Accept` actions, and such a row is accurate in none of them.
#[must_use]
fn groups() -> Vec<Group> {
    let table = keymap::table();
    GROUPS
        .iter()
        .map(|(title, contexts)| {
            let multi = contexts.len() > 1;
            let mut sections: Vec<Section> = Vec::new();
            for context in *contexts {
                let mut rows: Vec<(String, String)> = Vec::new();
                for spec in table.iter().filter(|spec| spec.context == *context) {
                    let label = humanize(spec.action);
                    let keys = pretty_keys(spec.keys);
                    match rows.iter_mut().find(|(_, existing)| existing == &label) {
                        Some((existing_keys, _)) => {
                            if !existing_keys.split(" / ").any(|key| key == keys) {
                                existing_keys.push_str(" / ");
                                existing_keys.push_str(&keys);
                            }
                        }
                        None => rows.push((keys, label)),
                    }
                }
                if rows.is_empty() {
                    continue;
                }
                sections.push(Section {
                    context,
                    title: multi.then(|| context_label(context).into()),
                    rows: rows
                        .into_iter()
                        .map(|(keys, label)| (keys.into(), label.into()))
                        .collect(),
                });
            }
            Group { title, sections }
        })
        .collect()
}

/// Packs the mode groups into [`COLUMNS`] columns, keeping their reading order.
///
/// The groups stay contiguous — a column is one or two whole modes, never half of one — and the
/// split is the one that makes the tallest column as short as possible, so the grid stays
/// balanced whichever group the caller moved to the front.
#[must_use]
fn columns(groups: Vec<Group>) -> Vec<Vec<Group>> {
    if groups.len() <= COLUMNS {
        return groups.into_iter().map(|group| vec![group]).collect();
    }
    // Every group carries its head and every section its sub-head, so a column's height is
    // the sum of what the groups in it actually draw.
    let heights: Vec<usize> = groups.iter().map(Group::height).collect();
    let mut best: Option<(usize, Vec<usize>)> = None;
    for first in 1..groups.len() {
        for second in (first + 1)..groups.len() {
            let cuts = vec![first, second];
            let tallest = column_heights(&heights, &cuts)
                .into_iter()
                .max()
                .unwrap_or(0);
            if best.as_ref().is_none_or(|(current, _)| tallest < *current) {
                best = Some((tallest, cuts));
            }
        }
    }
    let cuts = best.map_or_else(Vec::new, |(_, cuts)| cuts);
    let mut columns: Vec<Vec<Group>> = vec![Vec::new(); COLUMNS];
    let mut column = 0;
    for (index, group) in groups.into_iter().enumerate() {
        if cuts.get(column).is_some_and(|cut| index == *cut) {
            column += 1;
        }
        columns[column.min(COLUMNS - 1)].push(group);
    }
    columns
}

/// The height of each column for a given pair of cut points.
fn column_heights(heights: &[usize], cuts: &[usize]) -> Vec<usize> {
    let mut totals = vec![0usize; COLUMNS];
    let mut column = 0;
    for (index, height) in heights.iter().enumerate() {
        if cuts.get(column).is_some_and(|cut| index == *cut) {
            column += 1;
        }
        totals[column.min(COLUMNS - 1)] += height;
    }
    totals
}

fn prepared_columns(active_group: Option<&str>) -> &'static [Vec<Group>] {
    static LAYOUTS: std::sync::OnceLock<[Vec<Vec<Group>>; 3]> = std::sync::OnceLock::new();
    let layouts = LAYOUTS.get_or_init(|| {
        [None, Some("Terminal (^s)"), Some("Agent popup (^s)")].map(|active| {
            let mut groups = groups();
            if let Some(index) =
                active.and_then(|title| groups.iter().position(|group| group.title == title))
            {
                let group = groups.remove(index);
                groups.insert(0, group);
            }
            columns(groups)
        })
    });
    &layouts[match active_group {
        Some("Terminal (^s)") => 1,
        Some("Agent popup (^s)") => 2,
        _ => 0,
    }]
}

pub(super) fn prepare() {
    prepared_columns(None);
}

/// The §3.8.7 paragraph, with its key names in the same mono face as the key column below.
///
/// It is one shaped run rather than a row of elements so the sentence wraps like a sentence;
/// only the face and the contrast change inside it.
fn key_paragraph_view(markdown: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (text, keys) = key_paragraph(markdown);
    let key_color = theme.colors.text;
    let mono = theme.font_mono.clone();
    let highlights = keys
        .iter()
        .cloned()
        .map(|range| {
            (
                range,
                HighlightStyle {
                    color: Some(key_color),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect::<Vec<_>>();
    let families = keys
        .into_iter()
        .map(|range| (range, mono.clone()))
        .collect::<Vec<_>>();
    styled_with(div(), TextRole::Ui.style(theme), theme)
        .text_color(theme.colors.text_secondary)
        .child(
            StyledText::new(text)
                .with_highlights(highlights)
                .with_font_family_overrides(families),
        )
        .into_any_element()
}

/// Renders the help overlay (§3.8.7).
pub(crate) fn render(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight) = {
        let theme = cx.theme();
        (theme.space.lg, theme.space.xxs)
    };
    let scroll = help_scroll(state, cx);
    let paragraph = what_keeps_running(cx);
    let clipboard = key_paragraph_view(TERMINAL_CLIPBOARD, cx);
    let app = state.read(cx);
    let active_group = if app.agent_popup.is_some() {
        Some("Agent popup (^s)")
    } else if matches!(app.screen, Screen::Workspace { .. }) {
        Some("Terminal (^s)")
    } else {
        None
    };
    let (version, uptime) = app.snapshot.as_ref().map_or_else(
        || ("\u{2013}".to_owned(), "\u{2013}".to_owned()),
        |snapshot| {
            let started = age_secs(&snapshot.daemon.started_at, now_unix());
            (
                crate::presentation::bare_version(&snapshot.daemon.version).to_owned(),
                started.map_or_else(|| "\u{2013}".to_owned(), fleet_ui_kit::format_age),
            )
        },
    );

    let column_elements =
        prepared_columns(active_group)
            .iter()
            .enumerate()
            .map(|(column_index, groups)| {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(gap)
                    .children(groups.iter().enumerate().map(|(stacked, group)| {
                        // Only the group the caller moved to the front is accented (§3.8.7).
                        let accented = active_group.is_some() && column_index == 0 && stacked == 0;
                        let dimmed = active_group.is_some() && !accented;
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(tight)
                            .child(SectionHeader::new(group.title))
                            .children(group.sections.iter().map(move |section| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    // The sub-head is subordinate to the group head: it names
                                    // the key context these rows are actually true in.
                                    .children(section.title.as_ref().map(|title| {
                                        Text::hint(title.clone()).faint().into_any_element()
                                    }))
                                    .children(section.rows.iter().map(|(keys, label)| {
                                        Row::new()
                                            .dimmed(dimmed)
                                            .column(RowColumn::fixed(
                                                px(KEY_COLUMN),
                                                Text::data_small(keys.clone()),
                                            ))
                                            .column(RowColumn::flex(
                                                Text::ui(label.clone()).muted().ellipsize(),
                                            ))
                                    }))
                            }))
                    }))
            });

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(paragraph)
        .child(clipboard)
        .child(Divider::horizontal())
        .child(
            div()
                .id("keymap-columns")
                .flex()
                .flex_row()
                .items_start()
                .gap(gap)
                .w_full()
                .min_h_0()
                // The whole table is taller than 620 px; the wheel reaches the rest rather
                // than the bottom rows being silently unreachable.
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .children(column_elements),
        );

    let down = scroll.clone();
    let up = scroll.clone();
    let down_state = state.clone();
    let up_state = state.clone();
    root(focus)
        .on_action(move |_: &dialog::CursorDown, _, cx| {
            scroll_by(&down, KEYBOARD_SCROLL_STEP);
            notify(&down_state, cx);
        })
        .on_action(move |_: &dialog::CursorUp, _, cx| {
            scroll_by(&up, -KEYBOARD_SCROLL_STEP);
            notify(&up_state, cx);
        })
        .child(
            Dialog::new("Keymap")
                .icon(Icon::CircleQuestionMark)
                .width(super::Dialogs::Help.width(cx))
                .when_some(super::Dialogs::Help.height(), Dialog::height)
                .body(body)
                .hint_row(
                    KeyHintRow::new()
                        .key("↑/↓", "scroll")
                        .key("?", "close")
                        .key("esc", "close"),
                )
                .primary(format!(
                    "Fleet {version} \u{00b7} protocol {} \u{00b7} fleetd up {uptime}",
                    protocol()
                )),
        )
        .into_any_element()
}

fn what_keeps_running(cx: &App) -> AnyElement {
    key_paragraph_view(WHAT_KEEPS_RUNNING, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_watch_visibility_dismissal_and_navigation_keys() {
        let groups = groups();
        let prefix = groups
            .iter()
            .flat_map(|g| &g.sections)
            .find(|s| s.context == "Workspace > Prefix")
            .unwrap();
        assert!(
            prefix
                .rows
                .iter()
                .any(|(key, action)| key == "v" && action == "toggle watch pane")
        );
        assert!(
            prefix
                .rows
                .iter()
                .any(|(key, action)| key == "V" && action == "dismiss watch")
        );
        for (key, action) in [("N", "next watch"), ("P", "prev watch")] {
            assert!(
                prefix
                    .rows
                    .iter()
                    .any(|row| row == &(key.into(), action.into()))
            );
        }
    }

    #[test]
    fn every_group_has_rows_and_no_binding_is_orphaned() {
        let groups = groups();
        assert_eq!(groups.len(), GROUPS.len());
        for group in &groups {
            assert!(group.row_count() > 0, "{} is empty", group.title);
        }
        let covered: usize = groups.iter().map(Group::row_count).sum();
        assert!(covered > 100, "the help lost rows: {covered}");
    }

    #[test]
    fn the_six_groups_are_laid_over_three_balanced_columns() {
        let packed = columns(groups());
        assert_eq!(packed.len(), COLUMNS);
        assert!(
            packed.iter().all(|column| !column.is_empty()),
            "an empty column wastes a third of an 880 px card"
        );
        let flattened: Vec<&'static str> = packed
            .iter()
            .flat_map(|column| column.iter().map(|group| group.title))
            .collect();
        let expected: Vec<&'static str> = groups().iter().map(|group| group.title).collect();
        assert_eq!(flattened, expected, "reading order is preserved");

        let heights: Vec<usize> = packed
            .iter()
            .map(|column| column.iter().map(Group::height).sum::<usize>())
            .collect();
        let tallest = heights.iter().max().copied().unwrap_or(0);
        let total: usize = heights.iter().sum();
        assert!(
            tallest * 2 <= total,
            "one column must never hold more than half the table: {heights:?}"
        );
    }

    #[test]
    fn fewer_groups_than_columns_still_fill_one_column_each() {
        let section = |context, label: &str| Section {
            context,
            title: None,
            rows: vec![("k".into(), label.to_owned().into())],
        };
        let small = vec![
            Group {
                title: "a",
                sections: vec![section("Fleet", "one")],
            },
            Group {
                title: "b",
                sections: vec![section("Hub", "two")],
            },
        ];
        let packed = columns(small);
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0][0].title, "a");
        assert_eq!(packed[1][0].title, "b");
    }

    #[test]
    fn repeated_actions_merge_their_keys_into_one_row() {
        let hub = groups()
            .into_iter()
            .find(|group| group.title == "Hub")
            .unwrap_or_else(|| panic!("no Hub column"));
        let move_down = hub
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .find(|(_, label)| label == "move down")
            .unwrap_or_else(|| panic!("no move-down row"));
        assert!(move_down.0.contains('/'), "j and down must share a row");
    }

    #[test]
    fn a_row_never_merges_keys_from_two_key_contexts() {
        // KM-03: `esc / q / ? close` and `⏎ / y / Y accept` were rows the old label-only merge
        // produced out of mutually exclusive contexts, and they are true in none of them.
        let table = keymap::table();
        for group in groups() {
            for section in &group.sections {
                for (keys, label) in &section.rows {
                    for key in keys.split(" / ") {
                        assert!(
                            table.iter().any(|spec| spec.context == section.context
                                && pretty_keys(spec.keys) == key
                                && humanize(spec.action).as_str() == label.as_ref()),
                            "`{key} {label}` is not bound in `{}`",
                            section.context
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_palette_is_closed_by_escape_alone() {
        let dialogs = groups()
            .into_iter()
            .find(|group| group.title == "Dialogs & filter")
            .unwrap_or_else(|| panic!("no dialogs column"));
        let palette = dialogs
            .sections
            .iter()
            .find(|section| section.context == "Palette")
            .unwrap_or_else(|| panic!("no palette section"));
        assert_eq!(palette.title.as_deref(), Some("palette"));
        for (keys, label) in &palette.rows {
            assert!(
                !keys.split(" / ").any(|key| key == "q"),
                "`q` stays a printable query character (`{keys} {label}`)"
            );
        }
    }

    #[test]
    fn the_paragraph_renders_key_names_and_never_backticks() {
        let (text, keys) = key_paragraph(WHAT_KEEPS_RUNNING);
        assert!(!text.contains('`'), "backticks are markup, not characters");
        let named: Vec<&str> = keys.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(named, vec!["ctrl-q", "c", "K", "ctrl-shift-q"]);
        assert!(text.starts_with("What keeps running."));
        assert!(text.ends_with("Terminals survive a daemon restart and reattach on their own."));

        // Degenerate inputs stay printable rather than panicking or eating text.
        assert_eq!(key_paragraph("plain"), ("plain".to_owned(), Vec::new()));
        assert_eq!(
            key_paragraph("a `b"),
            ("a `b".to_owned(), Vec::new()),
            "an unpaired backtick is a character"
        );
    }

    #[test]
    fn a_group_that_covers_one_context_needs_no_sub_head() {
        let scroll = groups()
            .into_iter()
            .find(|group| group.title == "Scroll")
            .unwrap_or_else(|| panic!("no scroll column"));
        assert_eq!(scroll.sections.len(), 1);
        assert_eq!(scroll.sections[0].title, None);
        assert_eq!(context_label("Dialog > QuitDaemon"), "quit daemon");
        assert_eq!(context_label("Fleet"), "fleet");
    }

    #[test]
    fn action_names_become_readable_labels() {
        assert_eq!(humanize("hub::MoveDown"), "move down");
        assert_eq!(humanize("fleet::QuitAndStopDaemon"), "quit and stop daemon");
        assert_eq!(humanize("Bare"), "bare");
    }

    #[test]
    fn keystrokes_are_shortened_for_the_mono_column() {
        assert_eq!(pretty_keys("ctrl-n"), "^n");
        assert_eq!(pretty_keys("shift-tab"), "S-\u{21e5}");
        assert_eq!(pretty_keys("g g"), "g g");
        assert_eq!(pretty_keys("escape"), "esc");
    }

    #[test]
    fn every_context_of_the_keymap_lands_in_a_column() {
        let claimed: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, contexts)| contexts.iter().copied())
            .collect();
        for spec in keymap::table() {
            let daemon_or_first_run =
                spec.context.starts_with("Daemon") || spec.context == "FirstRun";
            assert!(
                claimed.contains(&spec.context) || daemon_or_first_run,
                "`{}` is documented nowhere",
                spec.context
            );
        }
    }

    #[test]
    fn keyboard_reaches_last_help_row() {
        let mut offset = 0.0;
        let maximum = 913.0;
        for _ in 0..100 {
            offset = next_scroll_offset(offset, maximum, KEYBOARD_SCROLL_STEP);
        }
        assert_eq!(offset, -maximum);
        assert_eq!(
            next_scroll_offset(offset, maximum, -KEYBOARD_SCROLL_STEP),
            -maximum + KEYBOARD_SCROLL_STEP
        );
    }
}
