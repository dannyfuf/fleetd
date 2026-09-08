//! Turning a [`MarkdownDocument`] into elements, using design tokens only.
//!
//! The geometry is `docs/NATIVE-AGENTS.md` §2 and §8: assistant prose sits on the ground with
//! no bubble, body text is the 13 / 18 `ui` role, inline code is a low-alpha neutral chip and a
//! fenced block sits on the panel token in mono 12.5 / 18.
//!
//! A paragraph is one wrapping flex row of word-sized children rather than one shaped string.
//! That is the price of the chip: a rounded, padded inline mark is a *box*, and a text run
//! cannot be one. Splitting on word boundaries is what keeps the row wrapping like prose
//! instead of clipping. Fenced code has no such constraint, so it stays a single `StyledText`
//! with the ADR 0005 highlight-range mechanism.

use std::ops::Range;

use gpui::{
    AnyElement, App, DefiniteLength, Div, FontFeatures, FontWeight, HighlightStyle, Hsla,
    SharedString, StyledText, TextStyle, WhiteSpace, div, prelude::*, px,
};

use super::code::{self, CodeToken};
use super::{MarkdownBlock, MarkdownDocument, MarkdownInline};
use crate::text::styled_with;
use crate::theme::{ActiveTheme, Theme, TypeStyle, ch};

/// How deep quotes, lists and emphasis may nest before the rest is flattened to plain text.
///
/// The parser already bounds what it produces; this bounds what a *caller* can hand in, because
/// `markdown` is public and a hand-built document is not the parser's output.
const MAX_DEPTH: usize = 12;

/// How opaque the inline-code fill is over the ground.
const CODE_FILL_ALPHA: f32 = 0.08;

/// The heading level at which the screen-title scale gives way to the emphasised body scale.
const TITLE_LEVELS: u8 = 2;

/// Render a whole document.
pub(super) fn render(document: &MarkdownDocument, cx: &App) -> Div {
    let theme = cx.theme();
    styled_with(div(), theme.text.ui, theme)
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.sm)
        .text_color(theme.colors.text)
        .children(blocks(&document.blocks, theme, 0))
}

fn blocks(nodes: &[MarkdownBlock], theme: &Theme, depth: usize) -> Vec<AnyElement> {
    nodes.iter().map(|node| block(node, theme, depth)).collect()
}

fn block(node: &MarkdownBlock, theme: &Theme, depth: usize) -> AnyElement {
    match node {
        MarkdownBlock::Paragraph(inlines) => {
            inline_flow(inlines, theme.text.ui, theme, depth).into_any_element()
        }
        MarkdownBlock::Heading { level, inlines } => {
            let style = if *level <= TITLE_LEVELS {
                theme.text.title
            } else {
                theme.text.ui_strong
            };
            inline_flow(inlines, style, theme, depth).into_any_element()
        }
        MarkdownBlock::Code { lang, text } => code_block(lang.as_deref(), text, theme),
        MarkdownBlock::List { ordered, items } => list(*ordered, items, theme, depth),
        MarkdownBlock::Quote(inner) => quote(inner, theme, depth),
        MarkdownBlock::Rule => div()
            .flex_none()
            .w_full()
            .h(theme.metrics.hairline)
            .bg(theme.colors.border)
            .into_any_element(),
    }
}

// ---------------------------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------------------------

/// A list with the hanging indent §8 asks for: the marker column is fixed, so wrapped item text
/// lines up under the first word rather than under the bullet.
fn list(ordered: bool, items: &[Vec<MarkdownBlock>], theme: &Theme, depth: usize) -> AnyElement {
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

    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.xs)
        .children(items.iter().zip(markers).map(|(item, marker)| {
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
                        .children(blocks(item, theme, depth + 1)),
                )
        }))
        .into_any_element()
}

/// A quote: a 2 px bar in the strong hairline plus secondary text, no fill.
fn quote(inner: &[MarkdownBlock], theme: &Theme, depth: usize) -> AnyElement {
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
                .children(blocks(inner, theme, depth + 1)),
        )
        .into_any_element()
}

/// A fenced block: the panel token, 4 px radius, 8 × 12 px padding, mono 12.5 / 18.
fn code_block(lang: Option<&str>, text: &str, theme: &Theme) -> AnyElement {
    let container = mono(div(), theme)
        .flex_none()
        .w_full()
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
    let highlights = highlight_styles(code::highlight(lang, text), theme);
    container
        .child(
            StyledText::new(SharedString::from(text.to_owned()))
                .with_default_highlights(&style, highlights),
        )
        .into_any_element()
}

/// Resolve scanner buckets into the highlight ranges `StyledText` wants.
fn highlight_styles(
    spans: Vec<(Range<usize>, CodeToken)>,
    theme: &Theme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    spans
        .into_iter()
        .map(|(range, token)| {
            (
                range,
                HighlightStyle {
                    color: Some(token.color(theme)),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect()
}

/// The style `StyledText` shapes a code block with. It must agree with [`mono`] on the
/// container, or the element's own layout and the runs it is handed disagree about line height.
fn code_style(theme: &Theme) -> TextStyle {
    TextStyle {
        color: theme.colors.text,
        font_family: theme.font_mono.clone(),
        font_features: FontFeatures::disable_ligatures(),
        font_size: theme.text.data.size.into(),
        line_height: DefiniteLength::Absolute(theme.text.data.line_height.into()),
        font_weight: FontWeight::NORMAL,
        white_space: WhiteSpace::Normal,
        ..TextStyle::default()
    }
}

/// Apply the mono data role to a container.
fn mono(element: Div, theme: &Theme) -> Div {
    element
        .font_family(theme.font_mono.clone())
        .text_size(theme.text.data.size)
        .line_height(theme.text.data.line_height)
}

// ---------------------------------------------------------------------------------------------
// Inline flow
// ---------------------------------------------------------------------------------------------

/// Which marks a chunk inherits from the inline nodes containing it.
#[derive(Clone, Copy, Debug, Default)]
struct Marks {
    strong: bool,
    emphasis: bool,
    link: bool,
}

/// One wrapping row of inline chunks.
fn inline_flow(inlines: &[MarkdownInline], style: TypeStyle, theme: &Theme, depth: usize) -> Div {
    let mut chunks = Vec::new();
    push_inlines(&mut chunks, inlines, Marks::default(), theme, depth);
    styled_with(div(), style, theme)
        .flex()
        .flex_wrap()
        .items_baseline()
        .w_full()
        .children(chunks)
}

fn push_inlines(
    out: &mut Vec<AnyElement>,
    inlines: &[MarkdownInline],
    marks: Marks,
    theme: &Theme,
    depth: usize,
) {
    if depth >= MAX_DEPTH {
        let flat = flatten(inlines);
        if !flat.is_empty() {
            push_text(out, &flat, marks, theme);
        }
        return;
    }
    for node in inlines {
        match node {
            MarkdownInline::Text(text) => push_text(out, text, marks, theme),
            MarkdownInline::Code(text) => out.push(code_chip(text, marks, theme)),
            MarkdownInline::Strong(inner) => push_inlines(
                out,
                inner,
                Marks {
                    strong: true,
                    ..marks
                },
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
                theme,
                depth + 1,
            ),
        }
    }
}

/// Split a text node into wrappable chunks. A `\n` is the parser's hard line break and becomes
/// a full-width, zero-height item, which is what forces the wrapping row to break there.
fn push_text(out: &mut Vec<AnyElement>, text: &str, marks: Marks, theme: &Theme) {
    for (index, segment) in text.split('\n').enumerate() {
        if index > 0 {
            out.push(div().w_full().h(px(0.0)).into_any_element());
        }
        for chunk in words(segment) {
            out.push(
                marked(div(), marks, theme)
                    .child(SharedString::from(chunk.to_owned()))
                    .into_any_element(),
            );
        }
    }
}

/// Split a line into "one word plus the whitespace that follows it" chunks, which is the
/// smallest unit that may sit at the end of a wrapped line without losing its spacing.
fn words(segment: &str) -> impl Iterator<Item = &str> {
    let mut rest = segment;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let end = rest[word..]
            .find(|c: char| !c.is_whitespace())
            .map_or(rest.len(), |offset| word + offset);
        let (chunk, tail) = rest.split_at(end);
        rest = tail;
        Some(chunk)
    })
}

/// An inline code chip: 8 % neutral fill, 3 px radius, 4 px of horizontal padding.
///
/// Unlike a word, a chip does not shrink: squeezing the fill narrower than its text would put
/// the mark and the code it marks in different places.
fn code_chip(text: &str, marks: Marks, theme: &Theme) -> AnyElement {
    marked(styled_with(div(), theme.text.data, theme), marks, theme)
        .flex_none()
        .px(theme.space.xs)
        .rounded(theme.radii.xs)
        .bg(code_fill(theme))
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

/// Apply the inherited marks to one chunk container.
///
/// Word chunks keep the default shrink factor on purpose. A wrapping flex line only shrinks an
/// item that is wider than the row on its own, so ordinary words wrap and a 200-character URL
/// with no break in it is squeezed to the measure instead of running off the pane.
fn marked(element: Div, marks: Marks, theme: &Theme) -> Div {
    let element = if marks.strong {
        element.font_weight(FontWeight::MEDIUM)
    } else {
        element
    };
    let element = if marks.emphasis {
        element.italic()
    } else {
        element
    };
    if marks.link {
        element.text_color(theme.colors.accent)
    } else {
        element
    }
}

/// The inline-code fill. There is no token for it: it is the secondary text neutral at
/// [`CODE_FILL_ALPHA`], so it lifts off both the app ground and the panel token.
pub(super) fn code_fill(theme: &Theme) -> Hsla {
    Hsla {
        a: CODE_FILL_ALPHA,
        ..theme.colors.text_secondary
    }
}

/// The text an inline tree carries, ignoring its marks. Bounded by [`MAX_DEPTH`] like every
/// other walk here, so a hand-built document cannot recurse the renderer off the stack.
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

/// The chunks one inline tree renders to. The render tests assert the wrapping geometry through
/// this rather than through painted pixels.
#[cfg(test)]
pub(super) fn inline_chunk_count(inlines: &[MarkdownInline], theme: &Theme) -> usize {
    let mut chunks = Vec::new();
    push_inlines(&mut chunks, inlines, Marks::default(), theme, 0);
    chunks.len()
}
