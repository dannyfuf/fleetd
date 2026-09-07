//! The diff renderer: file-header cards, hunk separators, tinted rows, syntax colours and
//! word-level marks, virtualised over `gpui::uniform_list`.
//!
//! Three layers compose per payload row, in this order:
//!
//! 1. **Row tint** — a wash derived from the theme background and the ANSI hue that already
//!    names this diff colour, so no new `ColorTokens` field is needed and both theme modes are
//!    correct for free. The gutter step is lighter than the row, so the columns read as a rail.
//! 2. **Syntax runs** from [`super::syntax`], as `HighlightStyle { color }`.
//! 3. **Word marks** from [`super::intraline`], as `HighlightStyle { background_color }` — a
//!    stronger wash on top of the already-tinted row.
//!
//! Layers 2 and 3 *must* go through [`gpui::combine_highlights`] before reaching
//! `StyledText::with_default_highlights`, which walks a monotonic cursor and panics on an
//! unsorted or overlapping range list. Syntax and word ranges overlap constantly.
//!
//! Horizontal scroll is a real pixel offset on the payload container (a negative left margin),
//! not a `chars().skip()`, so the gutters stay pinned and the clamp is `max_offset` on the
//! list's own scroll handle rather than nothing at all.

use std::ops::Range;
use std::rc::Rc;

use fleet_git::DiffKind;
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::theme::{CH, ch};
use fleet_ui_kit::{Theme, ThemeMode};
use gpui::{
    AnyElement, App, BorderStyle, Bounds, Corners, Edges, ElementId, FontFeatures, FontWeight,
    HighlightStyle, Hsla, Pixels, Point, SharedString, StyledText, TextRun, TextStyle,
    UniformListDecoration, UniformListScrollHandle, WhiteSpace, Window, WindowTextSystem, canvas,
    combine_highlights, div, fill, font, point, px, quad, size, transparent_black, uniform_list,
};

use super::Ansi;
use super::diff_model::{
    DiffModel, DiffRow, DiffViewMode, FileMeta, RowKind, is_panned_payload, marker,
};
use super::syntax::Bucket;

/// The width of the `+` / `-` sign column, in characters.
const SIGN_CH: f32 = 1.0;

/// The gap between the sign column and the code, in characters.
const SIGN_GAP_CH: f32 = 1.0;

/// The three washes one diff colour contributes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DiffTints {
    /// Behind the whole row.
    pub(crate) row: Hsla,
    /// Behind the line-number gutters — one step lighter, so the columns read as a rail.
    pub(crate) gutter: Hsla,
    /// Behind the words that actually changed.
    pub(crate) emphasis: Hsla,
    /// The 2 px bar at the very left edge, and the `+` / `-` sign.
    pub(crate) marker: Hsla,
}

/// Derives the washes for one diff colour from the theme, rather than adding tokens.
///
/// `Hsla::blend` composites its argument **over** the receiver and keeps the receiver's alpha,
/// so `bg.blend(hue.opacity(a))` is an opaque tinted background — Zed's own
/// `flattened_background_color` idiom.
#[must_use]
pub(crate) fn tints(theme: &Theme, ansi: Ansi) -> DiffTints {
    let hue = ansi.color(theme);
    let (row, emphasis, gutter) = match theme.mode {
        ThemeMode::Dark => (0.12, 0.26, 0.08),
        ThemeMode::Light => (0.16, 0.32, 0.10),
    };
    DiffTints {
        row: theme.colors.bg.blend(hue.opacity(row)),
        gutter: theme.colors.bg.blend(hue.opacity(gutter)),
        emphasis: theme.colors.bg.blend(hue.opacity(emphasis)),
        marker: hue,
    }
}

/// The washes for one row kind, or `None` for a row that keeps the plain background.
fn row_tints(theme: &Theme, kind: RowKind) -> Option<DiffTints> {
    match kind {
        RowKind::Added => Some(tints(theme, Ansi::Green)),
        RowKind::Removed => Some(tints(theme, Ansi::Red)),
        _ => None,
    }
}

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

/// The one `TextStyle` every payload line is shaped with.
///
/// Ligatures are off: a face that renders `!=` as a single glyph silently moves the column grid.
/// `WhiteSpace::Nowrap` short-circuits `TextLayout`'s wrap machinery entirely.
fn line_style(theme: &Theme, color: Hsla) -> TextStyle {
    TextStyle {
        color,
        font_family: theme.font_mono.clone(),
        font_features: FontFeatures::disable_ligatures(),
        font_size: theme.text.data.size.into(),
        line_height: gpui::DefiniteLength::Absolute(theme.text.data.line_height.into()),
        font_weight: FontWeight::NORMAL,
        white_space: WhiteSpace::Nowrap,
        ..TextStyle::default()
    }
}

/// Applies the mono type role to a container, so `StyledText`'s own layout agrees with the runs
/// we hand it — it reads font size and line height from the *inherited* style.
fn mono(element: gpui::Div, theme: &Theme) -> gpui::Div {
    element
        .font_family(theme.font_mono.clone())
        .text_size(theme.text.data.size)
        .line_height(theme.text.data.line_height)
        .whitespace_nowrap()
}

/// One payload line, syntax-coloured with its changed words marked.
fn payload(
    text: &SharedString,
    runs: &[(Range<usize>, Bucket)],
    words: &[Range<usize>],
    base: Hsla,
    emphasis: Option<Hsla>,
    theme: &Theme,
) -> AnyElement {
    if text.is_empty() {
        return div().into_any_element();
    }
    let style = line_style(theme, base);
    let syntax = runs.iter().map(|(range, bucket)| {
        (
            range.clone(),
            HighlightStyle {
                color: Some(bucket.color(theme)),
                ..HighlightStyle::default()
            },
        )
    });
    let marks = words.iter().filter_map(|range| {
        Some((
            range.clone(),
            HighlightStyle {
                background_color: Some(emphasis?),
                ..HighlightStyle::default()
            },
        ))
    });
    // `combine_highlights` sweeps the endpoints and emits disjoint, sorted ranges. Without it
    // `with_default_highlights` panics on the first overlap between a syntax run and a word mark.
    let highlights: Vec<(Range<usize>, HighlightStyle)> =
        combine_highlights(syntax, marks).collect();
    StyledText::new(text.clone())
        .with_default_highlights(&style, highlights)
        .into_any_element()
}

/// How a row is selected, and where the payload is scrolled to.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RowStyle {
    /// Whether the row is inside the current selection.
    pub(crate) selected: bool,
    /// Whether the row is the cursor itself, which also earns the 2 px left bar.
    pub(crate) cursor: bool,
    /// Whether the panel owning this list has the keyboard.
    pub(crate) focused: bool,
    /// Horizontal payload offset, in pixels.
    pub(crate) h_scroll: f32,
}

/// The background of a row, with the selection composited over the diff tint rather than
/// replacing it — a selected added line must still read as added.
///
/// The wash is the accent hue at a low alpha rather than `row_selected` at a high one: the
/// latter is an opaque blue-grey and swallows the green or red the row is carrying, which is
/// precisely the information a staging selection must not hide.
fn background(theme: &Theme, tint: Option<Hsla>, style: RowStyle) -> Option<Hsla> {
    if !style.selected {
        return tint;
    }
    let base = tint.unwrap_or(theme.colors.bg);
    let alpha = if style.focused { 0.26 } else { 0.10 };
    Some(base.blend(theme.colors.accent.opacity(alpha)))
}

/// One line-number gutter cell.
fn gutter(number: Option<u32>, digits: usize, tint: Option<Hsla>, theme: &Theme) -> AnyElement {
    let text = number.map_or_else(String::new, |number| number.to_string());
    let mut cell = mono(div(), theme)
        .flex_none()
        .w(ch(digits as f32 + 1.0))
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .pr(theme.space.xs)
        .text_color(theme.colors.text_muted);
    if let Some(tint) = tint {
        cell = cell.bg(tint);
    }
    cell.child(text).into_any_element()
}

/// Which line-number gutters a payload row draws.
///
/// Unified shows both, so a reader can follow either side. Split shows one per column, because
/// the column *is* the side and repeating the other number is noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gutters {
    /// Old and new, in that order.
    Both,
    /// The old side only — the left column of a split.
    Old,
    /// The new side only — the right column of a split.
    New,
}

/// One payload row: marker bar, the gutters, the sign column and the scrolled payload.
fn line_row(
    model: &DiffModel,
    index: usize,
    style: RowStyle,
    gutters: Gutters,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let Some(row) = model.rows.get(index) else {
        return div().h(theme.metrics.diff_row_h).into_any_element();
    };
    let tints = row_tints(theme, row.kind);
    let base = match row.kind {
        RowKind::Context => theme.colors.text_secondary,
        RowKind::Other => theme.colors.text_muted,
        _ => theme.colors.text,
    };
    let runs = model.runs_for(index);
    let mut element = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.metrics.diff_row_h)
        .overflow_hidden();
    if let Some(background) = background(theme, tints.map(|tint| tint.row), style) {
        element = element.bg(background);
    }
    // Zed's gutter strip: a thin bar flush at x = 0, so a change is legible even when the row
    // tint is washed out by a selection on top of it. Every row of a *selection* carries the
    // cursor colour, which is what makes a multi-row range read as one block.
    let bar = if style.selected {
        Some(theme.colors.cursor_bar)
    } else {
        tints.map(|tint| tint.marker)
    };
    if let Some(bar) = bar {
        element = element.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(theme.metrics.focus_ring_w)
                .h(theme.metrics.diff_row_h)
                .bg(bar),
        );
    }
    let wash = tints.map(|tint| tint.gutter);
    if matches!(gutters, Gutters::Both | Gutters::Old) {
        element = element.child(gutter(row.old_no, model.digits, wash, theme));
    }
    if matches!(gutters, Gutters::Both | Gutters::New) {
        element = element.child(gutter(row.new_no, model.digits, wash, theme));
    }
    element
        // The sign gets its own fixed `SIGN_CH` cell and the code a fixed `SIGN_GAP_CH` of
        // padding after it, so the code column starts at the same x on every row whatever its
        // kind — a `+`/`-` glued to an unindented line and floating away from an indented one is
        // how the columns drift by a character. GitHub, diffs.com and Zed all do this.
        .child(
            mono(div(), theme)
                .flex_none()
                .w(ch(SIGN_CH))
                .text_color(tints.map_or(theme.colors.text_muted, |tint| tint.marker))
                .child(marker(row.kind)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pl(ch(SIGN_GAP_CH))
                .overflow_hidden()
                .child(if let Some(line) = model.long_lines.get(&index) {
                    line.element(style.h_scroll, theme.text.data.line_height)
                } else {
                    mono(div(), theme)
                        .ml(px(-style.h_scroll))
                        .flex_none()
                        .child(payload(
                            &row.text,
                            &runs,
                            &row.words,
                            base,
                            tints.map(|tint| tint.emphasis),
                            theme,
                        ))
                        .into_any_element()
                }),
        )
        .into_any_element()
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
        _ => line_row(model, index, style, Gutters::Both, cx),
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
            .child(line_row(model, row, style, gutters, cx))
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
    fn markers_match_the_patch_sign_column() {
        assert_eq!(marker(RowKind::Added), "+");
        assert_eq!(marker(RowKind::Removed), "-");
        assert_eq!(marker(RowKind::Context), " ");
        assert_eq!(marker(RowKind::FileHeader), "");
    }

    #[test]
    fn tints_stay_opaque_and_ordered() {
        let theme = Theme::dark();
        let added = tints(&theme, Ansi::Green);
        assert_eq!(added.row.a, 1.0, "a row wash must be opaque");
        assert_eq!(added.gutter.a, 1.0);
        assert_eq!(added.emphasis.a, 1.0);
        // The emphasis wash must be further from the background than the row wash, or a marked
        // word is invisible on its own row.
        let distance = |colour: Hsla| (colour.l - theme.colors.bg.l).abs() + (colour.s).abs();
        assert!(distance(added.emphasis) > distance(added.row));
        assert!(distance(added.row) > distance(added.gutter));
    }

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
