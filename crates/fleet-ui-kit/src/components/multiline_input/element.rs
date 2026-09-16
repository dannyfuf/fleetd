use std::{collections::HashMap, ops::Range};

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity,
    Font, GlobalElementId, Hsla, InspectorElementId, LayoutId, PaintQuad, Pixels, SharedString,
    Size, Style, TextAlign, TextRun, TextStyle, UnderlineStyle, Window, WrappedLine, fill, point,
    prelude::*, relative, size,
};

use super::{
    MultilineInput,
    buffer::{display_offset, expand_tabs},
};

use crate::{
    scroll_thumb,
    theme::{ActiveTheme, Theme},
};

/// The painted half of a [`MultilineInput`]: wrapped lines, a selection wash, an IME underline
/// and the 2 px caret.
///
/// The element measures its own height, because that height is what makes the composer grow:
/// wrapping is only known once the flex row has resolved a width, so shaping happens in the
/// measure pass and the shaped lines are parked on the entity for hit-testing, IME bounds and
/// paint.
pub(super) struct MultilineInputElement {
    pub(super) input: Entity<MultilineInput>,
    pub(super) max_lines: usize,
}

/// What [`MultilineInputElement::prepaint`] hands to `paint`.
pub(super) struct MultilineLayout {
    lines: Vec<(usize, WrappedLine)>,
    caret: Option<PaintQuad>,
    thumb: Option<PaintQuad>,
    scroll: Pixels,
    revision: u64,
}

/// One logical line's shaped result. The key is exactly the text, wrap width and font tuple,
/// plus decorations that affect paint but not glyph shaping.
struct CachedLine {
    runs: Vec<TextRun>,
    layout: Option<WrappedLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LineShapeKey {
    line_index: usize,
    text: String,
    wrap_width_bits: Option<u32>,
    font: Font,
    font_size_bits: u32,
}

/// Per-logical-line shaping cache. A normal keystroke invalidates one map entry; unchanged
/// entries keep the same `SharedString`, allowing GPUI's layout cache to retain their glyphs.
#[derive(Default)]
pub(super) struct LineLayoutCache {
    entries: HashMap<LineShapeKey, CachedLine>,
    active: Vec<LineShapeKey>,
    revision: u64,
    #[cfg(test)]
    miss_counts: Vec<usize>,
}

impl LineLayoutCache {
    pub(super) fn is_current(&self, revision: u64) -> bool {
        self.revision == revision && !self.active.is_empty()
    }

    pub(super) fn line(&self, index: usize) -> Option<&WrappedLine> {
        self.entries.get(self.active.get(index)?)?.layout.as_ref()
    }

    pub(super) fn visual_rows(&self) -> usize {
        (0..self.active.len())
            .filter_map(|index| self.line(index))
            .map(|line| line.wrap_boundaries.len() + 1)
            .sum()
    }

    #[cfg(test)]
    pub(super) fn max_width(&self) -> Pixels {
        (0..self.active.len())
            .filter_map(|index| self.line(index))
            .map(|line| line.width())
            .max()
            .unwrap_or(Pixels::ZERO)
    }

    fn take_lines(&mut self) -> Vec<(usize, WrappedLine)> {
        (0..self.active.len())
            .filter_map(|index| {
                let key = self.active.get(index)?;
                self.entries
                    .get_mut(key)
                    .and_then(|entry| entry.layout.take())
                    .map(|line| (index, line))
            })
            .collect()
    }

    fn restore_lines(&mut self, lines: Vec<(usize, WrappedLine)>) {
        for (index, line) in lines {
            let Some(key) = self.active.get(index) else {
                continue;
            };
            if let Some(entry) = self.entries.get_mut(key) {
                entry.layout = Some(line);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn reset_probe(&mut self) {
        self.miss_counts.clear();
    }

    #[cfg(test)]
    pub(super) fn miss_counts(&self) -> &[usize] {
        &self.miss_counts
    }
}

fn prepare_lines(
    input: &mut MultilineInput,
    wrap_width: Option<Pixels>,
    font_size: Pixels,
    style: &TextStyle,
    theme: &Theme,
    window: &mut Window,
) -> (usize, Pixels) {
    let placeholder = input.buffer.is_empty();
    let color = if placeholder {
        theme.colors.text_muted
    } else {
        theme.colors.text
    };
    let selection = (!placeholder).then(|| input.buffer.selected_range());
    let marked = (!placeholder)
        .then(|| input.buffer.marked_range())
        .flatten();
    let revision = input.text_revision;
    let mut cache = std::mem::take(&mut input.line_cache);
    let source = if placeholder {
        input.placeholder.as_ref()
    } else {
        input.buffer.text()
    };
    let mut source_start = 0;
    let mut active = Vec::new();
    let mut identities = Vec::new();
    #[cfg(test)]
    let mut misses = 0;

    for (index, raw_line) in source.split('\n').enumerate() {
        let display = expand_tabs(raw_line);
        let local_selection = selection
            .as_ref()
            .and_then(|range| range_on_line(range, source_start, raw_line));
        let local_marked = marked
            .as_ref()
            .and_then(|range| range_on_line(range, source_start, raw_line));
        let runs = styled_runs(
            display.len(),
            local_selection,
            local_marked,
            color,
            style,
            theme,
        );
        let font = style.font();
        let key = LineShapeKey {
            line_index: index,
            text: display,
            wrap_width_bits: wrap_width.map(|width| f32::from(width).to_bits()),
            font: font.clone(),
            font_size_bits: f32::from(font_size).to_bits(),
        };
        let needs_shape = cache
            .entries
            .get(&key)
            .is_none_or(|cached| cached.runs != runs || cached.layout.is_none());

        if needs_shape {
            let shared = SharedString::from(key.text.clone());
            let layout = match window
                .text_system()
                .shape_text(shared, font_size, &runs, wrap_width, None)
            {
                Ok(lines) => lines.into_iter().next(),
                Err(error) => {
                    crate::paint_error::log_once(&error);
                    None
                }
            };
            cache
                .entries
                .insert(key.clone(), CachedLine { runs, layout });
            #[cfg(test)]
            {
                misses += 1;
            }
        }
        identities.push((key.text.clone(), font, key.font_size_bits));
        active.push(key);
        source_start += raw_line.len() + '\n'.len_utf8();
    }

    cache.entries.retain(|key, _| {
        identities.get(key.line_index).is_some_and(|identity| {
            key.text == identity.0 && key.font == identity.1 && key.font_size_bits == identity.2
        })
    });
    cache.active = active;
    cache.revision = revision;
    #[cfg(test)]
    cache.miss_counts.push(misses);
    let rows = (0..cache.active.len())
        .filter_map(|index| cache.line(index))
        .map(|line| line.wrap_boundaries.len() + 1)
        .sum::<usize>();
    let width = (0..cache.active.len())
        .filter_map(|index| cache.line(index))
        .map(|line| line.width())
        .max()
        .unwrap_or(Pixels::ZERO);
    input.line_cache = cache;
    (rows.max(1), width)
}

fn range_on_line(range: &Range<usize>, line_start: usize, raw_line: &str) -> Option<Range<usize>> {
    let line_end = line_start + raw_line.len();
    let start = range.start.max(line_start);
    let end = range.end.min(line_end);
    (start < end).then(|| {
        display_offset(raw_line, start - line_start)..display_offset(raw_line, end - line_start)
    })
}

fn styled_runs(
    display_len: usize,
    selection: Option<Range<usize>>,
    marked: Option<Range<usize>>,
    color: Hsla,
    style: &TextStyle,
    theme: &Theme,
) -> Vec<TextRun> {
    let base = TextRun {
        len: display_len,
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    if display_len == 0 || (selection.is_none() && marked.is_none()) {
        return vec![base];
    }
    let mut edges = vec![0, display_len];
    if let Some(selection) = &selection {
        edges.extend([selection.start, selection.end]);
    }
    if let Some(marked) = &marked {
        edges.extend([marked.start, marked.end]);
    }
    edges.retain(|edge| *edge <= display_len);
    edges.sort_unstable();
    edges.dedup();
    edges
        .windows(2)
        .filter_map(|edge| {
            let (start, end) = (edge[0], edge[1]);
            (start < end).then(|| TextRun {
                len: end - start,
                background_color: selection
                    .as_ref()
                    .filter(|range| start >= range.start && end <= range.end)
                    .map(|_| theme.colors.selection),
                underline: marked
                    .as_ref()
                    .filter(|range| start >= range.start && end <= range.end)
                    .map(|_| UnderlineStyle {
                        color: Some(color),
                        thickness: theme.metrics.hairline,
                        wavy: false,
                    }),
                ..base.clone()
            })
        })
        .collect()
}

impl IntoElement for MultilineInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MultilineInputElement {
    type RequestLayoutState = ();
    type PrepaintState = MultilineLayout;

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
        let max_lines = self.max_lines;
        // The inherited text style is read **here**, not inside the measure closure: a measured
        // layout runs during `compute_layout`, outside the `with_text_style` scope the parent
        // `div` pushes around `request_layout`, so `window.text_style()` in there answers the
        // window default — black on Fleet's dark ground, with the default font and line height.
        // The composer looked emptier once you typed in it (#000000 on #0E1013 is 1.06:1).
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let mut measured_style = Style::default();
        // A measured leaf otherwise negotiates its intrinsic width with flexbox. A long token
        // then makes the leaf wider than the composer, so `shape_text` receives that escaped
        // width and has nothing to wrap. The value leaf always fills the already-shrunk column.
        measured_style.size.width = relative(1.0).into();
        let layout_id = window.request_measured_layout(
            measured_style,
            move |known_dimensions, available_space, window, cx| {
                let wrap_width = known_dimensions.width.or(match available_space.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let theme = cx.theme().clone();
                let (rows, shaped_width) = input.update(cx, |input, _cx| {
                    input.line_height = line_height;
                    prepare_lines(input, wrap_width, font_size, &text_style, &theme, window)
                });
                let width = wrap_width.unwrap_or(shaped_width);

                Size {
                    width,
                    height: line_height * rows.clamp(1, max_lines) as f32,
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

        let (lines, caret, content_height, mut scroll, reveal_caret, revision) =
            self.input.update(cx, |input, _cx| {
                input.line_height = line_height;
                input.last_bounds = Some(bounds);
                let caret = input.position_for_offset(input.buffer.cursor());
                let content_height = input.content_height();
                (
                    input.line_cache.take_lines(),
                    caret,
                    content_height,
                    input.scroll,
                    input.scroll_to_caret,
                    input.text_revision,
                )
            });

        // Keep the caret in view by sliding the block up when it runs past the eight-line box.
        let viewport = bounds.size.height;
        if reveal_caret && let Some(caret) = caret {
            scroll = scroll.min(caret.y);
            scroll = scroll.max(caret.y + line_height - viewport);
        }
        let max_scroll = (content_height - viewport).max(Pixels::ZERO);
        scroll = scroll.clamp(Pixels::ZERO, max_scroll);

        let caret = caret.map(|caret| {
            fill(
                Bounds::new(
                    point(bounds.left() + caret.x, bounds.top() + caret.y - scroll),
                    size(theme.metrics.focus_ring_w, line_height),
                ),
                theme.colors.accent,
            )
        });

        let thumb = scroll_thumb(scroll, max_scroll, viewport, theme.metrics.diff_thumb_min_h).map(
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
        );

        self.input.update(cx, |input, _cx| {
            input.scroll = scroll;
            input.scroll_to_caret = false;
        });

        MultilineLayout {
            lines,
            caret,
            thumb,
            scroll,
            revision,
        }
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
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        let line_height = window.line_height();
        let lines = std::mem::take(&mut prepaint.lines);
        let scroll = prepaint.scroll;
        let caret = prepaint.caret.take();
        let thumb = prepaint.thumb.take();
        let focused = focus_handle.is_focused(window);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let mut top = bounds.top() - scroll;
            for (_, line) in &lines {
                let height = line_height * (line.wrap_boundaries.len() + 1) as f32;
                if top + height > bounds.top() && top < bounds.bottom() {
                    let origin = point(bounds.left(), top);
                    if let Err(error) = line.paint_background(
                        origin,
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    ) {
                        crate::paint_error::log_once(&error);
                    }
                    if let Err(error) =
                        line.paint(origin, line_height, TextAlign::Left, None, window, cx)
                    {
                        crate::paint_error::log_once(&error);
                    }
                }
                top += height;
            }
            if focused && let Some(caret) = caret {
                window.paint_quad(caret);
            }
            if let Some(thumb) = thumb {
                window.paint_quad(thumb);
            }
        });

        self.input.update(cx, |input, _cx| {
            if input.text_revision == prepaint.revision {
                input.line_cache.restore_lines(lines);
            }
        });
    }
}

/// The string to shape and the runs that colour it: the value, or the placeholder when the
/// buffer is empty.
///
/// Selection is a background run and IME composition is an underlined run, which is why the
/// composer shapes its own text instead of composing [`crate::Text`].
///
/// Both colours are the theme's own tokens rather than the inherited [`TextStyle::color`]: the
/// composer is the one place a reader types, so its value is *always* the primary text token
/// (`Tone::Default`) and its placeholder the muted one, whatever the surrounding style says.
#[cfg(test)]
pub(super) fn content(
    input: &MultilineInput,
    theme: &Theme,
    style: &TextStyle,
) -> (SharedString, Vec<TextRun>) {
    let placeholder = input.buffer.is_empty();
    let display = if placeholder {
        input.placeholder.clone()
    } else {
        input.buffer.shared_text()
    };
    let color = if placeholder {
        theme.colors.text_muted
    } else {
        theme.colors.text
    };
    let base = TextRun {
        len: display.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    if placeholder || display.is_empty() {
        return (display, vec![base]);
    }

    let raw = input.buffer.text();
    let selection = input.buffer.selected_range();
    let selection = (!selection.is_empty())
        .then(|| display_offset(raw, selection.start)..display_offset(raw, selection.end));
    let marked = input
        .buffer
        .marked_range()
        .map(|range| display_offset(raw, range.start)..display_offset(raw, range.end));
    let runs = styled_runs(display.len(), selection, marked, color, style, theme);
    (display, runs)
}
