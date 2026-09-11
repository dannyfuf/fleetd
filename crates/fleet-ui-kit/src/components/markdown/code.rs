//! A dependency-free colouriser for fenced code blocks.
//!
//! ADR 0005 puts `syntect` + `two-face` behind Fleet's diff highlighting, and
//! `docs/NATIVE-AGENTS.md` §8 asks for the same treatment here. `fleet-ui-kit` cannot reach it:
//! the crate depends on `gpui` and nothing else (see `lib.rs`, rule 1), and `fleet-lazygit`,
//! which owns the `syntect` wrapper, depends on *this* crate. So the mechanism ADR 0005 fixes
//! is kept — a sorted, disjoint range list handed to `StyledText::with_default_highlights` —
//! and only the token source is local: a lexer that recognises comments, string literals,
//! numbers and a shared keyword set.
//!
//! It is deliberately shallow. Function names, types and members need a grammar, so they stay
//! plain rather than being guessed at. The four buckets it does produce use the same colours
//! `fleet-lazygit`'s `Bucket` resolves to, so a snippet in a transcript and the same snippet in
//! a diff do not disagree.

use std::{
    cell::RefCell,
    collections::VecDeque,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    sync::Arc,
};

use gpui::Hsla;

use crate::theme::Theme;

/// ANSI slot 2 in [`crate::theme::TerminalPalette`]: string literals.
const ANSI_GREEN: usize = 2;
/// ANSI slot 3: numeric literals.
const ANSI_YELLOW: usize = 3;
/// ANSI slot 5: keywords.
const ANSI_MAGENTA: usize = 5;

/// One colour bucket a code span can land in. Anything else is left at the block's text colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CodeToken {
    /// A line or block comment.
    Comment,
    /// A quoted string or character literal.
    Literal,
    /// A numeric literal.
    Number,
    /// A word from [`KEYWORDS`].
    Keyword,
}

impl CodeToken {
    /// The token colour, from the same theme entries `fleet-lazygit` resolves its buckets to.
    pub(super) fn color(self, theme: &Theme) -> Hsla {
        match self {
            CodeToken::Comment => theme.colors.text_muted,
            CodeToken::Literal => theme.terminal.ansi[ANSI_GREEN],
            CodeToken::Number => theme.terminal.ansi[ANSI_YELLOW],
            CodeToken::Keyword => theme.terminal.ansi[ANSI_MAGENTA],
        }
    }
}

/// The comment and string spellings a fence's info string maps onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// `//`, `/* */`, and back-ticked template strings.
    CLike,
    /// `#` to end of line.
    Hash,
    /// `--` to end of line.
    Dash,
}

/// Map a fence info string to a lexer family. An unknown or absent language is left plain:
/// guessing a comment character over arbitrary text colours more than it explains.
fn family(lang: Option<&str>) -> Option<Family> {
    let lang = lang?.to_ascii_lowercase();
    let family = match lang.as_str() {
        "c" | "c++" | "cc" | "cpp" | "cs" | "csharp" | "dart" | "go" | "groovy" | "h" | "hpp"
        | "java" | "javascript" | "js" | "json" | "json5" | "jsonc" | "jsx" | "kotlin" | "kt"
        | "objc" | "php" | "proto" | "rs" | "rust" | "scala" | "swift" | "ts" | "tsx"
        | "typescript" | "zig" => Family::CLike,
        "bash" | "cmake" | "conf" | "dockerfile" | "elixir" | "fish" | "ini" | "julia" | "just"
        | "make" | "makefile" | "nix" | "perl" | "pl" | "powershell" | "ps1" | "py" | "python"
        | "r" | "rb" | "ruby" | "sh" | "shell" | "toml" | "yaml" | "yml" | "zsh" => Family::Hash,
        "elm" | "hs" | "haskell" | "lua" | "postgres" | "psql" | "sql" | "sqlite" => Family::Dash,
        _ => return None,
    };
    Some(family)
}

/// The colour spans of one fenced block, resolved when the document is built.
///
/// A block is lexed once, by [`CodeHighlights::new`], and the renderer only maps the buckets it
/// already holds onto theme colours: `gpui-performance` rule 1 keeps the scan out of the draw
/// path, where a streaming transcript would repeat it every frame.
///
/// The spans are shared rather than owned because a transcript row clones its document to draw
/// it: a long fence has thousands of them, and copying that vector per frame would trade the
/// scan for a memcpy of the same order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeHighlights(Arc<[(Range<usize>, CodeToken)]>);

impl CodeHighlights {
    /// Lex `text` as `lang`.
    pub(super) fn new(lang: Option<&str>, text: &str) -> Self {
        Self(highlight(lang, text).into())
    }

    /// The buckets, as sorted, disjoint byte ranges over the block's text.
    pub(super) fn spans(&self) -> &[(Range<usize>, CodeToken)] {
        &self.0
    }
}

#[cfg(test)]
thread_local! {
    /// How many times [`highlight`] has run on this thread, so a test can prove a block is
    /// lexed once when its document is built rather than once per frame.
    pub(super) static HIGHLIGHT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Colour one fenced block, as sorted, disjoint byte ranges over `text`.
///
/// The ranges are what `StyledText::with_default_highlights` wants; producing them out of order
/// or overlapping makes it panic, which is why the scanner only ever moves forward.
pub(super) fn highlight(lang: Option<&str>, text: &str) -> Vec<(Range<usize>, CodeToken)> {
    #[cfg(test)]
    HIGHLIGHT_CALLS.with(|calls| calls.set(calls.get() + 1));
    let Some(family) = family(lang) else {
        return Vec::new();
    };
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(end) = comment_end(family, bytes, index) {
            spans.push((index..end, CodeToken::Comment));
            index = end;
        } else if let Some(end) = string_end(family, bytes, index) {
            spans.push((index..end, CodeToken::Literal));
            index = end;
        } else if byte.is_ascii_digit() && !index.checked_sub(1).is_some_and(|i| is_word(bytes[i]))
        {
            let end = number_end(bytes, index);
            spans.push((index..end, CodeToken::Number));
            index = end;
        } else if is_word_start(byte) {
            let end = word_end(bytes, index);
            if is_keyword(&text[index..end]) {
                spans.push((index..end, CodeToken::Keyword));
            }
            index = end;
        } else {
            index += 1;
        }
    }
    spans
}

/// Where a comment starting at `index` ends, or `None` if none starts there.
fn comment_end(family: Family, bytes: &[u8], index: usize) -> Option<usize> {
    let rest = &bytes[index..];
    let line = match family {
        Family::CLike => rest.starts_with(b"//"),
        Family::Hash => rest.starts_with(b"#"),
        Family::Dash => rest.starts_with(b"--"),
    };
    if line {
        let end = bytes[index..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| index + offset);
        return Some(end);
    }
    if family == Family::CLike && rest.starts_with(b"/*") {
        // An unterminated block comment runs to the end: a streaming block has no closer yet.
        let end = bytes[index + 2..]
            .windows(2)
            .position(|pair| pair == b"*/")
            .map_or(bytes.len(), |offset| index + 2 + offset + 2);
        return Some(end);
    }
    None
}

/// Where a string starting at `index` ends, or `None` if none starts there.
///
/// A quote with no closer ends at the line break, so one stray apostrophe in a comment-free
/// language cannot paint the rest of the block green. A back-ticked template literal is the one
/// exception that may span lines.
fn string_end(family: Family, bytes: &[u8], index: usize) -> Option<usize> {
    let quote = match bytes[index] {
        b'"' => b'"',
        b'\'' => b'\'',
        b'`' if family == Family::CLike => b'`',
        _ => return None,
    };
    let multiline = quote == b'`';
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'\n' if !multiline => return Some(cursor),
            byte if byte == quote => return Some(cursor + 1),
            _ => cursor += 1,
        }
    }
    Some(bytes.len())
}

/// Where a numeric literal ends. Hex digits, exponents, separators and suffixes all stay in.
fn number_end(bytes: &[u8], index: usize) -> usize {
    let mut cursor = index;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        // A `.` only continues the literal when a digit follows it, so `1.max(2)` keeps its
        // method call plain.
        let continues = byte.is_ascii_alphanumeric()
            || byte == b'_'
            || (byte == b'.' && bytes.get(cursor + 1).is_some_and(u8::is_ascii_digit));
        if !continues {
            break;
        }
        cursor += 1;
    }
    cursor
}

fn word_end(bytes: &[u8], index: usize) -> usize {
    let mut cursor = index;
    while cursor < bytes.len() && is_word(bytes[cursor]) {
        cursor += 1;
    }
    cursor
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_keyword(word: &str) -> bool {
    KEYWORDS.binary_search(&word).is_ok()
}

/// The shared keyword set, sorted for [`is_keyword`]'s binary search.
///
/// One union across languages rather than a table per language: the cost of colouring `match` in
/// a shell script is a wrong-but-harmless magenta word, and the cost of a per-language table is
/// a grammar registry this crate has no business owning.
pub(super) const KEYWORDS: &[&str] = &[
    "abstract",
    "and",
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "crate",
    "def",
    "default",
    "defer",
    "del",
    "do",
    "dyn",
    "elif",
    "else",
    "end",
    "enum",
    "export",
    "extends",
    "extern",
    "false",
    "final",
    "finally",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "go",
    "if",
    "impl",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "is",
    "let",
    "loop",
    "match",
    "mod",
    "module",
    "move",
    "mut",
    "namespace",
    "new",
    "nil",
    "none",
    "not",
    "null",
    "or",
    "package",
    "pass",
    "private",
    "protected",
    "pub",
    "public",
    "raise",
    "record",
    "ref",
    "return",
    "select",
    "self",
    "static",
    "struct",
    "super",
    "switch",
    "then",
    "this",
    "throw",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "unsafe",
    "use",
    "var",
    "void",
    "where",
    "while",
    "with",
    "yield",
];

// ---------------------------------------------------------------------------------------------
// The highlight cache
// ---------------------------------------------------------------------------------------------

/// How many lexed blocks the cache keeps. A transcript's visible fences, plus room for the tail
/// the reader just scrolled past.
const CACHE_CAPACITY: usize = 16;

/// A small LRU of lexed fences, keyed by content.
///
/// A streaming transcript re-parses the same prose on every delta, so a settled fence would be
/// re-lexed once per token without this. The rule that matters is the one it enforces at the
/// boundary: **a partial fence is neither read from nor written to the cache.** An unterminated
/// fence's text changes with every chunk, so caching it would fill the cache with garbage keyed
/// on text that no longer exists — and, worse, a lookup could hand a *closed* fence the colours
/// of its own prefix. It must never poison it.
#[derive(Debug, Default)]
pub struct HighlightCache {
    entries: RefCell<VecDeque<(u64, CodeHighlights)>>,
}

impl HighlightCache {
    /// An empty cache. An owner holds one per transcript.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many blocks are cached. Mostly for the tests that pin the poisoning rule.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget everything, for a thread switch.
    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
    }

    /// The colours of one **closed** fence, lexing it only on a miss.
    pub(super) fn highlight(&self, lang: Option<&str>, text: &str) -> CodeHighlights {
        let key = key(lang, text);
        {
            let mut entries = self.entries.borrow_mut();
            if let Some(position) = entries.iter().position(|(cached, _)| *cached == key)
                && let Some(entry) = entries.remove(position)
            {
                entries.push_front(entry.clone());
                return entry.1;
            }
        }
        let highlights = CodeHighlights::new(lang, text);
        let mut entries = self.entries.borrow_mut();
        if entries.len() == CACHE_CAPACITY {
            entries.pop_back();
        }
        entries.push_front((key, highlights.clone()));
        highlights
    }
}

/// The cache key: the fence's language and its exact text.
fn key(lang: Option<&str>, text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    lang.hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}
