//! §3.8.7 Help (`?`) — every context side by side, grouped by mode.
//!
//! The rows are generated from [`crate::keymap::table`], never restated, so a binding and its
//! documentation cannot drift (`docs/APP-CONTRACTS.md` §6). The action label is the action's
//! own name, de-camel-cased; that is what makes the guarantee mechanical.
//!
//! The dialog is context-sensitive: opened from a terminal it puts the `Terminal (^s)` column
//! first and in accent and dims the rest, because that is the only column you can act on.

use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};

use crate::{
    dialogs::{root, uptime_label},
    keymap,
    state::{AppState, Screen},
};

/// The wire protocol this build speaks, for the Settings About section.
#[must_use]
pub const fn protocol() -> u32 {
    fleet_proto::PROTOCOL_VERSION
}

/// The width of the key column (§3.8.7: 68 px mono).
const KEY_COLUMN: f32 = 68.0;

/// The paragraph §3.8.7 calls "the single most valuable paragraph in the app".
pub const WHAT_KEEPS_RUNNING: &str = "What keeps running. Jobs and sessions live in fleetd. \
Closing a dialog, leaving a screen or quitting Fleet (ctrl-q) never stops them. Only `c` in the \
Jobs panel, `K`, and ctrl-shift-q stop things. Terminals do not survive a daemon restart.";

/// One column of the help grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The mode this column documents.
    pub title: &'static str,
    /// Its rows, in registration order.
    pub rows: Vec<(String, String)>,
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
        &["Workspace > Terminal", "Workspace > Prefix"],
    ),
    ("Scroll", &["Workspace > Scroll"]),
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

/// Builds every column from the binding table, merging the keys of a repeated action.
#[must_use]
pub fn groups() -> Vec<Group> {
    let table = keymap::table();
    GROUPS
        .iter()
        .map(|(title, contexts)| {
            let mut rows: Vec<(String, String)> = Vec::new();
            for spec in &table {
                if !contexts.contains(&spec.context) {
                    continue;
                }
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
            Group { title, rows }
        })
        .collect()
}

/// `hub::MoveDown` becomes `move down`.
#[must_use]
pub fn humanize(action: &str) -> String {
    let name = action.rsplit("::").next().unwrap_or(action);
    let mut words = String::new();
    for (index, character) in name.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            words.push(' ');
        }
        words.extend(character.to_lowercase());
    }
    words
}

/// `ctrl-n` becomes `^n`, `shift-tab` becomes `S-⇥`; the mono column is narrow.
#[must_use]
pub fn pretty_keys(keys: &str) -> String {
    keys.split(' ')
        .map(|stroke| {
            stroke
                .replace("ctrl-", "^")
                .replace("shift-", "S-")
                .replace("alt-", "\u{2325}")
                .replace("escape", "esc")
                .replace("enter", "\u{23ce}")
                .replace("tab", "\u{21e5}")
                .replace("space", "\u{2423}")
                .replace("backspace", "\u{232b}")
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    let app = state.read(cx);
    let from_terminal = matches!(app.screen, Screen::Workspace { .. });
    let (version, uptime) = app.snapshot.as_ref().map_or_else(
        || ("\u{2013}".to_owned(), "\u{2013}".to_owned()),
        |snapshot| {
            let started =
                crate::dialogs::age_secs(&snapshot.daemon.started_at, crate::dialogs::now_epoch());
            (
                snapshot.daemon.version.clone(),
                started.map_or_else(|| "\u{2013}".to_owned(), uptime_label),
            )
        },
    );

    let mut columns = groups();
    if from_terminal {
        // §3.8.7: the terminal column comes first and in accent when `?` was pressed there.
        if let Some(index) = columns
            .iter()
            .position(|group| group.title == "Terminal (^s)")
        {
            let terminal = columns.remove(index);
            columns.insert(0, terminal);
        }
    }

    let column_elements = columns.into_iter().enumerate().map(|(index, group)| {
        let accented = from_terminal && index == 0;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(tight)
            .child(SectionHeader::new(group.title))
            .children(group.rows.into_iter().map(|(keys, label)| {
                Row::new()
                    .dimmed(from_terminal && !accented)
                    .column(RowColumn::fixed(px(KEY_COLUMN), Text::data_small(keys)))
                    .column(RowColumn::flex(Text::ui(label).muted().ellipsize()))
            }))
    });

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(Text::ui(WHAT_KEEPS_RUNNING))
        .child(Divider::horizontal())
        .child(
            div()
                .flex()
                .flex_row()
                .gap(gap)
                .w_full()
                .children(column_elements),
        );

    root(focus)
        .child(
            Dialog::new("Keymap")
                .icon(Icon::CircleQuestionMark)
                .width(super::Dialogs::Help.width())
                .height(px(620.0))
                .body(body)
                .hint_row(KeyHintRow::new().key("?", "close").key("esc", "close"))
                .primary(format!(
                    "Fleet {version} \u{00b7} protocol {} \u{00b7} fleetd up {uptime}",
                    protocol()
                )),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_has_rows_and_no_binding_is_orphaned() {
        let groups = groups();
        assert_eq!(groups.len(), GROUPS.len());
        for group in &groups {
            assert!(!group.rows.is_empty(), "{} is empty", group.title);
        }
        let covered: usize = groups.iter().map(|group| group.rows.len()).sum();
        assert!(covered > 100, "the help lost rows: {covered}");
    }

    #[test]
    fn repeated_actions_merge_their_keys_into_one_row() {
        let hub = groups()
            .into_iter()
            .find(|group| group.title == "Hub")
            .unwrap_or_else(|| panic!("no Hub column"));
        let move_down = hub
            .rows
            .iter()
            .find(|(_, label)| label == "move down")
            .unwrap_or_else(|| panic!("no move-down row"));
        assert!(move_down.0.contains('/'), "j and down must share a row");
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
}
