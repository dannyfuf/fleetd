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

use std::ops::Range;

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

/// Colour one fenced block, as sorted, disjoint byte ranges over `text`.
///
/// The ranges are what `StyledText::with_default_highlights` wants; producing them out of order
/// or overlapping makes it panic, which is why the scanner only ever moves forward.
pub(super) fn highlight(lang: Option<&str>, text: &str) -> Vec<(Range<usize>, CodeToken)> {
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
