//! The diff renderer: file-header cards, hunk separators, notes and the split layout,
//! virtualised over `gpui::uniform_list`.
//!
//! Payload rows themselves come from [`super::row_layout`], which the reusable inline
//! [`crate::diff_view::DiffView`] draws from too; this module supplies lazygit's own
//! [`RowPalette::ansi`] wash and everything around a line.
//!
//! Horizontal scroll is a real pixel offset on the payload container (a negative left margin),
//! not a `chars().skip()`, so the gutters stay pinned and the clamp is `max_offset` on the
//! list's own scroll handle rather than nothing at all.

use std::ops::Range;
use std::rc::Rc;

use fleet_git::DiffKind;
use fleet_ui_kit::Theme;
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::theme::{CH, ch};
use gpui::{
    AnyElement, App, BorderStyle, Bounds, Corners, Edges, ElementId, FontFeatures, FontWeight,
    Hsla, Pixels, Point, TextRun, UniformListDecoration, UniformListScrollHandle, Window,
    WindowTextSystem, canvas, div, fill, font, point, px, quad, size, transparent_black,
    uniform_list,
};

use super::Ansi;
use super::diff_model::{DiffModel, DiffRow, DiffViewMode, FileMeta, RowKind, is_panned_payload};
use super::row_layout::{
    Gutters, RowPalette, RowStyle, SIGN_CH, SIGN_GAP_CH, background, line_row,
};

/// The colour a file's status glyph and counts use.
#[must_use]
pub(crate) fn kind_color(theme: &Theme, kind: DiffKind) -> Hsla {
    match kind {
        DiffKind::Added => Ansi::Green.color(theme),
        DiffKind::Deleted => Ansi::Red.color(theme),
        DiffKind::Unmerged => Ansi::Magenta.color(theme),
        _ => Ansi::Yellow.color(theme),
    }
}

/// The glyph a file's status shows.
#[must_use]
pub(crate) fn kind_icon(kind: DiffKind) -> Icon {
    match kind {
        DiffKind::Added => Icon::Plus,
        DiffKind::Deleted => Icon::Minus,
        DiffKind::Renamed => Icon::ArrowRightLeft,
        DiffKind::Copied => Icon::CopyPlus,
        DiffKind::Unmerged => Icon::TriangleAlert,
        _ => Icon::FilePen,
    }
}

/// The top half of a file-header card: status glyph, directory, file name, rename arrow.
fn header_head(meta: &FileMeta, style: RowStyle, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (directory, name) = match meta.path.rfind('/') {
        Some(at) => (&meta.path[..=at], &meta.path[at + 1..]),
        None => ("", meta.path.as_str()),
    };
    let mut element = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.metrics.diff_row_h)
        .gap(theme.space.xs)
        .px(theme.space.sm)
        .overflow_hidden()
        .border_t(theme.metrics.hairline)
        .border_color(theme.colors.border)
        // One selection recipe for every row kind: the accent hue composited over whatever the
        // row's own background is (see `background`).
        .bg(background(theme, Some(theme.colors.surface), style).unwrap_or(theme.colors.surface))
        .child(
            kind_icon(meta.kind)
                .size(IconSize::Small)
                .color(kind_color(theme, meta.kind)),
        );
    if !directory.is_empty() {
        element = element.child(Text::data(directory.to_owned()).faint().flex_none());
    }
    element = element.child(
        Text::data(name.to_owned())
            .weight(FontWeight::MEDIUM)
            .flex_none(),
    );
    if let Some(old) = &meta.old_path {
        element = element
            .child(Text::data("\u{2190}").faint().flex_none())
            .child(Text::data(old.clone()).muted().ellipsize());
    }
    element.into_any_element()
}

/// The bottom half of a file-header card: the diffstat, the grammar and any mode change.
///
/// The counts use Zed's `DiffStat` formatting verbatim — U+2009 THIN SPACE and U+2012 FIGURE
/// DASH, because a figure dash is digit-width, so `+ 12` and `− 3` line up as columns.
fn header_foot(meta: &FileMeta, style: RowStyle, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut element = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.metrics.diff_row_h)
        .gap(theme.space.sm)
        .px(theme.space.sm)
        .overflow_hidden()
        .border_b(theme.metrics.hairline)
        .border_color(theme.colors.border)
        // One selection recipe for every row kind: the accent hue composited over whatever the
        // row's own background is (see `background`).
        .bg(background(theme, Some(theme.colors.surface), style).unwrap_or(theme.colors.surface))
        // Line the counts up under the status glyph's text column.
        .child(div().w(ch(SIGN_CH + SIGN_GAP_CH)).flex_none())
        .child(
            Text::data_small(format!("+\u{2009}{}", meta.added))
                .color(Ansi::Green.color(theme))
                .flex_none(),
        )
        .child(
            Text::data_small(format!("\u{2012}\u{2009}{}", meta.removed))
                .color(Ansi::Red.color(theme))
                .flex_none(),
        );
    if let Some(language) = &meta.language {
        element = element.child(Text::data_small(language.clone()).faint().flex_none());
    }
    if let Some(mode) = &meta.mode {
        element = element.child(Text::data_small(mode.clone()).faint().flex_none());
    }
    if meta.binary {
        element = element.child(Text::data_small("binary").faint().flex_none());
    }
    element.into_any_element()
}

/// An `@@` separator row: a subtle band, the range summary, git's function context, and the
/// affordance that widens the surrounding context.
///
/// The affordance is deliberately the same action `}` fires rather than a per-hunk expansion:
/// git prints context per *diff*, not per hunk, so widening one hunk means re-reading the file
/// with a larger `-U` anyway — and then every hunk widens.
fn hunk_row(row: &DiffRow, index: usize, style: RowStyle, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let band = theme.colors.surface;
    let mut element = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.metrics.diff_row_h)
        .gap(theme.space.sm)
        .px(theme.space.sm)
        .overflow_hidden()
        .bg(background(theme, Some(band), style).unwrap_or(band));
    if style.cursor {
        element = element
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(theme.metrics.focus_ring_w)
                    .h(theme.metrics.diff_row_h)
                    .bg(theme.colors.cursor_bar),
            )
            .relative();
    }
    element
        .child(
            Text::data(row.text.clone())
                .color(Ansi::Cyan.color(theme))
                .ellipsize(),
        )
        .child(
            div()
                .id(("diff-expand", index))
                .flex_none()
                .px(theme.space.xs)
                .rounded(theme.radii.xs)
                .cursor_pointer()
                .hover(|style| style.bg(theme.colors.row_hover))
                .on_mouse_down(gpui::MouseButton::Left, |_event, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::diff::MoreContext), cx);
                })
                .child(Text::hint("} more context").faint()),
        )
        .into_any_element()
}

/// A split-mode alignment filler: hatched, so an empty side reads as "nothing here" rather than
/// as a blank line of code.
fn spacer(cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_1()
        .min_w_0()
        .h(theme.metrics.diff_row_h)
        // `pattern_slash`'s period is chosen to divide the row height an integral number of
        // times, so the hatching does not drift from row to row — Zed's `spacer_pattern_period`.
        .bg(gpui::pattern_slash(
            theme.colors.border,
            2.0,
            f32::from(theme.metrics.diff_row_h) / 3.0 - 2.0,
        ))
        .into_any_element()
}

/// One row of the unified layout.
#[must_use]
pub(crate) fn unified_row(
    model: &DiffModel,
    index: usize,
    style: RowStyle,
    cx: &App,
) -> AnyElement {
    let Some(row) = model.rows.get(index) else {
        return div().into_any_element();
    };
    match row.kind {
        RowKind::FileHeader => match model.files.get(row.file) {
            Some(meta) => header_head(meta, style, cx),
            None => div().into_any_element(),
        },
        RowKind::FileHeaderFoot => match model.files.get(row.file) {
            Some(meta) => header_foot(meta, style, cx),
            None => div().into_any_element(),
        },
        RowKind::HunkHeader => hunk_row(row, index, style, cx),
        RowKind::Note => note_row(row, style, cx),
        _ => line_row(
            model,
            index,
            style,
            Gutters::Both,
            RowPalette::ansi(cx.theme()),
            cx,
        ),
    }
}

/// A file-level note: binary, mode-only change, empty rename.
fn note_row(row: &DiffRow, style: RowStyle, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut element = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.metrics.diff_row_h)
        .px(theme.space.sm)
        .overflow_hidden();
    if let Some(background) = background(theme, None, style) {
        element = element.bg(background);
    }
    element
        .child(Text::data(row.text.clone()).muted().ellipsize())
        .into_any_element()
}

/// One row of the split layout: old on the left, new on the right, hairline between.
#[must_use]
pub(crate) fn split_row(model: &DiffModel, index: usize, style: RowStyle, cx: &App) -> AnyElement {
    let Some(&layout) = model.split.get(index) else {
        return div().into_any_element();
    };
    if layout.full {
        return layout.left.map_or_else(
            || div().into_any_element(),
            |row| unified_row(model, row, style, cx),
        );
    }
    let theme = cx.theme();
    // Both columns are `flex_1` over a zero basis, so each is exactly half of what the hairline
    // leaves — and since both draw the same `model.digits`-wide gutter and the same sign column,
    // the divider sits the same distance from each side's code. Each column carries the same
    // `style`, so `h_scroll` pans the two together and each clips its own panned content.
    let column = |row: Option<usize>, gutters: Gutters| match row {
        Some(row) => div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(line_row(
                model,
                row,
                style,
                gutters,
                RowPalette::ansi(theme),
                cx,
            ))
            .into_any_element(),
        None => spacer(cx),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.metrics.diff_row_h)
        .child(column(layout.left, Gutters::Old))
        .child(
            div()
                .w(theme.metrics.hairline)
                .h(theme.metrics.diff_row_h)
                .flex_none()
                .bg(theme.colors.border),
        )
        .child(column(layout.right, Gutters::New))
        .into_any_element()
}

/// A thin position indicator on the right edge of a diff list.
///
/// gpui has no `Scrollbar` and Zed's lives in its `ui` crate behind a hard dependency on the
/// `theme` crate, so this is ours: a `UniformListDecoration`, which is the only hook a
/// `uniform_list` offers. The decoration is prepainted in *scrolled* coordinates, hence the
/// `-scroll_offset` on both axes to pin it to the viewport.
struct Scrollbar {
    thumb: Hsla,
    track: Hsla,
    width: Pixels,
    thumb_min: Pixels,
}

impl UniformListDecoration for Scrollbar {
    fn compute(
        &self,
        _visible: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let viewport = f32::from(bounds.size.height);
        let content = f32::from(item_height) * item_count as f32;
        if content <= viewport || viewport <= 0.0 {
            return div().into_any_element();
        }
        let height = (viewport * viewport / content)
            .max(f32::from(self.thumb_min))
            .min(viewport);
        let progress = (-f32::from(scroll_offset.y) / (content - viewport)).clamp(0.0, 1.0);
        // The decoration is prepainted in *scrolled* coordinates, so undoing the offset pins the
        // bar to the viewport instead of letting it ride away with the content.
        let anchor = bounds.origin - scroll_offset;
        let left = anchor.x + bounds.size.width - self.width;
        let track = Bounds {
            origin: point(left, anchor.y),
            size: size(self.width, bounds.size.height),
        };
        let thumb = Bounds {
            origin: point(left, anchor.y + px(progress * (viewport - height))),
            size: size(self.width, px(height)),
        };
        let track_color = self.track;
        let width = self.width;
        let thumb_color = self.thumb;
        // A `canvas` rather than absolutely positioned `div`s: as the root of its own layout
        // pass the decoration has no containing block for a percentage or an inset to resolve
        // against, so painting the two quads directly is the only reliable route.
        canvas(
            move |_bounds, _window, _cx| {},
            move |_bounds, (), window, _cx| {
                window.paint_quad(fill(track, track_color));
                window.paint_quad(quad(
                    thumb,
                    Corners::all(width / 2.0).clamp_radii_for_quad_size(thumb.size),
                    thumb_color,
                    Edges::default(),
                    transparent_black(),
                    BorderStyle::default(),
                ));
            },
        )
        .w(bounds.size.width)
        .h(bounds.size.height)
        .into_any_element()
    }
}

/// What a rendered diff list needs to know beyond its model.
#[derive(Clone)]
pub(crate) struct ViewState {
    /// The cursor row, when this list has one.
    pub(crate) cursor: Option<usize>,
    /// The selected row range, inclusive, when a staging selection is active.
    pub(crate) range: Option<(usize, usize)>,
    /// Whether the owning panel has the keyboard.
    pub(crate) focused: bool,
    /// Horizontal payload offset, in pixels.
    pub(crate) h_scroll: f32,
    /// The list's scroll handle. Owned by the view, so the offset survives re-renders and the
    /// periodic refresh — case 1 of gpui's scroll-state persistence rule.
    pub(crate) scroll: UniformListScrollHandle,
    /// Whether to draw the position indicator.
    pub(crate) scrollbar: bool,
}

/// The virtualised diff list.
///
/// Mouse-wheel scrolling comes free and unconditionally: `uniform_list` presets
/// `overflow.y = Scroll` and, given `track_scroll`, owns a persistent offset — so the wheel
/// scrolls this list whether or not its panel holds the keyboard.
#[must_use]
pub(crate) fn diff_list(
    id: impl Into<ElementId>,
    model: Rc<DiffModel>,
    state: ViewState,
    cx: &App,
) -> AnyElement {
    measure_payload_advances(&model, cx);
    let count = model.len();
    if count == 0 {
        return div().into_any_element();
    }
    let theme = cx.theme();
    let scrollbar = Scrollbar {
        width: theme.metrics.diff_scrollbar_w,
        thumb_min: theme.metrics.diff_thumb_min_h,
        // Zed's recipe: blend the thumb onto the backdrop rather than trusting a token to be
        // legible on it. `scroll_thumb` alone is a hairline colour and disappears at 5 px.
        thumb: theme.colors.bg.blend(theme.colors.text_muted.opacity(0.55)),
        track: theme.colors.bg.blend(theme.colors.border.opacity(0.5)),
    };
    let split = model.mode == DiffViewMode::Split;
    let painted = model.clone();
    let cursor = state.cursor;
    let range = state.range;
    let focused = state.focused;
    let h_scroll = state.h_scroll;
    let show_scrollbar = state.scrollbar;
    let mut list = uniform_list(id, count, move |visible, _window, cx| {
        visible
            .map(|index| {
                let style = RowStyle {
                    selected: match range {
                        Some((start, end)) => index >= start && index <= end,
                        None => cursor == Some(index),
                    },
                    cursor: cursor == Some(index),
                    focused,
                    h_scroll,
                };
                if split {
                    split_row(&painted, index, style, cx)
                } else {
                    unified_row(&painted, index, style, cx)
                }
            })
            .collect::<Vec<_>>()
    })
    .size_full()
    .track_scroll(&state.scroll);
    if show_scrollbar {
        list = list.with_decoration(scrollbar);
    }
    list.into_any_element()
}

/// How far the payload may be scrolled sideways before there is nothing left to reveal.
///
/// `viewport_w` is the width of the payload column with the gutters already subtracted; in split
/// mode each column gets half of it, so the clamp has to halve too or `L` stops short of the end
/// of a long line. The old renderer had no clamp at all and happily scrolled past the longest
/// line into blank rows.
#[must_use]
pub(crate) fn max_h_scroll(model: &DiffModel, viewport_w: f32) -> f32 {
    let advances = model
        .payload_advances
        .get()
        .unwrap_or_else(|| model.payload_columns.map(|columns| columns as f32 * CH));
    let column = match model.mode {
        DiffViewMode::Unified => viewport_w,
        DiffViewMode::Split => viewport_w / 2.0,
    };
    advances
        .into_iter()
        .map(|advance| advance - column)
        .fold(0.0, f32::max)
}

fn measure_payload_advances(model: &DiffModel, cx: &App) {
    if model.payload_advances.get().is_some() {
        return;
    }
    let theme = cx.theme();
    let mut mono_font = font(theme.font_mono.clone());
    mono_font.features = FontFeatures::disable_ligatures();
    let text_system = WindowTextSystem::new(cx.text_system().clone());
    let advances: Vec<f32> = model
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if !is_panned_payload(row.kind) {
                return 0.0;
            }
            if let Some(line) = model.long_lines.get(&index) {
                return f32::from(line.width());
            }
            let run = TextRun {
                len: row.text.len(),
                font: mono_font.clone(),
                color: theme.colors.text,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(
                text_system
                    .shape_line(row.text.clone(), theme.text.data.size, &[run], None)
                    .width(),
            )
        })
        .collect();
    let width = |row: Option<usize>| row.map_or(0.0, |row| advances[row]);
    let measured = match model.mode {
        DiffViewMode::Unified => [advances.into_iter().fold(0.0, f32::max), 0.0],
        DiffViewMode::Split => model.split.iter().filter(|layout| !layout.full).fold(
            [0.0_f32; 2],
            |mut widest, layout| {
                widest[0] = widest[0].max(width(layout.left));
                widest[1] = widest[1].max(width(layout.right));
                widest
            },
        ),
    };
    model.payload_advances.set(Some(measured));
}

/// Everything left of the code on a unified payload row, in pixels: both line-number gutters,
/// the sign column and the gap after it.
#[must_use]
pub(crate) fn gutter_width(model: &DiffModel) -> f32 {
    (model.digits as f32 + 1.0) * 2.0 * CH + (SIGN_CH + SIGN_GAP_CH) * CH
}

#[cfg(test)]
mod tests {
    use super::super::diff_model::DiffViewMode;
    use super::*;
    use fleet_git::parse;

    const PATCH: &[u8] = concat!(
        "diff --git a/a.txt b/a.txt\n",
        "--- a/a.txt\n",
        "+++ b/a.txt\n",
        "@@ -1,2 +1,3 @@\n",
        " one\n",
        "-two\n",
        "+deux\n",
        "+trois\n",
    )
    .as_bytes();

    #[test]
    fn horizontal_scroll_is_clamped_to_the_content() {
        let diff = parse::diff::parse(PATCH).expect("parses");
        let model = DiffModel::build(&diff, DiffViewMode::Unified);
        // The widest line is `@@ -1,2 +1,3 @@`, far narrower than a 900 px payload column.
        assert_eq!(max_h_scroll(&model, 900.0), 0.0);
        assert!(max_h_scroll(&model, 10.0) > 0.0);
        // And it never exceeds the content itself.
        assert!(max_h_scroll(&model, 0.0) <= model.payload_columns[0] as f32 * CH);
        // Split halves the column, so the same pane allows twice the pan.
        let split = DiffModel::build(
            &parse::diff::parse(PATCH).expect("parses"),
            DiffViewMode::Split,
        );
        assert!(max_h_scroll(&split, 20.0) > max_h_scroll(&model, 20.0));
    }

    #[gpui::test]
    fn extent_uses_column_advances(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            let patch = concat!(
                "diff --git a/a.txt b/a.txt\n",
                "--- a/a.txt\n",
                "+++ b/a.txt\n",
                "@@ -1 +1 @@\n",
                "-界界界界\n",
                "+x\n",
            );
            let model = DiffModel::build(
                &parse::diff::parse(patch.as_bytes()).expect("parses"),
                DiffViewMode::Split,
            );
            measure_payload_advances(&model, cx);
            let measured = model.payload_advances.get().expect("measured");

            assert!(measured[0] > measured[1]);
            assert_eq!(max_h_scroll(&model, measured[0] * 2.0), 0.0);
        });
    }

    #[test]
    fn the_sign_column_is_fixed_width() {
        // The code column starts at both gutters plus the sign cell plus its gap, on every row
        // kind: nothing about the payload can move it.
        let model = DiffModel::build(
            &parse::diff::parse(PATCH).expect("parses"),
            DiffViewMode::Unified,
        );
        let gutters = (model.digits as f32 + 1.0) * 2.0 * CH;
        // The sign cell is one character and the gap after it is not zero, so the code can
        // neither move with the payload nor touch the sign.
        assert_eq!(gutter_width(&model) - gutters, (SIGN_CH + SIGN_GAP_CH) * CH);
        assert_eq!(SIGN_CH, 1.0);
        assert_eq!(SIGN_GAP_CH.max(0.0), SIGN_GAP_CH);
        assert_ne!(SIGN_GAP_CH, 0.0);
    }

    #[test]
    fn the_gutter_widens_with_the_line_numbers() {
        let narrow = DiffModel::build(
            &parse::diff::parse(PATCH).expect("parses"),
            DiffViewMode::Unified,
        );
        assert_eq!(narrow.digits, 2);
        let wide = concat!(
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n",
            "+++ b/a.txt\n",
            "@@ -12000,1 +12000,2 @@\n",
            " one\n",
            "+two\n",
        );
        let wide = DiffModel::build(
            &parse::diff::parse(wide.as_bytes()).expect("parses"),
            DiffViewMode::Unified,
        );
        assert_eq!(wide.digits, 5);
        assert!(gutter_width(&wide) > gutter_width(&narrow));
    }
}
