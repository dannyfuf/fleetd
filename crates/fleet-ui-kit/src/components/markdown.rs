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

use gpui::{App, SharedString, prelude::*};

mod code;
mod parser;
mod render;

pub use code::{CodeHighlights, HighlightCache};

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
    /// A fenced code block. Build one with [`MarkdownBlock::code`], which colours it, or with
    /// [`MarkdownBlock::streaming_code`], which deliberately does not.
    Code {
        /// Optional fence language.
        lang: Option<String>,
        /// Literal code contents.
        text: SharedString,
        /// The block's colour spans, resolved here so the renderer never lexes. Empty while
        /// the fence is still open.
        highlights: CodeHighlights,
        /// Whether the closing fence has arrived.
        ///
        /// An open fence is drawn as code — from its first line, so the block does not appear
        /// as prose and then reflow — but it is **not** highlighted: a fence whose colours
        /// changed per chunk would move the reader's eye on every token.
        closed: bool,
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

impl MarkdownBlock {
    /// A closed fenced code block, lexed now rather than every time it is drawn.
    #[must_use]
    pub fn code(lang: Option<String>, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        let highlights = CodeHighlights::new(lang.as_deref(), &text);
        MarkdownBlock::Code {
            lang,
            text,
            highlights,
            closed: true,
        }
    }

    /// A closed fenced code block whose colours come from `cache`, lexing only on a miss.
    #[must_use]
    pub fn cached_code(
        lang: Option<String>,
        text: impl Into<SharedString>,
        cache: &HighlightCache,
    ) -> Self {
        let text = text.into();
        let highlights = cache.highlight(lang.as_deref(), &text);
        MarkdownBlock::Code {
            lang,
            text,
            highlights,
            closed: true,
        }
    }

    /// A fence that has not closed yet: code, uncoloured, and never in the cache.
    #[must_use]
    pub fn streaming_code(lang: Option<String>, text: impl Into<SharedString>) -> Self {
        MarkdownBlock::Code {
            lang,
            text: text.into(),
            highlights: CodeHighlights::default(),
            closed: false,
        }
    }
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
/// the code block it has so far — uncoloured, because it is still growing — and an unmatched
/// `` ` ``, `*` or `[` stays literal text.
#[must_use]
pub fn parse_markdown(source: &str) -> MarkdownDocument {
    parser::parse(source, None)
}

/// Parses the same subset, reusing already-lexed fences from `cache`.
///
/// A streaming transcript re-parses the same prose on every delta, so a settled fence would
/// otherwise be re-lexed once per token. **A partial fence is neither read from nor written to
/// the cache** — see [`HighlightCache`].
#[must_use]
pub fn parse_markdown_cached(source: &str, cache: &HighlightCache) -> MarkdownDocument {
    parser::parse(source, Some(cache))
}

/// Renders a parsed Markdown document using design-system tokens.
///
/// Every colour, size and radius comes from [`crate::theme::Theme`]; the element takes the width
/// it is given and grows downwards, so a transcript row can hand it the 760 px content measure.
pub fn markdown(document: &MarkdownDocument, cx: &App) -> impl IntoElement {
    render::render(document, cx)
}
