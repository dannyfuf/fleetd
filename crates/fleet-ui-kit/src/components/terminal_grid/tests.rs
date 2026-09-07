use std::{rc::Rc, sync::Arc};

use gpui::{point, px, size};

use super::{painter::cell_rect_bounds, *};
use crate::theme::Theme;

fn theme() -> Theme {
    Theme::dark()
}

#[test]
fn spacer_occupies_zero_columns() {
    assert_eq!(CellWidth::Narrow.columns(), 1);
    assert_eq!(CellWidth::Wide.columns(), 2);
    assert_eq!(CellWidth::Spacer.columns(), 0);
}

#[test]
fn selection_spans_are_clipped_per_row() {
    let sel = GridSelection::new(2, 4, 1, 2).normalized();
    assert_eq!((sel.start_row, sel.start_col), (1, 2));
    assert_eq!(sel.span_in_row(0, 80), None);
    assert_eq!(sel.span_in_row(1, 80), Some((2, 80)));
    assert_eq!(sel.span_in_row(2, 80), Some((0, 4)));
    assert_eq!(sel.span_in_row(3, 80), None);
}

#[test]
fn equal_styles_merge_into_one_batch() {
    let t = theme();
    let row = GridRow::new("hello".chars().map(|c| GridCell::new(c.to_string(), &t)));
    let mut out = Vec::new();
    row_text_batches(0, &row, &t, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "hello");
    assert_eq!(out[0].col, 0);
}

#[test]
fn a_style_change_and_a_blank_both_flush_the_batch() {
    let t = theme();
    let row = GridRow::new([
        GridCell::new("a", &t),
        GridCell::new("b", &t).bold(true),
        GridCell::new(" ", &t),
        GridCell::new("c", &t),
    ]);
    let mut out = Vec::new();
    row_text_batches(0, &row, &t, &mut out);
    let batches: Vec<_> = out.iter().map(|b| (b.col, b.text.as_str())).collect();
    assert_eq!(batches, vec![(0, "a"), (1, "b"), (3, "c")]);
}

#[test]
fn a_wide_grapheme_is_its_own_batch_and_advances_two_columns() {
    let t = theme();
    let row = GridRow::new([
        GridCell::new("a", &t),
        GridCell::new("漢", &t).width(CellWidth::Wide),
        GridCell::new("", &t).width(CellWidth::Spacer),
        GridCell::new("b", &t),
    ]);
    let mut out = Vec::new();
    row_text_batches(0, &row, &t, &mut out);
    let batches: Vec<_> = out.iter().map(|b| (b.col, b.text.as_str())).collect();
    assert_eq!(batches, vec![(0, "a"), (1, "漢"), (3, "b")]);
}

#[test]
fn invisible_cells_keep_their_background_and_drop_their_glyph() {
    let t = theme();
    let row = GridRow::new([GridCell::new("x", &t)
        .bg(t.terminal.ansi[1])
        .invisible(true)]);
    let mut text = Vec::new();
    row_text_batches(0, &row, &t, &mut text);
    assert!(text.is_empty());
    let mut bg = Vec::new();
    row_backgrounds(0, &row, &t, &mut bg);
    assert_eq!(bg.len(), 1);
    assert_eq!(bg[0].cols, 1);
}

#[test]
fn a_wide_cell_and_its_spacer_paint_exactly_two_columns() {
    let t = theme();
    let red = t.terminal.ansi[1];
    let row = GridRow::new([
        GridCell::new("\u{6f22}", &t).bg(red).width(CellWidth::Wide),
        GridCell::new("", &t).bg(red).width(CellWidth::Spacer),
        GridCell::new("X", &t).bg(red),
    ]);
    let mut rects = Vec::new();
    row_backgrounds(0, &row, &t, &mut rects);
    assert_eq!(rects.len(), 1, "one colour, one run");
    assert_eq!(
        (rects[0].col, rects[0].cols),
        (0, 3),
        "two columns for the wide cell, one for the narrow one that follows"
    );
}

#[test]
fn a_spacer_never_extends_a_run_past_the_cell_that_follows() {
    let t = theme();
    let red = t.terminal.ansi[1];
    let blue = t.terminal.ansi[4];
    let row = GridRow::new([
        GridCell::new("\u{6f22}", &t).bg(red).width(CellWidth::Wide),
        GridCell::new("", &t).bg(red).width(CellWidth::Spacer),
        GridCell::new("X", &t).bg(blue),
    ]);
    let mut rects = Vec::new();
    row_backgrounds(0, &row, &t, &mut rects);
    let spans: Vec<_> = rects.iter().map(|rect| (rect.col, rect.cols)).collect();
    assert_eq!(
        spans,
        vec![(0, 2), (2, 1)],
        "the runs must tile the row, never overlap"
    );
}

#[test]
fn adjacent_backgrounds_merge_horizontally_then_vertically() {
    let t = theme();
    let red = t.terminal.ansi[1];
    let make = || GridRow::new((0..4).map(|_| GridCell::new("x", &t).bg(red)));
    let mut rects = Vec::new();
    row_backgrounds(0, &make(), &t, &mut rects);
    row_backgrounds(1, &make(), &t, &mut rects);
    assert_eq!(rects.len(), 2);
    let merged = merge_vertically(rects);
    assert_eq!(merged.len(), 1);
    assert_eq!((merged[0].cols, merged[0].rows), (4, 2));
}

#[test]
fn inverse_swaps_the_pair_and_dim_lowers_the_foreground() {
    let t = theme();
    let cell = GridCell::new("x", &t)
        .fg(t.terminal.ansi[2])
        .bg(t.terminal.ansi[4])
        .inverse(true);
    let (fg, bg) = cell.resolve(&t);
    assert_eq!(fg, t.terminal.ansi[4]);
    assert_eq!(bg, Some(t.terminal.ansi[2]));

    let dim = GridCell::new("x", &t).dim(true);
    let (dim_fg, _) = dim.resolve(&t);
    assert!(dim_fg.a < t.terminal.foreground.a);
}

#[test]
fn cell_at_resolves_through_wide_cells() {
    let t = theme();
    let row = GridRow::new([
        GridCell::new("a", &t),
        GridCell::new("漢", &t).width(CellWidth::Wide),
        GridCell::new("", &t).width(CellWidth::Spacer),
        GridCell::new("b", &t),
    ]);
    assert_eq!(
        cell_at(Some(&row), 0).map(|(_, c)| c.text.as_ref()),
        Some("a")
    );
    assert_eq!(
        cell_at(Some(&row), 1).map(|(_, c)| c.text.as_ref()),
        Some("漢")
    );
    assert_eq!(
        cell_at(Some(&row), 2).map(|(_, c)| c.text.as_ref()),
        Some("漢")
    );
    assert_eq!(
        cell_at(Some(&row), 3).map(|(_, c)| c.text.as_ref()),
        Some("b")
    );
    assert_eq!(cell_at(Some(&row), 9), None);
}

#[test]
fn wide_cursor_uses_owning_cell_start() {
    let t = theme();
    let row = GridRow::new([
        GridCell::new("a", &t),
        GridCell::new("漢", &t).width(CellWidth::Wide),
        GridCell::new("", &t).width(CellWidth::Spacer),
    ]);
    let (owner, cell) = cell_at(Some(&row), 2).expect("wide cell");
    assert_eq!(owner, 1);
    assert_eq!(cell.text.as_ref(), "漢");
}

#[test]
fn unfocused_bar_and_underline_become_hollow_blocks() {
    for shape in [CursorShape::Bar, CursorShape::Underline] {
        assert_eq!(normalized_cursor(shape, false), (CursorShape::Block, true));
    }
    assert_eq!(
        normalized_cursor(CursorShape::Bar, true),
        (CursorShape::Bar, false)
    );
}

#[gpui::test]
fn adjacent_fractional_rectangles_share_device_edges(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| gpui::EmptyView))
            .expect("test window")
    });
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, _| {
        window.set_scale_factor(2.0);
        let metrics = CellMetrics {
            width: px(7.5),
            height: px(18.0),
        };
        let color = theme().terminal.background;
        let first = CellRect {
            row: 0,
            rows: 1,
            col: 0,
            cols: 1,
            color,
        };
        let second = CellRect { col: 1, ..first };
        let origin = point(px(0.25), px(0.0));
        let first = cell_rect_bounds(&first, origin, metrics, window);
        let second = cell_rect_bounds(&second, origin, metrics, window);
        assert_eq!(first.right(), second.left());
    });
}

#[test]
fn metrics_fit_whole_cells_only() {
    let metrics = CellMetrics {
        width: px(8.0),
        height: px(18.0),
    };
    assert_eq!(metrics.fit(size(px(83.0), px(55.0))), (10, 3));
    assert_eq!(metrics.fit(size(px(0.0), px(0.0))), (0, 0));
}

#[test]
fn shared_snapshots_reuse_content_until_rows_theme_or_clip_changes() {
    let mut theme = Theme::dark();
    let rows: Arc<[GridRow]> = vec![
        GridRow::new(
            "first"
                .chars()
                .map(|ch| GridCell::new(ch.to_string(), &theme)),
        ),
        GridRow::new(
            "second"
                .chars()
                .map(|ch| GridCell::new(ch.to_string(), &theme)),
        ),
    ]
    .into();
    let cache = TerminalGridCache::default();
    let original = cache.content(&rows, &theme, 0..2);
    assert!(Rc::ptr_eq(&original, &cache.content(&rows, &theme, 0..2)));
    let clipped = cache.content(&rows, &theme, 1..2);
    assert!(!Rc::ptr_eq(&original, &clipped));
    assert_eq!(clipped.text.len(), 1);
    assert_eq!(clipped.text[0].row, 1);
    theme.font_mono = "another font".into();
    let new_font = cache.content(&rows, &theme, 1..2);
    assert!(!Rc::ptr_eq(&clipped, &new_font));
    let changed_rows: Arc<[GridRow]> = vec![GridRow::new(
        "replacement"
            .chars()
            .map(|ch| GridCell::new(ch.to_string(), &theme)),
    )]
    .into();
    assert_eq!(
        cache.content(&changed_rows, &theme, 0..1).text[0].text,
        "replacement"
    );
}

#[test]
fn culling_keeps_partially_visible_rows_and_skips_occluded_content() {
    let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(100.0)));
    let metrics = CellMetrics {
        width: px(8.0),
        height: px(10.0),
    };
    let mask = Bounds::new(point(px(0.0), px(35.0)), size(px(50.0), px(20.0)));
    assert_eq!(visible_rows(bounds, mask, metrics, 10), 1..4);
    let hidden = Bounds::new(point(px(200.0), px(20.0)), size(px(50.0), px(100.0)));
    assert_eq!(visible_rows(bounds, hidden, metrics, 10), 0..0);
    assert_eq!(visible_rows(bounds, bounds, metrics, 2), 0..2);
}

#[test]
fn shared_grid_constructor_retains_storage() {
    let rows: Arc<[GridRow]> = vec![GridRow::default()].into();
    let grid = TerminalGrid::from_shared(rows.clone());
    assert!(Arc::ptr_eq(&grid.rows, &rows));
}
