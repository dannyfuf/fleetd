use std::ops::Range;

use gpui::{
    App, Bounds, Element, ElementId, ElementInputHandler, Entity, GlobalElementId, Hsla,
    InspectorElementId, LayoutId, PaintQuad, Pixels, ShapedLine, SharedString, Style, TextAlign,
    TextRun, UnderlineStyle, Window, fill, point, prelude::*, px, relative, size,
};

use super::{TextInput, char_offset};

use crate::theme::{ActiveTheme, Theme};

/// The painted half of a [`TextInput`]: one shaped line, one caret, one marked-text underline.
pub(super) struct TextInputElement {
    pub(super) input: Entity<TextInput>,
}

/// What [`TextInputElement::prepaint`] hands to `paint`.
pub(super) struct FieldLineLayout {
    line: Option<ShapedLine>,
    caret: Option<PaintQuad>,
    scroll: Pixels,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = FieldLineLayout;

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
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
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
        let input = self.input.read(cx);
        let style = window.text_style();
        let empty = input.state.is_empty();
        let display: SharedString = if empty {
            input.placeholder.clone().unwrap_or_default()
        } else {
            input.display_text.clone()
        };
        let color = if empty {
            theme.colors.text_muted
        } else {
            style.color
        };
        let caret_offset = if empty { 0 } else { input.state.cursor() };
        let marked = if empty {
            None
        } else {
            input.state.marked_range()
        };

        prepare_line(display, caret_offset, marked, color, bounds, window, &theme)
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

        let scroll = prepaint.scroll;
        if let Some(line) = prepaint.line.take() {
            let origin = point(bounds.origin.x - scroll, bounds.origin.y);
            if let Err(error) = line.paint(
                origin,
                window.line_height(),
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                crate::paint_error::log_once(&error);
            }
            self.input.update(cx, |input, _cx| {
                input.last_layout = Some(line);
                input.last_bounds = Some(bounds);
                input.last_scroll = scroll;
            });
        }

        if focus_handle.is_focused(window)
            && let Some(caret) = prepaint.caret.take()
        {
            window.paint_quad(caret);
        }
    }
}

fn prepare_line(
    display: SharedString,
    caret_offset: usize,
    marked: Option<Range<usize>>,
    color: Hsla,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    theme: &Theme,
) -> FieldLineLayout {
    let style = window.text_style();
    let run = TextRun {
        len: display.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = match marked {
        Some(marked) if marked.end <= display.len() => vec![
            TextRun {
                len: marked.start,
                ..run.clone()
            },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle {
                    color: Some(color),
                    thickness: theme.metrics.hairline,
                    wavy: false,
                }),
                ..run.clone()
            },
            TextRun {
                len: display.len() - marked.end,
                ..run
            },
        ]
        .into_iter()
        .filter(|run| run.len > 0)
        .collect(),
        _ => vec![run],
    };

    let font_size = style.font_size.to_pixels(window.rem_size());
    let line = window
        .text_system()
        .shape_line(display, font_size, &runs, None);

    // Keep the caret in view by sliding the line left when it runs past the box.
    let caret_x = line.x_for_index(caret_offset);
    let caret_w = theme.metrics.focus_ring_w;
    let scroll = (caret_x + caret_w - bounds.size.width).max(px(0.0));
    let caret = fill(
        Bounds::new(
            point(bounds.left() + caret_x - scroll, bounds.top()),
            size(caret_w, bounds.size.height),
        ),
        theme.colors.accent,
    );

    FieldLineLayout {
        line: Some(line),
        caret: Some(caret),
        scroll,
    }
}

/// A single shaped line shared by controlled fields and the native input entity.
#[derive(IntoElement)]
pub(crate) struct FieldLine {
    value: SharedString,
    placeholder: SharedString,
    caret: Option<usize>,
    focused: bool,
}

impl FieldLine {
    pub(crate) fn new(
        value: SharedString,
        placeholder: Option<SharedString>,
        caret: Option<usize>,
        focused: bool,
    ) -> Self {
        Self {
            value,
            placeholder: placeholder.unwrap_or_default(),
            caret,
            focused,
        }
    }
}

impl RenderOnce for FieldLine {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let empty = self.value.is_empty();
        let caret = if empty {
            0
        } else {
            self.caret
                .map_or(self.value.len(), |index| char_offset(&self.value, index))
        };
        let display = if empty { self.placeholder } else { self.value };
        gpui::canvas(
            move |bounds, window, _cx| {
                let color = if empty {
                    theme.colors.text_muted
                } else {
                    window.text_style().color
                };
                prepare_line(display, caret, None, color, bounds, window, &theme)
            },
            move |bounds, mut layout, window, cx| {
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    if let Some(line) = layout.line.take()
                        && let Err(error) = line.paint(
                            point(bounds.left() - layout.scroll, bounds.top()),
                            bounds.size.height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                    {
                        crate::paint_error::log_once(&error);
                    }
                    if self.focused
                        && let Some(caret) = layout.caret.take()
                    {
                        window.paint_quad(caret);
                    }
                });
            },
        )
        .w_full()
        .h_full()
    }
}
