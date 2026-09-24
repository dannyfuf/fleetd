//! A sample ⌃S command menu for the galleries, spelled with [`Kbd::parse`] because the gallery has
//! no keymap. The app builds the same component from its action catalogue and live key table.

use fleet_ui_kit::prelude::*;

/// One row: its key, the last key of a range, and its label.
type SampleRow = (&'static str, Option<&'static str>, &'static str);

/// Columns of rows, the shape the app's catalogue produces.
const COLUMNS: &[(&str, &[SampleRow])] = &[
    (
        "Tabs",
        &[
            ("1", Some("9"), "Go to tab"),
            ("c", None, "New terminal"),
            ("x", None, "Close tab"),
            (",", None, "Rename tab"),
            ("h", None, "Previous tab"),
            ("l", None, "Next tab"),
        ],
    ),
    (
        "Session",
        &[
            ("s", None, "Back to hub"),
            ("S", None, "Sleep, back to hub"),
            ("w", None, "Last session"),
            ("W", None, "Switch session…"),
        ],
    ),
    (
        "Terminal",
        &[
            ("[", None, "Scroll back"),
            ("]", None, "Paste"),
            ("z", None, "Zoom pane"),
            ("r", None, "Restart command"),
        ],
    ),
    (
        "Agents",
        &[
            ("a", None, "New Claude thread"),
            ("A", None, "New Codex thread"),
        ],
    ),
    (
        "Panels",
        &[
            ("b", None, "Board"),
            ("v", None, "Watch pane"),
            ("J", None, "Jobs"),
            ("?", None, "All shortcuts"),
        ],
    ),
];

fn chip(key: &str) -> Kbd {
    Kbd::parse(key).unwrap_or_else(|error| panic!("gallery key {key:?}: {error}"))
}

/// The Workspace menu. `literal` adds the "press the prefix again" note a PTY-backed surface
/// has; `columns` keeps only the first few, for a narrow stage.
pub fn sample(t: &Theme, id: &'static str, literal: bool, columns: usize) -> PrefixMenu {
    let note = div()
        .flex()
        .items_center()
        .gap(t.space.xs)
        .child(Text::caption("Press a key or click."))
        .when(literal, |note| {
            note.child(chip("ctrl-s").size(KbdSize::Small))
                .child(Text::caption("again sends it to the terminal."))
        });
    let close = Button::new((id, 999usize), "Close")
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .kbd(chip("escape"))
        .on_click(|_, _, _| {});
    let mut index = 0usize;
    COLUMNS.iter().take(columns).fold(
        PrefixMenu::new(id, "Fleet commands")
            .prefix(chip("ctrl-s"))
            .note(note)
            .close(close),
        |menu, (title, rows)| {
            let column = rows.iter().fold(
                PrefixMenuColumn::new(*title),
                |column, (key, last, label)| {
                    index += 1;
                    let item =
                        PrefixMenuItem::new((id, index), chip(key), *label).on_click(|_, _, _| {});
                    column.item(match last {
                        Some(last) => item.range_end(chip(last)),
                        None => item,
                    })
                },
            );
            menu.column(column)
        },
    )
}
