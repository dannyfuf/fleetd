//! GPUI element that paints the mirrored terminal cell grid.
//!
//! The painting itself belongs to [`fleet_ui_kit::TerminalGrid`]; what lives here is the
//! translation between the daemon's wire model and the kit's, plus the geometry the Workspace
//! needs in order to tell the daemon how big its PTY should be.
//!
//! Three things happen on the way from a [`MirrorGrid`] to a painted frame:
//!
//! 1. **Colors are resolved.** `fleet-ui-kit` has no domain dependency, so a `Palette(u8)` is
//!    resolved through [`fleet_ui_kit::theme::TerminalPalette::color`] and a `Default` through
//!    the palette's `foreground` / `background` here, once per cell.
//! 2. **Attributes are unpacked.** The proto keeps ten VT flags in a bitfield; the kit keeps
//!    them as named fields, because `INVERSE` and `DIM` decide whether `nvim`'s status line and
//!    `fzf`'s selection are legible at all.
//! 3. **The area is measured.** The PTY size is a function of the painted area, not of the
//!    window: [`measure`] reports the terminal area's pixel bounds every frame and
//!    [`grid_size`] turns them into the `cols × rows` the daemon is asked to resize to.
//!
//! Everything except [`measure`] is a pure function of its arguments and is unit tested.

use std::collections::BTreeMap;

use fleet_proto::terminal::{
    Cell as ProtoCell, CellAttrs, CellWidth as ProtoWidth, Color, CursorShape as ProtoShape,
    TerminalModes,
};
use fleet_ui_kit::{
    CellWidth, CursorShape, GridCell, GridCursor, GridRow, GridSelection,
    TerminalMode as KitTerminalMode, Theme, UnderlineStyle,
};
use gpui::{Hsla, IntoElement, Pixels, Rgba, Size, canvas, prelude::*, px};

use crate::state::MirrorGrid;

/// The inner padding [`fleet_ui_kit::TerminalGrid`] paints with (§3.6: 8 px, no border).
///
/// It is subtracted on both axes before the area is divided into cells, so the grid the daemon
/// renders is exactly the grid that fits.
pub const GRID_PADDING: f32 = 8.0;

/// The smallest grid Fleet ever asks a PTY for.
///
/// A zero-sized PTY is not a valid `TIOCSWINSZ` argument and a one-column one makes every
/// full-screen app misbehave, so a window squeezed below the minimum still gets a usable shell.
pub const MIN_COLS: u16 = 2;
/// The smallest row count Fleet ever asks a PTY for. See [`MIN_COLS`].
pub const MIN_ROWS: u16 = 1;

/// The grid size that fits `area`, in cells.
///
/// `cell` is `theme.metrics.cell_w` × `theme.metrics.cell_h`. The result is clamped to
/// [`MIN_COLS`] × [`MIN_ROWS`] so a collapsed window never asks the daemon for an empty PTY.
#[must_use]
pub fn grid_size(area: Size<Pixels>, cell: Size<Pixels>) -> (u16, u16) {
    let usable = |extent: Pixels| f32::from(extent) - 2.0 * GRID_PADDING;
    let fit = |extent: f32, unit: Pixels| {
        let unit = f32::from(unit);
        if unit <= 0.0 || !extent.is_finite() || extent <= 0.0 {
            return 0;
        }
        // `as` saturates at u16::MAX for a NaN-free positive float, and the floor is what fits.
        (extent / unit).floor().max(0.0).min(f32::from(u16::MAX)) as u16
    };
    (
        fit(usable(area.width), cell.width).max(MIN_COLS),
        fit(usable(area.height), cell.height).max(MIN_ROWS),
    )
}

/// An invisible element that reports the pixel bounds it was laid out into.
///
/// The Workspace fills the terminal area with it and turns the reported size into a
/// `ResizeTerminal` request. It draws nothing: only the layout pass is interesting.
pub fn measure(report: impl 'static + FnOnce(Size<Pixels>)) -> impl IntoElement {
    canvas(
        move |bounds, _window, _cx| report(bounds.size),
        |_bounds, (), _window, _cx| {},
    )
    .absolute()
    .size_full()
}

/// The kit badges a `FrameUpdate`'s VT modes map onto (§3.6, DESIGN-SYSTEM "Terminal modes").
///
/// `TerminalModes` is zero-suppressed, so a plain shell produces an empty list and no badges.
/// The Kitty keyboard flags and focus-event reporting have no badge: neither changes what a
/// documented Fleet key does, which is the bar the badge row is drawn to.
#[must_use]
pub fn grid_modes(modes: &TerminalModes) -> Vec<KitTerminalMode> {
    let mut active = Vec::with_capacity(4);
    if modes.alt_screen {
        active.push(KitTerminalMode::AltScreen);
    }
    if modes.mouse_reporting {
        active.push(KitTerminalMode::MouseReporting);
    }
    if modes.bracketed_paste {
        active.push(KitTerminalMode::BracketedPaste);
    }
    if modes.app_cursor_keys {
        active.push(KitTerminalMode::ApplicationCursor);
    }
    active
}

/// Resolves a foreground color against the theme's terminal palette.
#[must_use]
pub fn foreground(color: Color, theme: &Theme) -> Hsla {
    match color {
        Color::Default => theme.terminal.foreground,
        Color::Palette(index) => theme.terminal.color(index),
        Color::Rgb { r, g, b } => rgb(r, g, b),
    }
}

/// Resolves a background color, mapping `Default` to "paint nothing" as the kit expects.
#[must_use]
pub fn background(color: Color, theme: &Theme) -> Option<Hsla> {
    match color {
        Color::Default => None,
        Color::Palette(index) => Some(theme.terminal.color(index)),
        Color::Rgb { r, g, b } => Some(rgb(r, g, b)),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    Hsla::from(Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    })
}

/// The underline style the three mutually exclusive underline flags encode.
#[must_use]
pub fn underline_style(attrs: CellAttrs) -> UnderlineStyle {
    if attrs.contains(CellAttrs::CURLY_UNDERLINE) {
        UnderlineStyle::Curly
    } else if attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
        UnderlineStyle::Double
    } else if attrs.contains(CellAttrs::UNDERLINE) {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    }
}

/// Converts one wire cell into the kit's painted cell.
#[must_use]
pub fn grid_cell(cell: &ProtoCell, theme: &Theme) -> GridCell {
    let attrs = cell.attrs;
    GridCell {
        text: cell.text.as_str().to_owned().into(),
        fg: foreground(cell.fg, theme),
        bg: background(cell.bg, theme),
        bold: attrs.contains(CellAttrs::BOLD),
        dim: attrs.contains(CellAttrs::DIM),
        italic: attrs.contains(CellAttrs::ITALIC),
        underline: underline_style(attrs),
        underline_color: cell
            .underline_color
            .and_then(|color| background(color, theme)),
        strikethrough: attrs.contains(CellAttrs::STRIKETHROUGH),
        inverse: attrs.contains(CellAttrs::INVERSE),
        blink: attrs.contains(CellAttrs::BLINK),
        invisible: attrs.contains(CellAttrs::INVISIBLE),
        width: match cell.width {
            ProtoWidth::Narrow => CellWidth::Narrow,
            ProtoWidth::Wide => CellWidth::Wide,
            ProtoWidth::Spacer => CellWidth::Spacer,
        },
    }
}

/// Converts the whole mirror into painted rows.
///
/// A row the daemon has never sent is painted as an empty row rather than as blanks: the kit
/// lays rows out at a fixed cell height, so an empty row already occupies the right space and
/// costs no text shaping.
#[must_use]
pub fn grid_rows(grid: &MirrorGrid, theme: &Theme) -> Vec<GridRow> {
    grid.lines
        .iter()
        .map(|line| GridRow::new(line.iter().map(|cell| grid_cell(cell, theme))))
        .collect()
}

/// The kit cursor for a mirror grid.
///
/// `focused` is the app's own focus, not the PTY's: an unfocused terminal draws the hollow
/// cursor so a window that is not taking keys cannot look like one that is.
#[must_use]
pub fn grid_cursor(grid: &MirrorGrid, focused: bool) -> GridCursor {
    GridCursor {
        row: grid.cursor.row as usize,
        col: grid.cursor.col as usize,
        visible: grid.cursor.visible,
        shape: if focused {
            match grid.cursor.shape {
                ProtoShape::Block => CursorShape::Block,
                ProtoShape::Bar => CursorShape::Bar,
                ProtoShape::Underline => CursorShape::Underline,
            }
        } else {
            CursorShape::Hollow
        },
    }
}

/// The absolute scrollback line the viewport's top row is showing.
///
/// Line 0 is the oldest row the daemon still holds. `ViewportInfo::scrollback_len` counts the
/// history *above* the screen and `offset` how far back the viewport was pulled, so the top row
/// sits at `scrollback_len - offset`.
///
/// Scroll-mode selections are anchored in this space and never in viewport rows: a viewport row
/// number means a different line after every scroll, so an anchor expressed that way silently
/// re-points at whatever moved under it — which is why a `v`, `PageUp`, `y` used to yank the
/// wrong single line.
#[must_use]
pub fn viewport_base(grid: &MirrorGrid) -> u64 {
    let scrollback = grid.viewport.scrollback_len as u64;
    let offset = grid.viewport.offset as u64;
    scrollback.saturating_sub(offset)
}

/// The last absolute line the viewport is showing, or `None` for an empty grid.
#[must_use]
pub fn viewport_last(grid: &MirrorGrid) -> Option<u64> {
    (grid.rows > 0).then(|| viewport_base(grid) + u64::from(grid.rows - 1))
}

/// A line-wise selection between two **absolute** scrollback lines, in either order.
///
/// Fleet's copy mode selects whole lines: `v` anchors a line, movement extends the range and
/// `y` yanks it. A column-wise selection would need a caret the spec does not draw, so the head
/// column is simply the end of the head row.
///
/// The result is in viewport rows, clipped to what is on screen — the selection itself may run
/// far past both edges — and is `None` when none of it is visible.
#[must_use]
pub fn line_selection(grid: &MirrorGrid, anchor: u64, head: u64) -> Option<GridSelection> {
    let base = viewport_base(grid);
    let bottom = viewport_last(grid)?;
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    if last < base || first > bottom {
        return None;
    }
    let start_row = first.max(base) - base;
    let end_row = last.min(bottom) - base;
    let end = grid
        .lines
        .get(usize::try_from(end_row).unwrap_or(usize::MAX))
        .map_or(0, |line| line.iter().map(cell_columns).sum());
    Some(GridSelection::new(
        usize::try_from(start_row).unwrap_or(0),
        0,
        usize::try_from(end_row).unwrap_or(0),
        end,
    ))
}

fn cell_columns(cell: &ProtoCell) -> usize {
    match cell.width {
        ProtoWidth::Narrow => 1,
        ProtoWidth::Wide => 2,
        ProtoWidth::Spacer => 0,
    }
}

/// The text a selection yanks, one line per selected scrollback line.
///
/// `history` is the client's record of every line it has painted since the selection was
/// anchored, keyed by absolute scrollback line. The daemon mirrors only the *viewport*, so a
/// selection that spans more than one screen can only be assembled from what this client saw —
/// which is every line the user scrolled the selection over.
///
/// Trailing blanks are what a terminal pads short rows with; keeping them would paste a
/// rectangle of spaces into the next shell prompt, so they are stripped on the way in.
#[must_use]
pub fn selection_text(history: &BTreeMap<u64, String>, anchor: u64, head: u64) -> String {
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    history
        .range(first..=last)
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The pixel size of one cell, from the theme metrics.
#[must_use]
pub fn cell_size(theme: &Theme) -> Size<Pixels> {
    gpui::size(theme.metrics.cell_w, theme.metrics.cell_h)
}

/// The 2 px amber reminder that `ctrl-s z` hid the header and the tab strip (§3.6).
pub fn zoom_bar(theme: &Theme) -> impl IntoElement {
    gpui::div()
        .h(px(2.0))
        .w_full()
        .flex_none()
        .bg(theme.colors.warning)
}

#[cfg(test)]
mod tests {

    #[test]
    fn frame_modes_become_zero_suppressed_badges() {
        let quiet = TerminalModes::default();
        assert!(
            grid_modes(&quiet).is_empty(),
            "a plain shell costs no badge row"
        );

        let vim = TerminalModes {
            alt_screen: true,
            mouse_reporting: false,
            bracketed_paste: true,
            focus_events: true,
            kitty_keyboard_flags: 1,
            app_cursor_keys: true,
        };
        assert_eq!(
            grid_modes(&vim),
            vec![
                KitTerminalMode::AltScreen,
                KitTerminalMode::BracketedPaste,
                KitTerminalMode::ApplicationCursor,
            ],
            "focus events and Kitty flags change no documented Fleet key, so they get no badge"
        );

        let pager = TerminalModes {
            mouse_reporting: true,
            ..TerminalModes::default()
        };
        assert_eq!(grid_modes(&pager), vec![KitTerminalMode::MouseReporting]);
    }
    use fleet_proto::terminal::{CursorState, RowUpdate, TerminalModes, ViewportInfo};
    use fleet_ui_kit::ThemeMode;

    use super::*;

    fn theme() -> Theme {
        Theme::for_mode(ThemeMode::Dark)
    }

    fn cell(text: &str) -> ProtoCell {
        ProtoCell {
            text: text.into(),
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: CellAttrs::empty(),
            width: ProtoWidth::Narrow,
        }
    }

    fn grid_with(rows: &[&str]) -> MirrorGrid {
        grid_scrolled(rows, 0, 0)
    }

    /// A grid whose viewport sits `scrollback_len - offset` lines into the scrollback.
    fn grid_scrolled(rows: &[&str], scrollback_len: usize, offset: usize) -> MirrorGrid {
        let cols = rows
            .iter()
            .map(|row| row.chars().count())
            .max()
            .unwrap_or(0);
        let mut grid = MirrorGrid::new(cols as u16, rows.len() as u16);
        grid.apply(&fleet_proto::terminal::FrameUpdate {
            terminal: fleet_core::ids::TerminalId(1),
            seq: 1,
            cols: cols as u16,
            rows: rows.len() as u16,
            full: true,
            rows_changed: rows
                .iter()
                .enumerate()
                .map(|(index, row)| RowUpdate {
                    index: index as u16,
                    cells: row.chars().map(|c| cell(&c.to_string())).collect(),
                })
                .collect(),
            cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: ProtoShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len,
                offset,
            },
            modes: TerminalModes::default(),
            title: None,
        });
        grid
    }

    #[test]
    fn grid_size_subtracts_the_padding_on_both_axes() {
        let cell = gpui::size(px(10.0), px(20.0));
        // 216 px wide minus 2 × 8 px of padding is exactly 20 cells.
        let (cols, rows) = grid_size(gpui::size(px(216.0), px(416.0)), cell);
        assert_eq!((cols, rows), (20, 20));
    }

    #[test]
    fn grid_size_never_returns_an_empty_pty() {
        let cell = gpui::size(px(10.0), px(20.0));
        let (cols, rows) = grid_size(gpui::size(px(0.0), px(0.0)), cell);
        assert_eq!((cols, rows), (MIN_COLS, MIN_ROWS));
    }

    #[test]
    fn grid_size_survives_a_degenerate_cell() {
        let (cols, rows) = grid_size(
            gpui::size(px(100.0), px(100.0)),
            gpui::size(px(0.0), px(0.0)),
        );
        assert_eq!((cols, rows), (MIN_COLS, MIN_ROWS));
    }

    #[test]
    fn underline_flags_are_mutually_exclusive_in_priority_order() {
        assert_eq!(underline_style(CellAttrs::empty()), UnderlineStyle::None);
        assert_eq!(
            underline_style(CellAttrs::UNDERLINE),
            UnderlineStyle::Single
        );
        assert_eq!(
            underline_style(CellAttrs::UNDERLINE | CellAttrs::DOUBLE_UNDERLINE),
            UnderlineStyle::Double
        );
        assert_eq!(
            underline_style(CellAttrs::DOUBLE_UNDERLINE | CellAttrs::CURLY_UNDERLINE),
            UnderlineStyle::Curly
        );
    }

    #[test]
    fn default_background_paints_nothing_and_default_foreground_uses_the_palette() {
        let theme = theme();
        assert_eq!(background(Color::Default, &theme), None);
        assert_eq!(
            foreground(Color::Default, &theme),
            theme.terminal.foreground
        );
        assert_eq!(
            foreground(Color::Palette(1), &theme),
            theme.terminal.color(1)
        );
    }

    #[test]
    fn attributes_survive_the_round_trip_into_the_kit() {
        let theme = theme();
        let mut source = cell("x");
        source.attrs = CellAttrs::BOLD | CellAttrs::INVERSE | CellAttrs::DIM;
        source.width = ProtoWidth::Wide;
        let converted = grid_cell(&source, &theme);
        assert!(converted.bold && converted.inverse && converted.dim);
        assert!(!converted.italic);
        assert_eq!(converted.width, CellWidth::Wide);
    }

    #[test]
    fn an_unfocused_terminal_draws_a_hollow_cursor() {
        let grid = grid_with(&["ab"]);
        assert_eq!(grid_cursor(&grid, true).shape, CursorShape::Block);
        assert_eq!(grid_cursor(&grid, false).shape, CursorShape::Hollow);
    }

    #[test]
    fn a_line_selection_covers_whole_rows_in_either_direction() {
        let grid = grid_with(&["one", "two", "three"]);
        let down = line_selection(&grid, 0, 2).unwrap_or_else(|| panic!("off screen"));
        let up = line_selection(&grid, 2, 0).unwrap_or_else(|| panic!("off screen"));
        assert_eq!(down, up);
        assert_eq!((down.start_row, down.start_col), (0, 0));
        assert_eq!((down.end_row, down.end_col), (2, 5));
    }

    #[test]
    fn the_viewport_base_is_where_the_top_row_sits_in_the_scrollback() {
        // 900 lines of history, scrolled back 10: the top row is line 890.
        let grid = grid_scrolled(&["a", "b", "c"], 900, 10);
        assert_eq!(viewport_base(&grid), 890);
        assert_eq!(viewport_last(&grid), Some(892));
        // At the live bottom the viewport starts right after the history.
        let live = grid_scrolled(&["a", "b", "c"], 900, 0);
        assert_eq!(viewport_base(&live), 900);
    }

    #[test]
    fn a_selection_anchored_before_the_viewport_still_paints_what_is_visible() {
        // terminal-008: `v` on line 890, then a page up — the anchor is above the screen now.
        let grid = grid_scrolled(&["d", "e", "f"], 900, 13);
        assert_eq!(viewport_base(&grid), 887);
        let painted = line_selection(&grid, 890, 887).unwrap_or_else(|| panic!("off screen"));
        assert_eq!(
            (painted.start_row, painted.end_row),
            (0, 2),
            "the visible part of the selection is clipped to the viewport, not lost"
        );

        // A selection entirely above or below the viewport paints nothing at all.
        assert_eq!(line_selection(&grid, 400, 401), None);
        assert_eq!(line_selection(&grid, 1_000, 1_001), None);
    }

    #[test]
    fn yanked_text_spans_every_line_the_selection_covers() {
        // terminal-008: the yank reads the client's scrollback record, not the viewport, so a
        // selection that started three pages up still yields all of its lines.
        let mut history = BTreeMap::new();
        for (offset, line) in ["one", "two", "three", "four"].into_iter().enumerate() {
            history.insert(890 + offset as u64, line.to_owned());
        }
        assert_eq!(selection_text(&history, 890, 893), "one\ntwo\nthree\nfour");
        assert_eq!(
            selection_text(&history, 893, 890),
            "one\ntwo\nthree\nfour",
            "the range is the same in either direction"
        );
        assert_eq!(selection_text(&history, 891, 892), "two\nthree");
        assert_eq!(selection_text(&history, 500, 501), "");
    }

    #[test]
    fn rows_are_painted_one_kit_row_per_mirror_row() {
        let theme = theme();
        let grid = grid_with(&["ab", "c"]);
        let rows = grid_rows(&grid, &theme);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].columns(), 2);
        assert_eq!(rows[1].columns(), 1);
    }
}
