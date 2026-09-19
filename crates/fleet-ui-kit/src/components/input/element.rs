//! Custom layout and paint for [`super::TextInput`].

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity,
    Font, GlobalElementId, InspectorElementId, LayoutId, PaintQuad, Pixels, ShapedLine,
    SharedString, Size, Style, TextAlign, TextRun, TextStyle, UnderlineStyle, Window, fill, point,
    prelude::*, relative, size,
};

use super::{InputMode, TextInput};
use crate::theme::{ActiveTheme, Theme, ThemeMode};

/// The painted half of a [`TextInput`]. It is custom because selection, caret, IME geometry and
/// input-handler bounds must all use the same shaped lines used for hit testing.
pub(super) struct TextInputElement {
    pub(super) input: Entity<TextInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LayoutKey {
    revision: u64,
    available_width_bits: u32,
    font: Font,
    font_size_bits: u32,
    theme_mode: ThemeMode,
}

struct CachedLine {
    start: usize,
    len: usize,
    layout: Option<ShapedLine>,
}

/// Memoised logical-line layouts used by paint, pointer hit testing and the platform bridge.
#[derive(Default)]
pub(super) struct LineLayoutCache {
    key: Option<LayoutKey>,
    lines: Vec<CachedLine>,
}

impl LineLayoutCache {
    pub(super) fn clear(&mut self) {
        self.key = None;
        self.lines.clear();
    }

    pub(super) fn line(&self, index: usize) -> Option<(usize, &ShapedLine)> {
        let line = self.lines.get(index)?;
        Some((line.start, line.layout.as_ref()?))
    }

    pub(super) fn line_index_for_offset(&self, offset: usize) -> Option<(usize, usize)> {
        self.lines.iter().enumerate().find_map(|(index, line)| {
            (offset >= line.start && offset <= line.start + line.len)
                .then_some((index, offset.saturating_sub(line.start).min(line.len)))
        })
    }

    fn take_lines(&mut self) -> Vec<(usize, usize, ShapedLine)> {
        self.lines
            .iter_mut()
            .filter_map(|line| {
                line.layout
                    .take()
                    .map(|layout| (line.start, line.len, layout))
            })
            .collect()
    }

    fn restore_lines(&mut self, lines: Vec<(usize, usize, ShapedLine)>) {
        for (start, len, layout) in lines {
            if let Some(line) = self
                .lines
                .iter_mut()
                .find(|line| line.start == start && line.len == len)
            {
                line.layout = Some(layout);
            }
        }
    }
}

pub(super) struct InputLayout {
    lines: Vec<(usize, usize, ShapedLine)>,
    selections: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    horizontal_scroll: Pixels,
    scroll_row: usize,
    revision: u64,
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
        style.size.width = relative(1.0).into();
        let layout_id = window.request_measured_layout(
            style,
            move |known_dimensions, available_space, window, cx| {
                let width = known_dimensions
                    .width
                    .unwrap_or(match available_space.width {
                        AvailableSpace::Definite(width) => width,
                        AvailableSpace::MinContent | AvailableSpace::MaxContent => Pixels::ZERO,
                    });
                let theme = cx.theme().clone();
                let (rows, shaped_width) = input.update(cx, |input, _cx| {
                    input.line_height = line_height;
                    prepare_lines(input, width, font_size, &text_style, &theme, window);
                    let rows = match input.mode() {
                        InputMode::SingleLine => 1,
                        InputMode::Multiline { min_rows, max_rows } => input
                            .buffer
                            .line_count()
                            .clamp(min_rows.max(1), max_rows.max(min_rows.max(1))),
                    };
                    (rows, widest_line(&input.line_cache))
                });
                Size {
                    width: if width > Pixels::ZERO {
                        width
                    } else {
                        shaped_width
                    },
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
            let revision = input.buffer.revision();
            let lines = input.line_cache.take_lines();
            let selections = selection_quads(input, &lines, bounds, &theme);
            let caret = caret_quad(input, &lines, bounds, &theme);
            input.reveal_caret = false;
            InputLayout {
                lines,
                selections,
                caret,
                horizontal_scroll,
                scroll_row,
                revision,
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
            .update(cx, |input, cx| input.ensure_blur_subscription(window, cx));

        let line_height = window.line_height();
        let horizontal_scroll = prepaint.horizontal_scroll;
        let scroll_row = prepaint.scroll_row;
        let lines = std::mem::take(&mut prepaint.lines);
        let selections = std::mem::take(&mut prepaint.selections);
        let caret = prepaint.caret.take();

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for selection in selections {
                window.paint_quad(selection);
            }
            for (index, (_, _, line)) in lines.iter().enumerate() {
                if index < scroll_row {
                    continue;
                }
                let top = bounds.top() + line_height * (index - scroll_row) as f32;
                if top >= bounds.bottom() {
                    break;
                }
                let origin = point(bounds.left() - horizontal_scroll, top);
                if let Err(error) =
                    line.paint(origin, line_height, TextAlign::Left, None, window, cx)
                {
                    crate::paint_error::log_once(&error);
                }
            }
            if focused && let Some(caret) = caret {
                window.paint_quad(caret);
            }
        });

        self.input.update(cx, |input, _cx| {
            if input.buffer.revision() == prepaint.revision {
                input.line_cache.restore_lines(lines);
            }
        });
    }
}

fn prepare_lines(
    input: &mut TextInput,
    available_width: Pixels,
    font_size: Pixels,
    style: &TextStyle,
    theme: &Theme,
    window: &mut Window,
) {
    let key = LayoutKey {
        revision: input.buffer.revision(),
        available_width_bits: f32::from(available_width).to_bits(),
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
    for raw_line in source.split('\n') {
        let line_end = start + raw_line.len();
        let local_marked = marked
            .as_ref()
            .and_then(|range| range_on_line(range, start, line_end));
        let runs = text_runs(raw_line.len(), local_marked, color, style, theme);
        let layout = window.text_system().shape_line(
            SharedString::from(raw_line.to_owned()),
            font_size,
            &runs,
            None,
        );
        lines.push(CachedLine {
            start,
            len: raw_line.len(),
            layout: Some(layout),
        });
        start = line_end + '\n'.len_utf8();
    }
    input.line_cache.key = Some(key);
    input.line_cache.lines = lines;
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
        .map(ShapedLine::width)
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
            let caret_x = line.x_for_index(local);
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
            let visible = input.buffer.line_count().min(max_rows.max(1));
            let caret_line = input
                .line_for_offset(input.buffer.caret())
                .map_or(0, |(line, _)| line);
            if input.reveal_caret {
                if caret_line < input.scroll_row {
                    input.scroll_row = caret_line;
                } else if caret_line >= input.scroll_row + visible {
                    input.scroll_row = caret_line + 1 - visible;
                }
            }
            input.scroll_row = input
                .scroll_row
                .min(input.buffer.line_count().saturating_sub(visible));
            input.horizontal_scroll = Pixels::ZERO;
        }
    }
}

fn selection_quads(
    input: &TextInput,
    lines: &[(usize, usize, ShapedLine)],
    bounds: Bounds<Pixels>,
    theme: &Theme,
) -> Vec<PaintQuad> {
    let selected = input.buffer.selected_range();
    if selected.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, (start, len, line))| {
            if index < input.scroll_row {
                return None;
            }
            let top = bounds.top() + input.line_height * (index - input.scroll_row) as f32;
            if top >= bounds.bottom() {
                return None;
            }
            let end = start + len;
            let local_start = selected.start.max(*start).min(end) - start;
            let local_end = selected.end.max(*start).min(end) - start;
            let includes_newline =
                selected.start <= end && selected.end > end && end < input.buffer.text().len();
            if local_start == local_end && !includes_newline {
                return None;
            }
            let scroll = input.horizontal_scroll;
            let left = bounds.left() + line.x_for_index(local_start) - scroll;
            let mut right = bounds.left() + line.x_for_index(local_end) - scroll;
            if includes_newline {
                right = right.max(bounds.left() + line.width() + theme.metrics.focus_ring_w);
            }
            Some(fill(
                Bounds::from_corners(point(left, top), point(right, top + input.line_height)),
                theme.colors.selection,
            ))
        })
        .collect()
}

fn caret_quad(
    input: &TextInput,
    lines: &[(usize, usize, ShapedLine)],
    bounds: Bounds<Pixels>,
    theme: &Theme,
) -> Option<PaintQuad> {
    if input.buffer.has_selection() {
        return None;
    }
    let caret = input.buffer.caret();
    let (line_index, local) = input.line_for_offset(caret)?;
    let (_, _, line) = lines.get(line_index)?;
    let visible_index = line_index.checked_sub(input.scroll_row)?;
    let top = bounds.top() + input.line_height * visible_index as f32;
    (top < bounds.bottom()).then(|| {
        let x = bounds.left() + line.x_for_index(local) - input.horizontal_scroll;
        fill(
            Bounds::new(
                point(x, top),
                size(theme.metrics.focus_ring_w, input.line_height),
            ),
            theme.colors.accent,
        )
    })
}
