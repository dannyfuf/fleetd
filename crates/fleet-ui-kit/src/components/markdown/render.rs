//! Turning a [`MarkdownDocument`] into elements, using design tokens only.
//!
//! Prose blocks are each one [`StyledText`] with [`TextRun`](gpui::TextRun)s. GPUI therefore
//! wraps the paragraph as text — including an individual token wider than the measure — while
//! strong, emphasis, code and links remain inline marks. Fenced code deliberately does the
//! opposite: it is nowrap content inside an axis-restricted horizontal scroller.

use std::ops::Range;

use gpui::{
    AnyElement, App, DefiniteLength, Div, FontFeatures, FontStyle, FontWeight, HighlightStyle,
    SharedString, StyledText, TextAlign, TextRun, TextStyle, UnderlineStyle, WhiteSpace, div,
    prelude::*,
};

use super::code::{CodeHighlights, CodeToken};
use super::{MarkdownBlock, MarkdownDocument, MarkdownInline};
use crate::text::styled_with;
use crate::theme::{ActiveTheme, FontRole, Theme, TypeStyle, ch};
use crate::tone::Tone;

/// How deep quotes, lists and emphasis may nest before the rest is flattened to plain text.
const MAX_DEPTH: usize = 12;

/// The heading level at which the screen-title scale gives way to the emphasised body scale.
const TITLE_LEVELS: u8 = 2;

#[derive(Default)]
struct RenderState {
    next_code: usize,
    namespace: u64,
}

/// Render a whole document.
pub(super) fn render(document: &MarkdownDocument, caret: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let mut state = RenderState {
        namespace: document.render_namespace,
        ..RenderState::default()
    };
    styled_with(div(), theme.text.ui, theme)
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .gap(theme.space.sm)
        .text_color(theme.colors.text)
        .children(blocks(&document.blocks, theme, 0, caret, &mut state))
}

fn blocks(
    nodes: &[MarkdownBlock],
    theme: &Theme,
    depth: usize,
    caret: bool,
    state: &mut RenderState,
) -> Vec<AnyElement> {
    let caret_at = caret.then(|| nodes.iter().rposition(has_text)).flatten();
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| block(node, theme, depth, caret_at == Some(index), state))
        .collect()
}

fn block(
    node: &MarkdownBlock,
    theme: &Theme,
    depth: usize,
    caret: bool,
    state: &mut RenderState,
) -> AnyElement {
    match node {
        MarkdownBlock::Paragraph(inlines) => {
            inline_text(inlines, theme.text.ui, theme, depth, caret).into_any_element()
        }
        MarkdownBlock::Heading { level, inlines } => {
            let style = if *level <= TITLE_LEVELS {
                theme.text.title
            } else {
                theme.text.ui_strong
            };
            inline_text(inlines, style, theme, depth, caret).into_any_element()
        }
        MarkdownBlock::Code {
            text, highlights, ..
        } => {
            let id = state.next_code;
            state.next_code += 1;
            code_block(text, highlights, theme, state.namespace, id)
        }
        MarkdownBlock::List { ordered, items } => list(*ordered, items, theme, depth, caret, state),
        MarkdownBlock::Quote(inner) => quote(inner, theme, depth, caret, state),
        MarkdownBlock::Table {
            header,
            alignments,
            rows,
        } => table(header, alignments, rows, theme, depth, caret),
        MarkdownBlock::Rule => div()
            .flex_none()
            .w_full()
            .h(theme.metrics.hairline)
            .bg(theme.colors.border)
            .into_any_element(),
    }
}

fn has_text(block: &MarkdownBlock) -> bool {
    match block {
        MarkdownBlock::Paragraph(_) | MarkdownBlock::Heading { .. } => true,
        MarkdownBlock::List { items, .. } => items.iter().flatten().any(has_text),
        MarkdownBlock::Quote(inner) => inner.iter().any(has_text),
        MarkdownBlock::Table { header, rows, .. } => {
            !header.is_empty() || rows.iter().any(|row| !row.is_empty())
        }
        MarkdownBlock::Code { .. } | MarkdownBlock::Rule => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------------------------

fn list(
    ordered: bool,
    items: &[Vec<MarkdownBlock>],
    theme: &Theme,
    depth: usize,
    caret: bool,
    state: &mut RenderState,
) -> AnyElement {
    if depth >= MAX_DEPTH {
        return div().into_any_element();
    }
    let markers: Vec<SharedString> = (0..items.len())
        .map(|index| {
            if ordered {
                SharedString::from(format!("{}.", index + 1))
            } else {
                SharedString::new_static("•")
            }
        })
        .collect();
    let column = ch(markers
        .iter()
        .map(|marker| marker.chars().count())
        .max()
        .unwrap_or(1) as f32
        + 1.0);
    let caret_at = caret
        .then(|| items.iter().rposition(|item| item.iter().any(has_text)))
        .flatten();

    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.xs)
        .children(
            items
                .iter()
                .zip(markers)
                .enumerate()
                .map(|(index, (item, marker))| {
                    div()
                        .flex()
                        .w_full()
                        .items_start()
                        .child(
                            styled_with(div(), theme.text.data, theme)
                                .flex_none()
                                .w(column)
                                .text_color(theme.colors.text_secondary)
                                .child(marker),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(theme.space.xs)
                                .children(blocks(
                                    item,
                                    theme,
                                    depth + 1,
                                    caret_at == Some(index),
                                    state,
                                )),
                        )
                }),
        )
        .into_any_element()
}

fn quote(
    inner: &[MarkdownBlock],
    theme: &Theme,
    depth: usize,
    caret: bool,
    state: &mut RenderState,
) -> AnyElement {
    if depth >= MAX_DEPTH {
        return div().into_any_element();
    }
    div()
        .flex()
        .w_full()
        .gap(theme.space.md)
        .child(
            div()
                .flex_none()
                .w(theme.space.xxs)
                .rounded(theme.radii.xs)
                .bg(theme.colors.border_strong),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(theme.space.sm)
                .text_color(theme.colors.text_secondary)
                .children(blocks(inner, theme, depth + 1, caret, state)),
        )
        .into_any_element()
}

fn table(
    header: &[Vec<MarkdownInline>],
    alignments: &[TextAlign],
    rows: &[Vec<Vec<MarkdownInline>>],
    theme: &Theme,
    depth: usize,
    caret: bool,
) -> AnyElement {
    let body_caret = caret && !rows.is_empty();
    let last_row = rows.len().saturating_sub(1);
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .child(
            div()
                .flex()
                .w_full()
                .border_b(theme.metrics.hairline)
                .border_color(theme.colors.border)
                .children(header.iter().enumerate().map(|(column, cell)| {
                    table_cell(
                        cell,
                        theme.text.ui_strong,
                        alignments.get(column).copied().unwrap_or_default(),
                        theme,
                        depth,
                        caret && rows.is_empty() && column + 1 == header.len(),
                    )
                })),
        )
        .children(rows.iter().enumerate().map(|(row_index, row)| {
            div()
                .flex()
                .w_full()
                .children(row.iter().enumerate().map(|(column, cell)| {
                    table_cell(
                        cell,
                        theme.text.ui,
                        alignments.get(column).copied().unwrap_or_default(),
                        theme,
                        depth,
                        body_caret && row_index == last_row && column + 1 == row.len(),
                    )
                }))
        }))
        .into_any_element()
}

fn table_cell(
    inlines: &[MarkdownInline],
    style: TypeStyle,
    alignment: TextAlign,
    theme: &Theme,
    depth: usize,
    caret: bool,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .px(theme.space.sm)
        .py(theme.space.xs)
        .text_align(alignment)
        .child(inline_text(inlines, style, theme, depth + 1, caret))
}

// ---------------------------------------------------------------------------------------------
// Fenced code
// ---------------------------------------------------------------------------------------------

fn code_block(
    text: &SharedString,
    highlights: &CodeHighlights,
    theme: &Theme,
    namespace: u64,
    id: usize,
) -> AnyElement {
    let element_id = namespace.wrapping_mul(31).wrapping_add(id as u64);
    let container = styled_with(div(), theme.text.data, theme)
        .id(("markdown-code", element_id))
        .debug_selector(|| "markdown-code-scroll".to_owned())
        .flex_none()
        .w_full()
        .min_w_0()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .whitespace_nowrap()
        .bg(theme.colors.surface)
        .rounded(theme.radii.sm)
        .py(theme.space.sm)
        .px(theme.space.md);
    if text.is_empty() {
        return container
            .h(theme.text.data.line_height + theme.space.sm + theme.space.sm)
            .into_any_element();
    }
    let style = code_style(theme);
    let highlights = highlight_styles(highlights.spans(), theme);
    container
        .child(StyledText::new(text.clone()).with_default_highlights(&style, highlights))
        .into_any_element()
}

fn highlight_styles(
    spans: &[(Range<usize>, CodeToken)],
    theme: &Theme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .map(|(range, token)| {
            (
                range.clone(),
                HighlightStyle {
                    color: Some(token.color(theme)),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect()
}

fn code_style(theme: &Theme) -> TextStyle {
    TextStyle {
        color: theme.colors.text,
        font_family: theme.font_mono.clone(),
        font_features: FontFeatures::disable_ligatures(),
        font_size: theme.text.data.size.into(),
        line_height: DefiniteLength::Absolute(theme.text.data.line_height.into()),
        font_weight: FontWeight::NORMAL,
        white_space: WhiteSpace::Nowrap,
        ..TextStyle::default()
    }
}

// ---------------------------------------------------------------------------------------------
// Inline text
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct Marks {
    strong: bool,
    emphasis: bool,
    link: bool,
}

struct InlineRuns {
    text: String,
    runs: Vec<TextRun>,
}

fn inline_text(
    inlines: &[MarkdownInline],
    style: TypeStyle,
    theme: &Theme,
    depth: usize,
    caret: bool,
) -> Div {
    let base = prose_style(style, theme);
    let mut out = InlineRuns {
        text: String::new(),
        runs: Vec::new(),
    };
    push_inlines(&mut out, inlines, Marks::default(), &base, theme, depth);
    if caret {
        push_run(&mut out, "▍", caret_style(&base, theme));
    }
    let text = StyledText::new(SharedString::from(out.text)).with_runs(out.runs);
    styled_with(div(), style, theme)
        .debug_selector(|| "markdown-inline-text".to_owned())
        .w_full()
        .min_w_0()
        .child(text)
}

fn push_inlines(
    out: &mut InlineRuns,
    inlines: &[MarkdownInline],
    marks: Marks,
    base: &TextStyle,
    theme: &Theme,
    depth: usize,
) {
    if depth >= MAX_DEPTH {
        push_run(out, &flatten(inlines), marked_style(base, marks, theme));
        return;
    }
    for inline in inlines {
        match inline {
            MarkdownInline::Text(text) => {
                push_run(out, text, marked_style(base, marks, theme));
            }
            MarkdownInline::Code(text) => {
                let mut style = marked_style(base, marks, theme);
                style.font_family = theme.font_mono.clone();
                style.font_features = FontFeatures::disable_ligatures();
                style.background_color = Some(code_fill(theme));
                push_run(out, text, style);
            }
            MarkdownInline::Strong(inner) => push_inlines(
                out,
                inner,
                Marks {
                    strong: true,
                    ..marks
                },
                base,
                theme,
                depth + 1,
            ),
            MarkdownInline::Emphasis(inner) => push_inlines(
                out,
                inner,
                Marks {
                    emphasis: true,
                    ..marks
                },
                base,
                theme,
                depth + 1,
            ),
            MarkdownInline::Link { label, .. } => push_inlines(
                out,
                label,
                Marks {
                    link: true,
                    ..marks
                },
                base,
                theme,
                depth + 1,
            ),
        }
    }
}

fn push_run(out: &mut InlineRuns, text: &str, style: TextStyle) {
    if text.is_empty() {
        return;
    }
    out.text.push_str(text);
    out.runs.push(style.to_run(text.len()));
}

fn prose_style(style: TypeStyle, theme: &Theme) -> TextStyle {
    let font_family = match style.font {
        FontRole::Ui => theme.font_ui.clone(),
        FontRole::Mono => theme.font_mono.clone(),
    };
    TextStyle {
        color: theme.colors.text,
        font_family,
        font_size: style.size.into(),
        line_height: DefiniteLength::Absolute(style.line_height.into()),
        font_weight: style.weight,
        white_space: WhiteSpace::Normal,
        ..TextStyle::default()
    }
}

fn marked_style(base: &TextStyle, marks: Marks, theme: &Theme) -> TextStyle {
    let mut style = base.clone();
    if marks.strong {
        style.font_weight = FontWeight::MEDIUM;
    }
    if marks.emphasis {
        style.font_style = FontStyle::Italic;
    }
    if marks.link {
        style.color = theme.colors.accent;
        style.underline = Some(UnderlineStyle {
            thickness: theme.metrics.hairline,
            color: Some(theme.colors.accent),
            wavy: false,
        });
    }
    style
}

fn caret_style(base: &TextStyle, theme: &Theme) -> TextStyle {
    let mut style = base.clone();
    style.color = theme.colors.accent;
    style
}

pub(super) fn code_fill(theme: &Theme) -> gpui::Hsla {
    Tone::Secondary.fill(theme)
}

/// The text an inline tree carries, ignoring its marks.
pub(super) fn flatten(inlines: &[MarkdownInline]) -> String {
    let mut out = String::new();
    flatten_into(&mut out, inlines, 0);
    out
}

fn flatten_into(out: &mut String, inlines: &[MarkdownInline], depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    for node in inlines {
        match node {
            MarkdownInline::Text(text) | MarkdownInline::Code(text) => out.push_str(text),
            MarkdownInline::Strong(inner)
            | MarkdownInline::Emphasis(inner)
            | MarkdownInline::Link { label: inner, .. } => flatten_into(out, inner, depth + 1),
        }
    }
}

#[cfg(test)]
pub(super) fn inline_run_count(inlines: &[MarkdownInline], theme: &Theme) -> usize {
    let base = prose_style(theme.text.ui, theme);
    let mut out = InlineRuns {
        text: String::new(),
        runs: Vec::new(),
    };
    push_inlines(&mut out, inlines, Marks::default(), &base, theme, 0);
    out.runs.len()
}
