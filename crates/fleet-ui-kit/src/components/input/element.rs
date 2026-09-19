//! Custom layout and paint for [`super::TextInput`].

use std::{collections::HashMap, ops::Range, sync::Arc};

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity,
    Font, GlobalElementId, InspectorElementId, LayoutId, PaintQuad, Pixels, ShapedLine,
    SharedString, Size, Style, TextAlign, TextRun, TextStyle, UnderlineStyle, Window, WrappedLine,
    fill, point, prelude::*, relative, size,
};

use super::{InputMode, TextInput};
use crate::{
    scroll_thumb,
    theme::{ActiveTheme, Theme, ThemeMode},
};

/// The painted half of a [`TextInput`]. It is custom because selection, caret, IME geometry and
/// input-handler bounds must all use the same shaped lines used for hit testing.
pub(super) struct TextInputElement {
    pub(super) input: Entity<TextInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LayoutKey {
    revision: u64,
    wrap_width_bits: Option<u32>,
    font: Font,
    font_size_bits: u32,
    theme_mode: ThemeMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LineShapeKey {
    line_index: usize,
    text: String,
    wrap_width_bits: Option<u32>,
    font: Font,
    font_size_bits: u32,
    dark_theme: bool,
    marked: Option<Range<usize>>,
    placeholder: bool,
    wrapped: bool,
}

pub(super) enum ShapedTextLine {
    Unwrapped(Box<ShapedLine>),
    Wrapped(WrappedLine),
}

impl ShapedTextLine {
    pub(super) fn visual_rows(&self) -> usize {
        match self {
            Self::Unwrapped(_) => 1,
            Self::Wrapped(line) => line.wrap_boundaries.len() + 1,
        }
    }

    fn width(&self) -> Pixels {
        match self {
            Self::Unwrapped(line) => line.width(),
            Self::Wrapped(line) => line.width(),
        }
    }

    pub(super) fn position_for_index(
        &self,
        index: usize,
        line_height: Pixels,
    ) -> Option<gpui::Point<Pixels>> {
        match self {
            Self::Unwrapped(line) => Some(point(line.x_for_index(index), Pixels::ZERO)),
            Self::Wrapped(line) => line.position_for_index(index, line_height),
        }
    }

    pub(super) fn closest_index_for_position(
        &self,
        position: gpui::Point<Pixels>,
        line_height: Pixels,
    ) -> usize {
        match self {
            Self::Unwrapped(line) => line.closest_index_for_x(position.x),
            Self::Wrapped(line) => match line.closest_index_for_position(position, line_height) {
                Ok(index) | Err(index) => index,
            },
        }
    }

    fn row_boundaries(&self) -> Vec<usize> {
        let Self::Wrapped(line) = self else {
            return Vec::new();
        };
        line.wrap_boundaries
            .iter()
            .map(|boundary| line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index)
            .collect()
    }
}

struct CachedLine {
    start: usize,
    len: usize,
    shape_key: LineShapeKey,
    layout: Option<Arc<ShapedTextLine>>,
}

/// Memoised logical-line layouts used by paint, pointer hit testing and the platform bridge.
#[derive(Default)]
pub(super) struct LineLayoutCache {
    key: Option<LayoutKey>,
    lines: Vec<CachedLine>,
    shapes: HashMap<LineShapeKey, Arc<ShapedTextLine>>,
    #[cfg(test)]
    miss_counts: Vec<usize>,
}

impl LineLayoutCache {
    pub(super) fn clear(&mut self) {
        self.key = None;
    }

    pub(super) fn is_current(&self, revision: u64) -> bool {
        self.key
            .as_ref()
            .is_some_and(|key| key.revision == revision)
            && !self.lines.is_empty()
            && self.lines.iter().all(|line| line.layout.is_some())
    }

    pub(super) fn line(&self, index: usize) -> Option<(usize, &ShapedTextLine)> {
        let line = self.lines.get(index)?;
        Some((line.start, line.layout.as_deref()?))
    }

    pub(super) fn visual_rows(&self) -> usize {
        self.lines
            .iter()
            .filter_map(|line| line.layout.as_ref())
            .map(|line| line.visual_rows())
            .sum()
    }

    #[cfg(test)]
    pub(super) fn logical_lines(&self) -> usize {
        self.lines.len()
    }

    #[cfg(test)]
    pub(super) fn reset_probe(&mut self) {
        self.miss_counts.clear();
    }

    #[cfg(test)]
    pub(super) fn miss_counts(&self) -> &[usize] {
        &self.miss_counts
    }

    pub(super) fn visual_row_start(&self, line_index: usize) -> Option<usize> {
        let mut rows = 0;
        for line in self.lines.get(..line_index)? {
            rows += line.layout.as_ref()?.visual_rows();
        }
        Some(rows)
    }

    pub(super) fn line_for_visual_row(
        &self,
        visual_row: usize,
    ) -> Option<(usize, &ShapedTextLine, usize)> {
        let mut row_start = 0;
        for line in &self.lines {
            let layout = line.layout.as_deref()?;
            let row_end = row_start + layout.visual_rows();
            if visual_row < row_end {
                return Some((line.start, layout, visual_row - row_start));
            }
            row_start = row_end;
        }
        None
    }

    pub(super) fn line_index_for_offset(&self, offset: usize) -> Option<(usize, usize)> {
        self.lines.iter().enumerate().find_map(|(index, line)| {
            (offset >= line.start && offset <= line.start + line.len)
                .then_some((index, offset.saturating_sub(line.start).min(line.len)))
        })
    }

    fn take_lines(&self) -> Vec<(usize, usize, Arc<ShapedTextLine>)> {
        self.lines
            .iter()
            .filter_map(|line| {
                line.layout
                    .clone()
                    .map(|layout| (line.start, line.len, layout))
            })
            .collect()
    }
}

pub(super) struct InputLayout {
    lines: Vec<(usize, usize, Arc<ShapedTextLine>)>,
    selections: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    thumb: Option<PaintQuad>,
    horizontal_scroll: Pixels,
    scroll_row: usize,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = InputLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.clone();
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let mut style = Style::default();
        // A measured leaf otherwise negotiates its intrinsic width with flexbox. Long text can
        // then make the leaf wider than the input box, leaving `shape_text` nothing to wrap.
        // Fill the already-resolved value column so its definite width is the wrapping width.
        style.size.width = relative(1.0).into();
        let layout_id = window.request_measured_layout(
            style,
            move |known_dimensions, available_space, window, cx| {
                let wrap_width = known_dimensions.width.or(match available_space.width {
                    AvailableSpace::Definite(width) => Some(width),
                    AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
                });
                let theme = cx.theme().clone();
                let (rows, shaped_width) = input.update(cx, |input, _cx| {
                    input.line_height = line_height;
                    prepare_lines(input, wrap_width, font_size, &text_style, &theme, window);
                    let rows = match input.mode() {
                        InputMode::SingleLine => 1,
                        InputMode::Multiline { min_rows, max_rows } => input
                            .line_cache
                            .visual_rows()
                            .max(1)
                            .clamp(min_rows.max(1), max_rows.max(min_rows.max(1))),
                    };
                    (rows, widest_line(&input.line_cache))
                });
                Size {
                    width: wrap_width.unwrap_or(shaped_width),
                    height: line_height * rows as f32,
                }
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.theme().clone();
        let line_height = window.line_height();
        self.input.update(cx, |input, _cx| {
            input.last_bounds = Some(bounds);
            input.line_height = line_height;
            prepare_scroll(input, bounds, &theme);
            let horizontal_scroll = input.horizontal_scroll;
            let scroll_row = input.scroll_row;
            let lines = input.line_cache.take_lines();
            let selections = selection_quads(input, &lines, bounds, &theme);
            let caret = caret_quad(input, &lines, bounds, &theme);
            let thumb = scroll_thumb_quad(input, bounds, &theme);
            #[cfg(test)]
            {
                input.last_selection_quad_count = selections.len();
            }
            input.reveal_caret = false;
            InputLayout {
                lines,
                selections,
                caret,
                thumb,
                horizontal_scroll,
                scroll_row,
            }
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        let focused = focus_handle.is_focused(window);
        let read_only = self.input.read(cx).read_only;
        if focused && !read_only {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }
        self.input
            .update(cx, |input, cx| input.ensure_focus_subscriptions(window, cx));

        let line_height = window.line_height();
        let horizontal_scroll = prepaint.horizontal_scroll;
        let scroll_row = prepaint.scroll_row;
        let lines = std::mem::take(&mut prepaint.lines);
        let selections = std::mem::take(&mut prepaint.selections);
        let caret = prepaint.caret.take();
        let thumb = prepaint.thumb.take();

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for selection in selections {
                window.paint_quad(selection);
            }
            let mut row_start = 0usize;
            for (_, _, line) in &lines {
                let top =
                    bounds.top() + line_height * row_start as f32 - line_height * scroll_row as f32;
                let height = line_height * line.visual_rows() as f32;
                if top + height <= bounds.top() {
                    row_start += line.visual_rows();
                    continue;
                }
                if top >= bounds.bottom() {
                    break;
                }
                let origin = point(bounds.left() - horizontal_scroll, top);
                let result = match line.as_ref() {
                    ShapedTextLine::Unwrapped(line) => {
                        line.paint(origin, line_height, TextAlign::Left, None, window, cx)
                    }
                    ShapedTextLine::Wrapped(line) => {
                        line.paint(origin, line_height, TextAlign::Left, None, window, cx)
                    }
                };
                if let Err(error) = result {
                    crate::paint_error::log_once(&error);
                }
                row_start += line.visual_rows();
            }
            if focused && let Some(caret) = caret {
                window.paint_quad(caret);
            }
            if let Some(thumb) = thumb {
                window.paint_quad(thumb);
            }
        });
    }
}

fn scroll_thumb_quad(
    input: &TextInput,
    bounds: Bounds<Pixels>,
    theme: &Theme,
) -> Option<PaintQuad> {
    let InputMode::Multiline { .. } = input.mode() else {
        return None;
    };
    let content_height = input.line_height * input.line_cache.visual_rows().max(1) as f32;
    let viewport = bounds.size.height;
    let max_scroll = (content_height - viewport).max(Pixels::ZERO);
    let scroll = input.line_height * input.scroll_row as f32;
    scroll_thumb(scroll, max_scroll, viewport, theme.metrics.diff_thumb_min_h).map(
        |(top, height)| {
            let width = theme.metrics.scroll_thumb_w;
            fill(
                Bounds::new(
                    point(
                        bounds.right() - width,
                        bounds.top() + bounds.size.height * top,
                    ),
                    size(width, bounds.size.height * height),
                ),
                theme.colors.scroll_thumb,
            )
        },
    )
}

fn prepare_lines(
    input: &mut TextInput,
    wrap_width: Option<Pixels>,
    font_size: Pixels,
    style: &TextStyle,
    theme: &Theme,
    window: &mut Window,
) {
    let key = LayoutKey {
        revision: input.buffer.revision(),
        wrap_width_bits: wrap_width.map(|width| f32::from(width).to_bits()),
        font: style.font(),
        font_size_bits: f32::from(font_size).to_bits(),
        theme_mode: theme.mode,
    };
    if input.line_cache.key.as_ref() == Some(&key)
        && input
            .line_cache
            .lines
            .iter()
            .all(|line| line.layout.is_some())
    {
        return;
    }

    let placeholder = input.buffer.is_empty();
    let source = if placeholder {
        input.placeholder.as_ref()
    } else {
        input.buffer.text()
    };
    let color = if placeholder {
        theme.colors.text_muted
    } else {
        theme.colors.text
    };
    let marked = (!placeholder)
        .then(|| input.buffer.marked_range())
        .flatten();
    let mut start = 0usize;
    let mut lines = Vec::new();
    #[cfg(test)]
    let mut misses = 0;
    for (line_index, raw_line) in source.split('\n').enumerate() {
        let line_end = start + raw_line.len();
        let local_marked = marked
            .as_ref()
            .and_then(|range| range_on_line(range, start, line_end));
        let shape_key = LineShapeKey {
            line_index,
            text: raw_line.to_owned(),
            wrap_width_bits: wrap_width.map(|width| f32::from(width).to_bits()),
            font: style.font(),
            font_size_bits: f32::from(font_size).to_bits(),
            dark_theme: matches!(theme.mode, ThemeMode::Dark),
            marked: local_marked.clone(),
            placeholder,
            wrapped: matches!(input.mode(), InputMode::Multiline { .. }),
        };
        let layout = input
            .line_cache
            .shapes
            .get(&shape_key)
            .cloned()
            .or_else(|| {
                #[cfg(test)]
                {
                    misses += 1;
                }
                let runs = text_runs(raw_line.len(), local_marked, color, style, theme);
                let text = SharedString::from(raw_line.to_owned());
                match input.mode() {
                    InputMode::SingleLine => Some(Arc::new(ShapedTextLine::Unwrapped(Box::new(
                        window
                            .text_system()
                            .shape_line(text, font_size, &runs, None),
                    )))),
                    InputMode::Multiline { .. } => match window
                        .text_system()
                        .shape_text(text, font_size, &runs, wrap_width, None)
                    {
                        Ok(lines) => lines
                            .into_iter()
                            .next()
                            .map(ShapedTextLine::Wrapped)
                            .map(Arc::new),
                        Err(error) => {
                            crate::paint_error::log_once(&error);
                            None
                        }
                    },
                }
            });
        let Some(layout) = layout else {
            return;
        };
        input
            .line_cache
            .shapes
            .insert(shape_key.clone(), layout.clone());
        lines.push(CachedLine {
            start,
            len: raw_line.len(),
            shape_key,
            layout: Some(layout),
        });
        start = line_end + '\n'.len_utf8();
    }
    input.line_cache.shapes.retain(|shape_key, _| {
        lines.get(shape_key.line_index).is_some_and(|line| {
            let active = &line.shape_key;
            shape_key.line_index == active.line_index
                && shape_key.text == active.text
                && shape_key.font == active.font
                && shape_key.font_size_bits == active.font_size_bits
                && shape_key.dark_theme == active.dark_theme
                && shape_key.marked == active.marked
                && shape_key.placeholder == active.placeholder
                && shape_key.wrapped == active.wrapped
        })
    });
    input.line_cache.key = Some(key);
    input.line_cache.lines = lines;
    #[cfg(test)]
    input.line_cache.miss_counts.push(misses);
}

fn range_on_line(range: &Range<usize>, line_start: usize, line_end: usize) -> Option<Range<usize>> {
    let start = range.start.max(line_start);
    let end = range.end.min(line_end);
    (start < end).then_some(start - line_start..end - line_start)
}

fn text_runs(
    len: usize,
    marked: Option<Range<usize>>,
    color: gpui::Hsla,
    style: &TextStyle,
    theme: &Theme,
) -> Vec<TextRun> {
    let base = TextRun {
        len,
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let Some(marked) = marked else {
        return vec![base];
    };
    [0..marked.start, marked.clone(), marked.end..len]
        .into_iter()
        .filter(|range| !range.is_empty())
        .map(|range| TextRun {
            len: range.len(),
            underline: (range == marked).then_some(UnderlineStyle {
                color: Some(color),
                thickness: theme.metrics.hairline,
                wavy: false,
            }),
            ..base.clone()
        })
        .collect()
}

fn widest_line(cache: &LineLayoutCache) -> Pixels {
    cache
        .lines
        .iter()
        .filter_map(|line| line.layout.as_ref())
        .map(|line| line.width())
        .max()
        .unwrap_or(Pixels::ZERO)
}

fn prepare_scroll(input: &mut TextInput, bounds: Bounds<Pixels>, theme: &Theme) {
    match input.mode() {
        InputMode::SingleLine => {
            let Some((line_index, local)) = input.line_for_offset(input.buffer.caret()) else {
                return;
            };
            let Some((_, line)) = input.line_cache.line(line_index) else {
                return;
            };
            let Some(caret) = line.position_for_index(local, input.line_height) else {
                return;
            };
            let caret_x = caret.x;
            if input.reveal_caret {
                input.horizontal_scroll = input.horizontal_scroll.min(caret_x);
                input.horizontal_scroll = input
                    .horizontal_scroll
                    .max(caret_x + theme.metrics.focus_ring_w - bounds.size.width);
            }
            let max_scroll =
                (line.width() + theme.metrics.focus_ring_w - bounds.size.width).max(Pixels::ZERO);
            input.horizontal_scroll = input.horizontal_scroll.clamp(Pixels::ZERO, max_scroll);
        }
        InputMode::Multiline { max_rows, .. } => {
            let total_rows = input.line_cache.visual_rows().max(1);
            let visible = total_rows.min(max_rows.max(1));
            let caret_row = input
                .position_for_offset(input.buffer.caret())
                .map_or(0, |position| {
                    (f32::from(position.y) / f32::from(input.line_height)).floor() as usize
                });
            if input.reveal_caret {
                if caret_row < input.scroll_row {
                    input.scroll_row = caret_row;
                } else if caret_row >= input.scroll_row + visible {
                    input.scroll_row = caret_row + 1 - visible;
                }
            }
            input.scroll_row = input.scroll_row.min(total_rows.saturating_sub(visible));
            input.horizontal_scroll = Pixels::ZERO;
        }
    }
}

fn selection_quads(
    input: &TextInput,
    lines: &[(usize, usize, Arc<ShapedTextLine>)],
    bounds: Bounds<Pixels>,
    theme: &Theme,
) -> Vec<PaintQuad> {
    let selected = input.buffer.selected_range();
    if selected.is_empty() {
        return Vec::new();
    }
    let mut quads = Vec::new();
    let mut logical_row_start = 0usize;
    for (start, len, line) in lines {
        let mut row_boundaries = line.row_boundaries();
        row_boundaries.push(*len);
        let mut row_start = 0usize;
        for (row, row_end) in row_boundaries.into_iter().enumerate() {
            let visual_row = logical_row_start + row;
            if visual_row < input.scroll_row {
                row_start = row_end;
                continue;
            }
            let top = bounds.top() + input.line_height * (visual_row - input.scroll_row) as f32;
            if top >= bounds.bottom() {
                break;
            }
            let end = start + len;
            let local_start = (selected.start.max(*start).min(end) - start).max(row_start);
            let local_end = (selected.end.max(*start).min(end) - start).min(row_end);
            let includes_newline = row + 1 == line.visual_rows()
                && selected.start <= end
                && selected.end > end
                && end < input.buffer.text().len();
            if local_start >= local_end && !includes_newline {
                row_start = row_end;
                continue;
            }
            let left_x = if local_start == row_start {
                Pixels::ZERO
            } else {
                line.position_for_index(local_start, input.line_height)
                    .map_or(Pixels::ZERO, |position| position.x)
            };
            let right_x = line
                .position_for_index(local_end, input.line_height)
                .map_or(Pixels::ZERO, |position| position.x);
            let left = bounds.left() + left_x - input.horizontal_scroll;
            let mut right = bounds.left() + right_x - input.horizontal_scroll;
            if includes_newline {
                right = right.max(bounds.left() + right_x + theme.metrics.focus_ring_w);
            }
            quads.push(fill(
                Bounds::from_corners(point(left, top), point(right, top + input.line_height)),
                theme.colors.selection,
            ));
            row_start = row_end;
        }
        logical_row_start += line.visual_rows();
    }
    quads
}

fn caret_quad(
    input: &TextInput,
    lines: &[(usize, usize, Arc<ShapedTextLine>)],
    bounds: Bounds<Pixels>,
    theme: &Theme,
) -> Option<PaintQuad> {
    if input.buffer.has_selection() {
        return None;
    }
    let caret = input.buffer.caret();
    let position = position_for_offset(lines, caret, input.line_height)?;
    let top = bounds.top() + position.y - input.line_height * input.scroll_row as f32;
    (top < bounds.bottom()).then(|| {
        let x = bounds.left() + position.x - input.horizontal_scroll;
        fill(
            Bounds::new(
                point(x, top),
                size(theme.metrics.focus_ring_w, input.line_height),
            ),
            theme.colors.accent,
        )
    })
}

fn position_for_offset(
    lines: &[(usize, usize, Arc<ShapedTextLine>)],
    offset: usize,
    line_height: Pixels,
) -> Option<gpui::Point<Pixels>> {
    let mut row_start = 0usize;
    for (start, len, line) in lines {
        if offset >= *start && offset <= start + len {
            let local = offset.saturating_sub(*start).min(*len);
            let position = line.position_for_index(local, line_height)?;
            return Some(point(
                position.x,
                position.y + line_height * row_start as f32,
            ));
        }
        row_start += line.visual_rows();
    }
    None
}
