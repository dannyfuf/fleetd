//! `Markdown` — the transcript's prose surface.
//!
//! `docs/NATIVE-AGENTS.md` §8 asks for a focused in-house renderer rather than a general
//! Markdown crate, because the input is not a document: it is a stream. Text arrives a token at
//! a time, so half of everything here is about what a *prefix* must look like.
//!
//! The abstract syntax is deliberately small — paragraphs, ATX headings, fenced code, lists,
//! quotes, rules, GFM tables, and inline code / strong / emphasis / links. Images and indented
//! code are out of scope and survive as their own source text.
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

use std::{
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use gpui::{App, SharedString, TextAlign, prelude::*};

mod code;
mod parser;
mod render;
mod table;

pub use code::{CodeHighlights, HighlightCache};

#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod tests;

/// Parsed Markdown document supported by the native-agent transcript.
#[derive(Debug, Clone)]
pub struct MarkdownDocument {
    /// Ordered block nodes.
    pub blocks: Vec<MarkdownBlock>,
    open_source: String,
    settled_blocks: usize,
    cache: Rc<HighlightCache>,
    render_namespace: u64,
}

static NEXT_RENDER_NAMESPACE: AtomicU64 = AtomicU64::new(1);

fn next_render_namespace() -> u64 {
    NEXT_RENDER_NAMESPACE.fetch_add(1, Ordering::Relaxed)
}

impl Default for MarkdownDocument {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            open_source: String::new(),
            settled_blocks: 0,
            cache: Rc::new(HighlightCache::new()),
            render_namespace: next_render_namespace(),
        }
    }
}

impl PartialEq for MarkdownDocument {
    fn eq(&self, other: &Self) -> bool {
        self.blocks == other.blocks
    }
}

impl Eq for MarkdownDocument {}

impl MarkdownDocument {
    fn parsed(source: &str, blocks: Vec<MarkdownBlock>, starts: Vec<usize>) -> Self {
        let settled_blocks = blocks.len().saturating_sub(1);
        let open_start = starts.last().copied().unwrap_or(source.len());
        Self {
            blocks,
            open_source: source[open_start..].to_owned(),
            settled_blocks,
            cache: Rc::new(HighlightCache::new()),
            render_namespace: next_render_namespace(),
        }
    }

    /// Append one source delta, reparsing only the last block that may still grow.
    ///
    /// Completed blocks are retained byte-for-byte and closed fences reuse this document's
    /// [`HighlightCache`]. The work is therefore proportional to the open block, not the whole
    /// transcript. A default document is an empty stream, while a document returned by either
    /// parse entry point can continue streaming from its existing source.
    pub fn append(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.open_source.push_str(delta);
        let parsed = parser::parse_fragment(&self.open_source, Some(&self.cache));
        self.blocks.truncate(self.settled_blocks);
        let prior = self.blocks.len();
        let parsed_len = parsed.blocks.len();
        self.blocks.extend(parsed.blocks);
        if parsed_len > 0 {
            self.settled_blocks = prior + parsed_len - 1;
        }
        if let Some(start) = parsed.starts.last().copied()
            && start > 0
        {
            drop(self.open_source.drain(..start));
        }
    }

    /// Render this document, optionally appending a caret glyph to its final prose block.
    ///
    /// The caret is a separate text run and does not mutate the parsed source. Agent transcript
    /// rows can set `caret` while the assistant item is streaming.
    pub fn render_with_caret(&self, caret: bool, cx: &App) -> impl IntoElement {
        render::render(self, caret, cx)
    }
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
    /// A GFM table. Every cell contains the same inline nodes as a paragraph.
    Table {
        /// The emphasised header row.
        header: Vec<Vec<MarkdownInline>>,
        /// Column alignment decoded from the delimiter row.
        alignments: Vec<TextAlign>,
        /// Body rows, padded or clipped to the header's column count.
        rows: Vec<Vec<Vec<MarkdownInline>>>,
    },
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
    render::render(document, false, cx)
}
