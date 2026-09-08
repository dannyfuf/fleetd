//! `Markdown` — the transcript's prose surface.
//!
//! `docs/NATIVE-AGENTS.md` §8 asks for a focused in-house renderer rather than a general
//! Markdown crate, because the input is not a document: it is a stream. Text arrives a token at
//! a time, so half of everything here is about what a *prefix* must look like.
//!
//! The abstract syntax is deliberately small — paragraphs, ATX headings, fenced code, lists,
//! quotes, rules, and inline code / strong / emphasis / links. Tables, images and indented code
//! are out of scope and survive as their own source text.
//!
//! Two invariants hold, and both are tested:
//!
//! - **Total.** [`parse_markdown`] never panics and never loops, on any `&str`. Unterminated
//!   fences, unmatched delimiters and pathological nesting all have a defined shape.
//! - **Stable under growth.** For any prefix `p` of `s`, every block `parse_markdown(p)` has
//!   decided except its last one is also a block of `parse_markdown(s)`, at the same index. A
//!   transcript therefore never reflows behind the reader as the model keeps typing.
//!
//! `parser` documents how the subset differs from CommonMark and why; `render` documents the
//! geometry; `code` documents why fenced blocks are coloured locally rather than with the
//! ADR 0005 `syntect` stack.

use gpui::{App, prelude::*};

mod code;
mod parser;
mod render;

#[cfg(test)]
mod tests;

/// Parsed Markdown document supported by the native-agent transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkdownDocument {
    /// Ordered block nodes.
    pub blocks: Vec<MarkdownBlock>,
}

/// Supported block-level Markdown nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownBlock {
    /// A paragraph of inline nodes.
    Paragraph(Vec<MarkdownInline>),
    /// A fenced code block.
    Code {
        /// Optional fence language.
        lang: Option<String>,
        /// Literal code contents.
        text: String,
    },
    /// An ordered or unordered list.
    List {
        /// Whether numeric markers are used.
        ordered: bool,
        /// One nested block sequence per list item.
        items: Vec<Vec<MarkdownBlock>>,
    },
    /// A heading.
    Heading {
        /// Heading level, clamped by the renderer.
        level: u8,
        /// Heading contents.
        inlines: Vec<MarkdownInline>,
    },
    /// A quoted block sequence.
    Quote(Vec<MarkdownBlock>),
    /// A horizontal rule.
    Rule,
}

/// Supported inline Markdown nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownInline {
    /// Literal text.
    Text(String),
    /// Inline code.
    Code(String),
    /// Strong emphasis.
    Strong(Vec<MarkdownInline>),
    /// Emphasis.
    Emphasis(Vec<MarkdownInline>),
    /// A labelled URL.
    Link {
        /// Link label nodes.
        label: Vec<MarkdownInline>,
        /// Destination URL.
        url: String,
    },
}

/// Parses the supported streaming Markdown subset.
///
/// Total: every input maps to a document, including a partial one. An unterminated fence yields
/// the code block it has so far, and an unmatched `` ` ``, `*` or `[` stays literal text.
#[must_use]
pub fn parse_markdown(source: &str) -> MarkdownDocument {
    parser::parse(source)
}

/// Renders a parsed Markdown document using design-system tokens.
///
/// Every colour, size and radius comes from [`crate::theme::Theme`]; the element takes the width
/// it is given and grows downwards, so a transcript row can hand it the 760 px content measure.
pub fn markdown(document: &MarkdownDocument, cx: &App) -> impl IntoElement {
    render::render(document, cx)
}
