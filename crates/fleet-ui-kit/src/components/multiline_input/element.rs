use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId, InspectorElementId, LayoutId, PaintQuad, Pixels, SharedString, Size, Style,
    TextAlign, TextRun, TextStyle, UnderlineStyle, Window, WrappedLine, fill, point, prelude::*,
    size,
};

use super::MultilineInput;

use crate::theme::{ActiveTheme, Theme};

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
    lines: Vec<WrappedLine>,
    caret: Option<PaintQuad>,
    scroll: Pixels,
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
        let layout_id = window.request_measured_layout(
            Style::default(),
            move |known_dimensions, available_space, window, cx| {
                let wrap_width = known_dimensions.width.or(match available_space.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let (display, runs) = {
                    let theme = cx.theme().clone();
                    let this = input.read(cx);
                    content(this, &theme, &text_style)
                };

                let lines: Vec<WrappedLine> = match window
                    .text_system()
                    .shape_text(display, font_size, &runs, wrap_width, None)
                {
                    Ok(lines) => lines.into_iter().collect(),
                    Err(error) => {
                        crate::paint_error::log_once(&error);
                        Vec::new()
                    }
                };

                let width = wrap_width.unwrap_or_else(|| {
                    lines
                        .iter()
                        .map(|line| line.width())
                        .max()
                        .unwrap_or(Pixels::ZERO)
                });
                let rows = visual_rows(&lines);
                input.update(cx, |input, _cx| {
                    input.line_height = line_height;
                    input.line_layout = lines;
                });

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

        let (lines, caret, content_height, mut scroll) = self.input.update(cx, |input, _cx| {
            input.line_height = line_height;
            input.last_bounds = Some(bounds);
            let caret = input.position_for_offset(input.buffer.cursor());
            let content_height = input.content_height();
            (
                std::mem::take(&mut input.line_layout),
                caret,
                content_height,
                input.scroll,
            )
        });

        // Keep the caret in view by sliding the block up when it runs past the eight-line box.
        let viewport = bounds.size.height;
        if let Some(caret) = caret {
            scroll = scroll.min(caret.y);
            scroll = scroll.max(caret.y + line_height - viewport);
        }
        scroll = scroll.clamp(Pixels::ZERO, (content_height - viewport).max(Pixels::ZERO));

        let caret = caret.map(|caret| {
            fill(
                Bounds::new(
                    point(bounds.left() + caret.x, bounds.top() + caret.y - scroll),
                    size(theme.metrics.focus_ring_w, line_height),
                ),
                theme.colors.accent,
            )
        });

        self.input.update(cx, |input, _cx| input.scroll = scroll);

        MultilineLayout {
            lines,
            caret,
            scroll,
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
        let focused = focus_handle.is_focused(window);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let mut top = bounds.top() - scroll;
            for line in &lines {
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
        });

        self.input
            .update(cx, |input, _cx| input.line_layout = lines);
    }
}

/// How many visual rows a set of shaped lines occupies: one per logical line plus one per
/// wrap boundary.
pub(super) fn visual_rows(lines: &[WrappedLine]) -> usize {
    lines
        .iter()
        .map(|line| line.wrap_boundaries.len() + 1)
        .sum()
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
pub(super) fn content(
    input: &MultilineInput,
    theme: &Theme,
    style: &TextStyle,
) -> (SharedString, Vec<TextRun>) {
    let placeholder = input.buffer.is_empty();
    let display = if placeholder {
        input.placeholder.clone()
    } else {
        input.display_text.clone()
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

    let selection = input.buffer.selected_range();
    let marked = input.buffer.marked_range();
    let mut edges = vec![0, display.len()];
    if !selection.is_empty() {
        edges.push(selection.start);
        edges.push(selection.end);
    }
    if let Some(marked) = &marked {
        edges.push(marked.start);
        edges.push(marked.end);
    }
    edges.retain(|edge| *edge <= display.len());
    edges.sort_unstable();
    edges.dedup();

    let runs = edges
        .windows(2)
        .map(|edge| {
            let (start, end) = (edge[0], edge[1]);
            TextRun {
                len: end - start,
                background_color: (!selection.is_empty()
                    && start >= selection.start
                    && end <= selection.end)
                    .then_some(theme.colors.selection),
                underline: marked
                    .as_ref()
                    .filter(|marked| start >= marked.start && end <= marked.end)
                    .map(|_| UnderlineStyle {
                        color: Some(color),
                        thickness: theme.metrics.hairline,
                        wavy: false,
                    }),
                ..base.clone()
            }
        })
        .filter(|run| run.len > 0)
        .collect();
    (display, runs)
}
