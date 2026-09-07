use std::collections::BTreeMap;
#[test]
fn saturated_wheel_keeps_only_fractional_rows() {
    use fleet_core::ids::TerminalId;
    use gpui::{ScrollDelta, TouchPhase, point};
    for (delta, expected) in [(f32::MAX, i32::MIN), (-f32::MAX, i32::MAX)] {
        let mut acc = WheelAccumulator::default();
        assert_eq!(
            acc.steps(
                TerminalId(1),
                ScrollDelta::Lines(point(0.0, delta)),
                px(20.0),
                50,
                TouchPhase::Moved
            ),
            expected
        );
        assert!(acc.remainder.abs() < 1.0);
        assert_eq!(
            acc.steps(
                TerminalId(1),
                ScrollDelta::Lines(point(0.0, 0.0)),
                px(20.0),
                50,
                TouchPhase::Moved
            ),
            0
        );
    }
}

#[test]
fn focus_changes_reset_wheel_without_pointer_motion() {
    use fleet_core::ids::TerminalId;
    use gpui::{ScrollDelta, TouchPhase, point};
    let mut acc = WheelAccumulator::default();
    for next in [Some(TerminalId(2)), None] {
        acc.steps(
            TerminalId(1),
            ScrollDelta::Lines(point(0.0, 0.25)),
            px(20.0),
            3,
            TouchPhase::Moved,
        );
        acc.reconcile(Some(TerminalId(1)));
        assert_eq!(acc.remainder, -0.75, "unchanged focus preserves momentum");
        acc.reconcile(next);
        acc.reconcile(Some(TerminalId(1)));
        assert_eq!(
            acc.remainder, 0.0,
            "switching away and back clears momentum"
        );
    }
}

#[test]
fn wheel_accumulator_preserves_fractional_rows_and_reversals() {
    use fleet_core::ids::TerminalId;
    use gpui::{ScrollDelta, TouchPhase, point};
    let mut acc = WheelAccumulator::default();
    let mut pixel = |delta| {
        acc.steps(
            TerminalId(1),
            ScrollDelta::Pixels(point(px(0.0), px(delta))),
            px(20.0),
            3,
            TouchPhase::Moved,
        )
    };
    assert_eq!(pixel(5.0), 0);
    assert_eq!(pixel(10.0), 0);
    assert_eq!(pixel(10.0), -1); // -0.25 remains
    assert_eq!(pixel(-10.0), 0); // reverse to +0.25
    assert_eq!(pixel(-20.0), 1); // +0.25 remains
    assert_eq!(pixel(-15.0), 1); // exact row
    assert_eq!(acc.remainder, 0.0);
}

#[test]
fn wheel_lines_pixels_and_terminal_phase_resets() {
    use fleet_core::ids::TerminalId;
    use gpui::{ScrollDelta, TouchPhase, point};
    let mut acc = WheelAccumulator::default();
    let lines = |y| ScrollDelta::Lines(point(0.0, y));
    assert_eq!(
        acc.steps(TerminalId(1), lines(0.5), px(20.0), 3, TouchPhase::Moved),
        -1
    );
    assert_eq!(
        acc.steps(
            TerminalId(1),
            ScrollDelta::Pixels(point(px(0.0), px(10.0))),
            px(20.0),
            3,
            TouchPhase::Moved
        ),
        -1
    );
    for phase in [TouchPhase::Ended, TouchPhase::Cancelled] {
        assert_eq!(acc.steps(TerminalId(1), lines(0.25), px(20.0), 3, phase), 0);
        assert_eq!(acc.remainder, 0.0);
    }
    acc.steps(TerminalId(1), lines(0.25), px(20.0), 3, TouchPhase::Moved);
    acc.point_at(TerminalId(2));
    assert_eq!(
        acc.steps(TerminalId(2), lines(0.25), px(20.0), 3, TouchPhase::Moved),
        0
    );
    assert_eq!(
        acc.steps(TerminalId(1), lines(0.25), px(20.0), 3, TouchPhase::Moved),
        0
    );
}

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

fn wide_cell(text: &str) -> ProtoCell {
    ProtoCell {
        width: ProtoWidth::Wide,
        ..cell(text)
    }
}

fn spacer() -> ProtoCell {
    ProtoCell {
        width: ProtoWidth::Spacer,
        ..cell("")
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
        shift: None,
        rows_changed: rows
            .iter()
            .enumerate()
            .map(|(index, row)| RowUpdate {
                index: index as u16,
                cells: row.chars().map(|c| cell(&c.to_string())).collect(),
                wrapped: false,
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
            history_epoch: 0,
        },
        modes: TerminalModes::default(),
        title: None,
    });
    grid
}

fn cache_row(text: &str, wrapped: bool) -> CachedGridRow {
    let mut grid = grid_with(&[text]);
    grid.wrapped[0] = wrapped;
    cached_grid_row(&grid, 0).unwrap_or_else(|| panic!("fixture row must exist"))
}

#[test]
fn grid_size_subtracts_the_padding_on_both_axes() {
    let cell = gpui::size(px(10.0), px(20.0));
    // 216 px wide minus 2 × 8 px of padding is exactly 20 cells.
    let (cols, rows) = grid_size(
        gpui::size(px(216.0), px(416.0)),
        cell,
        fleet_ui_kit::theme::Spacing::default().sm,
    );
    assert_eq!((cols, rows), (20, 20));
}

#[test]
fn grid_size_never_returns_an_empty_pty() {
    let cell = gpui::size(px(10.0), px(20.0));
    let (cols, rows) = grid_size(
        gpui::size(px(0.0), px(0.0)),
        cell,
        fleet_ui_kit::theme::Spacing::default().sm,
    );
    assert_eq!((cols, rows), (MIN_COLS, MIN_ROWS));
}

#[test]
fn grid_size_survives_a_degenerate_cell() {
    let (cols, rows) = grid_size(
        gpui::size(px(100.0), px(100.0)),
        gpui::size(px(0.0), px(0.0)),
        fleet_ui_kit::theme::Spacing::default().sm,
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
fn cell_selection_is_inclusive_and_order_independent() {
    let forward = cell_selection(CellPoint::new(1, 2), CellPoint::new(2, 4));
    let backward = cell_selection(CellPoint::new(2, 4), CellPoint::new(1, 2));
    assert_eq!(forward, backward);
    assert_eq!(
        forward,
        GridSelection::new(1, 2, 2, 5),
        "the head cell is included by the half-open painted range"
    );
}

#[test]
fn absolute_cell_selection_clips_at_both_viewport_edges() {
    let grid = grid_scrolled(&["aaaaaa", "bbbbbb", "cccccc"], 110, 10);
    assert_eq!(viewport_base(&grid), 100);

    assert_eq!(
        viewport_cell_selection(
            &grid,
            AbsoluteCellPoint::new(99, 4),
            AbsoluteCellPoint::new(101, 2),
        ),
        Some(GridSelection::new(0, 0, 1, 3)),
        "an anchor above the viewport clips to its first cell"
    );
    assert_eq!(
        viewport_cell_selection(
            &grid,
            AbsoluteCellPoint::new(101, 2),
            AbsoluteCellPoint::new(103, 4),
        ),
        Some(GridSelection::new(1, 2, 2, 6)),
        "a head below the viewport clips to its last cell"
    );
    assert_eq!(
        viewport_cell_selection(
            &grid,
            AbsoluteCellPoint::new(103, 4),
            AbsoluteCellPoint::new(101, 2),
        ),
        Some(GridSelection::new(1, 2, 2, 6)),
        "backwards selections clip identically"
    );
}

#[test]
fn absolute_cell_selection_survives_a_viewport_base_shift() {
    let anchor = AbsoluteCellPoint::new(101, 1);
    let head = AbsoluteCellPoint::new(102, 3);
    let before = grid_scrolled(&["aaaaaa", "bbbbbb", "cccccc"], 110, 10);
    let after = grid_scrolled(&["bbbbbb", "cccccc", "dddddd"], 111, 10);

    assert_eq!(
        viewport_cell_selection(&before, anchor, head),
        Some(GridSelection::new(1, 1, 2, 4))
    );
    assert_eq!(
        viewport_cell_selection(&after, anchor, head),
        Some(GridSelection::new(0, 1, 1, 4)),
        "the same absolute endpoints move up when output advances the viewport"
    );
}

#[test]
fn selection_and_copy_follow_wrapped_rows_through_a_shift_frame() {
    let mut grid = grid_scrolled(&["aaaaaa", "bbbbbb", "cccccc"], 110, 10);
    grid.wrapped[1] = true;
    let selection = AbsoluteCellSelection::new(
        AbsoluteCellPoint::new(101, 1),
        AbsoluteCellPoint::new(102, 3),
    );
    let copied = absolute_selection_text(&grid, &BTreeMap::new(), selection);
    assert_eq!(copied.as_deref(), Some("bbbbbcccc"));
    let shifted = fleet_proto::terminal::FrameUpdate {
        terminal: fleet_core::ids::TerminalId(1),
        seq: 2,
        cols: grid.cols,
        rows: grid.rows,
        full: false,
        shift: Some(1),
        rows_changed: vec![RowUpdate {
            index: 2,
            cells: "dddddd".chars().map(|c| cell(&c.to_string())).collect(),
            wrapped: false,
        }],
        cursor: grid.cursor,
        viewport: ViewportInfo {
            offset: 9,
            ..grid.viewport
        },
        modes: grid.modes,
        title: None,
    };
    assert!(grid.apply(&shifted));
    assert_eq!(
        viewport_cell_selection(&grid, selection.start, selection.end),
        Some(GridSelection::new(0, 1, 1, 4)),
    );
    assert_eq!(
        absolute_selection_text(&grid, &BTreeMap::new(), selection),
        copied
    );
}

#[test]
fn off_screen_absolute_cell_selection_paints_nothing_without_losing_its_endpoints() {
    let grid = grid_scrolled(&["aaaaaa", "bbbbbb", "cccccc"], 110, 10);
    let selection = (AbsoluteCellPoint::new(90, 1), AbsoluteCellPoint::new(91, 3));

    assert_eq!(
        viewport_cell_selection(&grid, selection.0, selection.1),
        None
    );
    assert_eq!(selection.0, AbsoluteCellPoint::new(90, 1));
    assert_eq!(selection.1, AbsoluteCellPoint::new(91, 3));
}

#[test]
fn word_selection_stops_at_character_classes() {
    let grid = grid_with(&["one_two:: three"]);
    assert_eq!(
        word_selection(&grid, CellPoint::new(0, 4)),
        Some(GridSelection::new(0, 0, 0, 7))
    );
    assert_eq!(
        word_selection(&grid, CellPoint::new(0, 7)),
        Some(GridSelection::new(0, 7, 0, 9))
    );
    assert_eq!(
        word_selection(&grid, CellPoint::new(0, 9)),
        Some(GridSelection::new(0, 9, 0, 10))
    );
}

#[test]
fn word_drag_ignores_jitter_then_extends_to_the_hovered_word_boundary() {
    let grid = grid_with(&["one two"]);
    let initiating = AbsoluteCellPoint::new(0, 1);
    let initial = absolute_selection_at(&grid, CellPoint::new(0, 1), SelectionGranularity::Word)
        .unwrap_or_else(|| panic!("word missing"));

    assert_eq!(
        extend_absolute_selection(
            &grid,
            initial,
            initiating,
            CellPoint::new(0, 1),
            SelectionGranularity::Word,
        ),
        Some(initial)
    );
    assert_eq!(
        extend_absolute_selection(
            &grid,
            initial,
            initiating,
            CellPoint::new(0, 5),
            SelectionGranularity::Word,
        ),
        Some(AbsoluteCellSelection::new(
            AbsoluteCellPoint::new(0, 0),
            AbsoluteCellPoint::new(0, 6),
        ))
    );
}

#[test]
fn line_drag_extends_by_whole_visual_rows() {
    let grid = grid_with(&["aaaa", "bbbb", "cccc"]);
    let initiating = AbsoluteCellPoint::new(1, 2);
    let initial = absolute_selection_at(&grid, CellPoint::new(1, 2), SelectionGranularity::Line)
        .unwrap_or_else(|| panic!("line missing"));

    assert_eq!(
        extend_absolute_selection(
            &grid,
            initial,
            initiating,
            CellPoint::new(2, 1),
            SelectionGranularity::Line,
        ),
        Some(AbsoluteCellSelection::new(
            AbsoluteCellPoint::new(1, 0),
            AbsoluteCellPoint::new(2, 3),
        ))
    );
}

#[test]
fn mouse_position_maps_through_grid_padding_and_clamps() {
    let bounds = Bounds::new(
        gpui::point(px(100.0), px(200.0)),
        gpui::size(px(116.0), px(76.0)),
    );
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(
        cell_at_position(
            bounds,
            gpui::point(px(133.0), px(249.0)),
            cell,
            10,
            3,
            fleet_ui_kit::theme::Spacing::default().sm
        ),
        Some(CellPoint::new(2, 2))
    );
    assert_eq!(
        cell_at_position(
            bounds,
            gpui::point(px(100.0), px(200.0)),
            cell,
            10,
            3,
            fleet_ui_kit::theme::Spacing::default().sm
        ),
        Some(CellPoint::new(0, 0)),
        "padding maps to the nearest grid cell"
    );
}

#[test]
fn grid_selection_text_orders_rows_and_trims_each_one() {
    let mut grid = MirrorGrid::new(6, 2);
    grid.lines = vec![
        "abcdef".chars().map(|c| cell(&c.to_string())).collect(),
        "gh    ".chars().map(|c| cell(&c.to_string())).collect(),
    ];
    let selection = cell_selection(CellPoint::new(1, 1), CellPoint::new(0, 2));
    assert_eq!(grid_selection_text(&grid, selection), "cdef\ngh");
}

#[test]
fn grid_selection_text_joins_three_soft_wrapped_rows_as_one_line() {
    let mut grid = MirrorGrid::new(4, 3);
    grid.lines = ["abc ", "def ", "ghi "]
        .map(|row| row.chars().map(|c| cell(&c.to_string())).collect())
        .to_vec();
    grid.wrapped = vec![true, true, false];

    assert_eq!(
        grid_selection_text(&grid, GridSelection::new(0, 0, 2, 4)),
        "abc def ghi"
    );
}

#[test]
fn grid_selection_text_emits_a_wide_cell_once() {
    let mut grid = MirrorGrid::new(6, 1);
    grid.lines = vec![vec![
        cell("a"),
        wide_cell("漢"),
        spacer(),
        cell("b"),
        cell(" "),
        cell(" "),
    ]];
    assert_eq!(
        grid_selection_text(&grid, GridSelection::new(0, 0, 0, 6)),
        "a漢b"
    );
    assert_eq!(
        grid_selection_text(&grid, GridSelection::new(0, 2, 0, 3)),
        "漢",
        "selecting the second half of a wide cell still emits one grapheme"
    );
}

#[test]
fn absolute_selection_uses_cache_for_the_offscreen_prefix() {
    let grid = grid_scrolled(&["ef  ", "gh  "], 11, 0);
    let cache = BTreeMap::from([(10, cache_row("abcd", true))]);
    let selection =
        AbsoluteCellSelection::new(AbsoluteCellPoint::new(10, 1), AbsoluteCellPoint::new(12, 3));

    assert_eq!(
        absolute_selection_text(&grid, &cache, selection),
        Some("bcdef\ngh".to_owned())
    );
}

#[test]
fn absolute_selection_can_copy_fully_offscreen_cached_rows() {
    let grid = grid_scrolled(&["live"], 20, 0);
    let cache = BTreeMap::from([
        (10, cache_row("abcd", false)),
        (11, cache_row("efgh", false)),
    ]);
    let selection =
        AbsoluteCellSelection::new(AbsoluteCellPoint::new(10, 1), AbsoluteCellPoint::new(11, 1));

    assert_eq!(
        absolute_selection_text(&grid, &cache, selection),
        Some("bcd\nef".to_owned())
    );
}

#[test]
fn absolute_selection_fails_instead_of_truncating_a_missing_row() {
    let grid = grid_scrolled(&["live"], 20, 0);
    let cache = BTreeMap::from([
        (10, cache_row("abcd", false)),
        (12, cache_row("ijkl", false)),
    ]);
    let selection =
        AbsoluteCellSelection::new(AbsoluteCellPoint::new(10, 0), AbsoluteCellPoint::new(12, 3));

    assert_eq!(absolute_selection_text(&grid, &cache, selection), None);
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

// Exercise the production absolute extraction path with viewport-relative fixtures.
fn grid_selection_text(grid: &MirrorGrid, selection: GridSelection) -> String {
    let selection = selection.normalized();
    let base = viewport_base(grid);
    absolute_selection_text(
        grid,
        &BTreeMap::new(),
        AbsoluteCellSelection::new(
            AbsoluteCellPoint::new(base + selection.start_row as u64, selection.start_col),
            AbsoluteCellPoint::new(
                base + selection.end_row as u64,
                selection.end_col.saturating_sub(1),
            ),
        ),
    )
    .unwrap_or_default()
}

#[test]
fn explicit_padding_is_shared_by_size_and_pointer_geometry() {
    let bounds = Bounds::new(
        gpui::point(px(100.0), px(200.0)),
        gpui::size(px(216.0), px(416.0)),
    );
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(grid_size(bounds.size, cell, px(16.0)), (18, 19));
    assert_eq!(
        cell_at_position(
            bounds,
            gpui::point(px(126.0), px(236.0)),
            cell,
            18,
            19,
            px(16.0)
        ),
        Some(CellPoint::new(1, 1))
    );
}
