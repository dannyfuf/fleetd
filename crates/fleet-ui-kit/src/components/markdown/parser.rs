//! The block and inline scanner behind [`super::parse_markdown`].
//!
//! It is a hand-written CommonMark **subset**, sized for one job: provider transcript prose
//! that arrives one token at a time. Three properties matter more than coverage.
//!
//! 1. **Total.** Every `&str` maps to a document. There is no error path, no `unwrap` on a
//!    slice and no unbounded recursion: quotes and lists stop nesting at [`MAX_BLOCK_DEPTH`]
//!    and inline containers at [`MAX_INLINE_DEPTH`], after which the source is kept verbatim.
//! 2. **Safe on a prefix.** An unterminated fence closes at end of input, an unmatched `` ` ``,
//!    `*` or `[` stays literal. Nothing is ever dropped waiting for a delimiter that a slower
//!    model has not typed yet.
//! 3. **Stable under growth.** Every block is decided by its own opening line, so appending
//!    text can only extend or replace the **last** block. `setext` headings (`text` then
//!    `---`) are deliberately absent for exactly that reason: they would retroactively turn a
//!    finished paragraph into a heading two lines later, and the transcript would reflow under
//!    the reader. `---` is always a thematic break here.
//!
//! Line breaks are normalised into the inline text: a soft break becomes a space, a hard break
//! (two trailing spaces, or a trailing backslash) becomes a `\n` inside a
//! [`MarkdownInline::Text`]. The renderer is the only place that knows what a `\n` looks like.
//!
//! Out of scope, per `docs/NATIVE-AGENTS.md` §8: tables, images and indented code blocks. They
//! are kept as their own source text so nothing is silently lost.

use super::{MarkdownBlock, MarkdownDocument, MarkdownInline};

/// How deep quotes and lists may nest before the source is kept as literal paragraphs.
const MAX_BLOCK_DEPTH: usize = 6;

/// How deep emphasis and link labels may nest before the source is kept as literal text.
const MAX_INLINE_DEPTH: usize = 8;

/// The largest indent that still counts as "not indented" for a block opener.
const MAX_OPENER_INDENT: usize = 3;

/// Parse a whole document.
pub(super) fn parse(source: &str) -> MarkdownDocument {
    let lines: Vec<&str> = source
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    MarkdownDocument {
        blocks: parse_blocks(&lines, 0),
    }
}

/// Parse a run of already-dedented lines into blocks.
fn parse_blocks(lines: &[&str], depth: usize) -> Vec<MarkdownBlock> {
    if depth > MAX_BLOCK_DEPTH {
        return literal_paragraphs(lines);
    }

    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if is_blank(line) {
            index += 1;
            continue;
        }
        let (indent, rest) = split_indent(line);
        if indent > MAX_OPENER_INDENT {
            let (block, next) = paragraph(lines, index);
            blocks.push(block);
            index = next;
            continue;
        }

        if let Some(fence) = fence_open(rest) {
            let (block, next) = code_block(lines, index, indent, fence);
            blocks.push(block);
            index = next;
        } else if is_thematic_break(rest) {
            blocks.push(MarkdownBlock::Rule);
            index += 1;
        } else if let Some((level, text)) = atx_heading(rest) {
            blocks.push(MarkdownBlock::Heading {
                level,
                inlines: parse_inlines(text),
            });
            index += 1;
        } else {
            // A container that consumed nothing would spin here forever, so the paragraph
            // fallback is what actually guarantees the parser terminates on every input.
            let container = if rest.starts_with('>') {
                Some(quote(lines, index, depth))
            } else {
                list_marker(rest).map(|marker| list(lines, index, marker.ordered, depth))
            };
            let (block, next) = match container {
                Some((block, next)) if next > index => (block, next),
                _ => paragraph(lines, index),
            };
            blocks.push(block);
            index = next;
        }
    }
    blocks
}

/// Keep an over-nested region as plain paragraphs rather than recursing further.
fn literal_paragraphs(lines: &[&str]) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if is_blank(lines[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < lines.len() && !is_blank(lines[index]) {
            index += 1;
        }
        blocks.push(MarkdownBlock::Paragraph(vec![MarkdownInline::Text(
            lines[start..index].join("\n"),
        )]));
    }
    blocks
}

// ---------------------------------------------------------------------------------------------
// Block openers
// ---------------------------------------------------------------------------------------------

/// A fence opener: its character, its length and its info string.
#[derive(Clone, Copy)]
struct Fence<'a> {
    marker: u8,
    len: usize,
    info: &'a str,
}

/// Whether `rest` (an already dedented line) opens a fenced code block.
fn fence_open(rest: &str) -> Option<Fence<'_>> {
    let bytes = rest.as_bytes();
    let marker = *bytes.first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = bytes.iter().take_while(|byte| **byte == marker).count();
    if len < 3 {
        return None;
    }
    let info = rest[len..].trim();
    // A backtick fence's info string may not contain a backtick, or ``a ` b`` would open one.
    if marker == b'`' && info.contains('`') {
        return None;
    }
    Some(Fence { marker, len, info })
}

/// Whether `rest` closes a fence opened by `fence`.
fn fence_close(rest: &str, fence: Fence<'_>) -> bool {
    let bytes = rest.as_bytes();
    let len = bytes
        .iter()
        .take_while(|byte| **byte == fence.marker)
        .count();
    len >= fence.len && rest[len..].trim().is_empty()
}

/// A `***`, `---` or `___` thematic break.
fn is_thematic_break(rest: &str) -> bool {
    let mut marker = None;
    let mut count = 0;
    for byte in rest.bytes() {
        match byte {
            b'*' | b'-' | b'_' => {
                if *marker.get_or_insert(byte) != byte {
                    return false;
                }
                count += 1;
            }
            b' ' | b'\t' => {}
            _ => return false,
        }
    }
    count >= 3
}

/// An ATX heading: its level and its inline source.
fn atx_heading(rest: &str) -> Option<(u8, &str)> {
    let level = rest.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let after = &rest[level..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    // A trailing run of `#` closes the heading only when whitespace separates it from the text,
    // so `# c#` keeps its hash and `# c #` does not.
    let text = after.trim();
    let without_hashes = text.trim_end_matches('#');
    let text = if without_hashes.len() == text.len() {
        text
    } else if without_hashes.is_empty() {
        ""
    } else if without_hashes.ends_with([' ', '\t']) {
        without_hashes.trim_end()
    } else {
        text
    };
    Some((level as u8, text))
}

/// A list marker: whether it is ordered and how many bytes it occupies including its space.
#[derive(Clone, Copy)]
struct ListMarker {
    ordered: bool,
    len: usize,
}

/// Whether `rest` opens a list item.
fn list_marker(rest: &str) -> Option<ListMarker> {
    let bytes = rest.as_bytes();
    let first = *bytes.first()?;
    if matches!(first, b'-' | b'*' | b'+') {
        return match bytes.get(1) {
            None => Some(ListMarker {
                ordered: false,
                len: 1,
            }),
            Some(b' ' | b'\t') => Some(ListMarker {
                ordered: false,
                len: 2,
            }),
            Some(_) => None,
        };
    }
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || digits > 9 {
        return None;
    }
    if !matches!(bytes.get(digits), Some(b'.' | b')')) {
        return None;
    }
    match bytes.get(digits + 1) {
        None => Some(ListMarker {
            ordered: true,
            len: digits + 1,
        }),
        Some(b' ' | b'\t') => Some(ListMarker {
            ordered: true,
            len: digits + 2,
        }),
        Some(_) => None,
    }
}

/// Whether a dedented line starts a block that a paragraph or list item cannot swallow.
fn starts_block(rest: &str) -> bool {
    fence_open(rest).is_some()
        || is_thematic_break(rest)
        || atx_heading(rest).is_some()
        || rest.starts_with('>')
        || list_marker(rest).is_some()
}

// ---------------------------------------------------------------------------------------------
// Block bodies
// ---------------------------------------------------------------------------------------------

/// Collect a fenced code block. An unterminated fence ends at the last line, so a block that is
/// still streaming renders as code from its first line rather than as a wall of prose.
fn code_block(
    lines: &[&str],
    start: usize,
    indent: usize,
    fence: Fence<'_>,
) -> (MarkdownBlock, usize) {
    let lang = (!fence.info.is_empty()).then(|| {
        fence
            .info
            .split_whitespace()
            .next()
            .unwrap_or(fence.info)
            .to_owned()
    });
    let mut body = Vec::new();
    let mut index = start + 1;
    while index < lines.len() {
        let (line_indent, rest) = split_indent(lines[index]);
        if line_indent <= MAX_OPENER_INDENT && fence_close(rest, fence) {
            index += 1;
            break;
        }
        body.push(dedent(lines[index], indent));
        index += 1;
    }
    (
        MarkdownBlock::Code {
            lang,
            text: body.join("\n"),
        },
        index,
    )
}

/// Collect a block quote, including CommonMark's lazy continuation lines.
fn quote(lines: &[&str], start: usize, depth: usize) -> (MarkdownBlock, usize) {
    let mut inner: Vec<&str> = Vec::new();
    let mut index = start;
    while index < lines.len() {
        let line = lines[index];
        if is_blank(line) {
            break;
        }
        let (indent, rest) = split_indent(line);
        if indent <= MAX_OPENER_INDENT
            && let Some(after) = rest.strip_prefix('>')
        {
            inner.push(after.strip_prefix(' ').unwrap_or(after));
        } else if starts_block(rest) || inner.is_empty() {
            break;
        } else {
            inner.push(line);
        }
        index += 1;
    }
    (MarkdownBlock::Quote(parse_blocks(&inner, depth + 1)), index)
}

/// Collect one list and every item of the same kind that follows it.
fn list(lines: &[&str], start: usize, ordered: bool, depth: usize) -> (MarkdownBlock, usize) {
    let mut items: Vec<Vec<MarkdownBlock>> = Vec::new();
    let mut index = start;
    while index < lines.len() {
        let (indent, rest) = split_indent(lines[index]);
        if indent > MAX_OPENER_INDENT || is_thematic_break(rest) {
            break;
        }
        let Some(marker) = list_marker(rest) else {
            break;
        };
        if marker.ordered != ordered {
            break;
        }

        let content_indent = indent + marker.len;
        let mut item: Vec<&str> = vec![&rest[marker.len..]];
        index += 1;
        while index < lines.len() {
            if is_blank(lines[index]) {
                // A blank line only stays inside the item when indented content follows it.
                let next = lines[index + 1..]
                    .iter()
                    .position(|line| !is_blank(line))
                    .map(|offset| index + 1 + offset);
                match next {
                    Some(next) if split_indent(lines[next]).0 >= content_indent => {
                        item.push("");
                        index += 1;
                        continue;
                    }
                    _ => break,
                }
            }
            let (line_indent, line_rest) = split_indent(lines[index]);
            if line_indent >= content_indent {
                item.push(dedent(lines[index], content_indent));
            } else if starts_block(line_rest) {
                break;
            } else {
                // Lazy continuation of the item's paragraph.
                item.push(lines[index]);
            }
            index += 1;
        }
        items.push(parse_blocks(&item, depth + 1));
    }
    (MarkdownBlock::List { ordered, items }, index)
}

/// Collect a paragraph and its lazy continuation lines.
fn paragraph(lines: &[&str], start: usize) -> (MarkdownBlock, usize) {
    let mut text = String::new();
    let mut index = start;
    while index < lines.len() {
        let line = lines[index];
        if is_blank(line) {
            break;
        }
        if index > start && starts_block(split_indent(line).1) {
            break;
        }
        let (content, hard_break) = strip_break(line);
        if !text.is_empty() && !text.ends_with('\n') {
            text.push(' ');
        }
        text.push_str(content.trim_start());
        if hard_break {
            text.push('\n');
        }
        index += 1;
    }
    // A hard break on the last line of a paragraph has nothing to break, so drop it.
    while text.ends_with(['\n', ' ']) {
        text.pop();
    }
    (MarkdownBlock::Paragraph(parse_inlines(&text)), index)
}

/// Split a line into its content and whether it ends in a hard line break.
///
/// Two trailing spaces or an odd run of trailing backslashes are the two CommonMark spellings.
fn strip_break(line: &str) -> (&str, bool) {
    let trimmed = line.trim_end_matches([' ', '\t']);
    let spaces = line.len() - trimmed.len();
    let backslashes = trimmed
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count();
    if backslashes % 2 == 1 {
        (&trimmed[..trimmed.len() - 1], true)
    } else if spaces >= 2 {
        (trimmed, true)
    } else {
        (trimmed, false)
    }
}

// ---------------------------------------------------------------------------------------------
// Inline scanning
// ---------------------------------------------------------------------------------------------

/// Parse the inline content of one block.
pub(super) fn parse_inlines(text: &str) -> Vec<MarkdownInline> {
    parse_inlines_at(text, 0)
}

fn parse_inlines_at(text: &str, depth: usize) -> Vec<MarkdownInline> {
    if depth > MAX_INLINE_DEPTH {
        return literal(text);
    }
    let bytes = text.as_bytes();
    let mut nodes: Vec<MarkdownInline> = Vec::new();
    let mut pending = String::new();
    let mut index = 0;
    while index < bytes.len() {
        // Every delimiter is ASCII, so byte scanning never lands inside a code point.
        match bytes[index] {
            b'\\' if bytes.get(index + 1).is_some_and(u8::is_ascii_punctuation) => {
                pending.push(bytes[index + 1] as char);
                index += 2;
            }
            b'`' => {
                let run = run_len(bytes, index, b'`');
                match close_code(bytes, index + run, run) {
                    Some(end) => {
                        flush(&mut nodes, &mut pending);
                        nodes.push(MarkdownInline::Code(code_span(&text[index + run..end])));
                        index = end + run;
                    }
                    None => {
                        pending.push_str(&text[index..index + run]);
                        index += run;
                    }
                }
            }
            b'<' => match autolink(text, index) {
                Some((url, end)) => {
                    flush(&mut nodes, &mut pending);
                    nodes.push(MarkdownInline::Link {
                        label: vec![MarkdownInline::Text(url.to_owned())],
                        url: url.to_owned(),
                    });
                    index = end;
                }
                None => {
                    pending.push('<');
                    index += 1;
                }
            },
            // Images are out of scope: keep the source so nothing is lost.
            b'!' if bytes.get(index + 1) == Some(&b'[') => match link_at(text, index + 1) {
                Some((_, _, end)) => {
                    pending.push_str(&text[index..end]);
                    index = end;
                }
                None => {
                    pending.push('!');
                    index += 1;
                }
            },
            b'[' => match link_at(text, index) {
                Some((label, url, end)) => {
                    flush(&mut nodes, &mut pending);
                    nodes.push(MarkdownInline::Link {
                        label: parse_inlines_at(label, depth + 1),
                        url: url.to_owned(),
                    });
                    index = end;
                }
                None => {
                    pending.push('[');
                    index += 1;
                }
            },
            marker @ (b'*' | b'_') => {
                let run = run_len(bytes, index, marker);
                match emphasis(text, index, run, marker, depth) {
                    Some((node, extra, end)) => {
                        pending.push_str(&text[index..index + extra]);
                        flush(&mut nodes, &mut pending);
                        nodes.push(node);
                        index = end;
                    }
                    None => {
                        pending.push_str(&text[index..index + run]);
                        index += run;
                    }
                }
            }
            _ => match text[index..].chars().next() {
                Some(character) => {
                    pending.push(character);
                    index += character.len_utf8();
                }
                None => break,
            },
        }
    }
    flush(&mut nodes, &mut pending);
    nodes
}

/// One literal text node, or nothing for an empty string.
fn literal(text: &str) -> Vec<MarkdownInline> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![MarkdownInline::Text(text.to_owned())]
    }
}

fn flush(nodes: &mut Vec<MarkdownInline>, pending: &mut String) {
    if !pending.is_empty() {
        nodes.push(MarkdownInline::Text(std::mem::take(pending)));
    }
}

/// How long the run of `marker` starting at `index` is.
fn run_len(bytes: &[u8], index: usize, marker: u8) -> usize {
    bytes[index..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count()
}

/// Find the closing backtick run of exactly `run` backticks at or after `from`.
fn close_code(bytes: &[u8], from: usize, run: usize) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let len = run_len(bytes, index, b'`');
            if len == run {
                return Some(index);
            }
            index += len;
        } else {
            index += 1;
        }
    }
    None
}

/// CommonMark's code-span stripping: one space at each end when both are present.
fn code_span(text: &str) -> String {
    let stripped = text
        .strip_prefix(' ')
        .and_then(|rest| rest.strip_suffix(' '))
        .filter(|rest| !rest.trim().is_empty());
    stripped.unwrap_or(text).to_owned()
}

/// `<https://example.com>` and `<mailto:me@example.com>`.
fn autolink(text: &str, index: usize) -> Option<(&str, usize)> {
    let rest = &text[index + 1..];
    let end = rest.find('>')?;
    let inner = &rest[..end];
    let colon = inner.find(':')?;
    let (scheme, _) = inner.split_at(colon);
    let valid_scheme = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    if !valid_scheme || inner.chars().any(char::is_whitespace) {
        return None;
    }
    Some((inner, index + 1 + end + 1))
}

/// `[label](url "title")` starting at the `[`, returning label, url and the byte after the `)`.
fn link_at(text: &str, index: usize) -> Option<(&str, &str, usize)> {
    let bytes = text.as_bytes();
    let mut cursor = index + 1;
    let mut depth = 1usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    if depth != 0 || cursor >= bytes.len() || bytes.get(cursor + 1) != Some(&b'(') {
        return None;
    }
    let label = &text[index + 1..cursor];
    let dest_start = cursor + 2;
    let mut dest_end = dest_start;
    let mut parens = 1usize;
    while dest_end < bytes.len() {
        match bytes[dest_end] {
            b'\\' => dest_end += 1,
            b'(' => parens += 1,
            b')' => {
                parens -= 1;
                if parens == 0 {
                    break;
                }
            }
            b'\n' => return None,
            _ => {}
        }
        dest_end += 1;
    }
    if parens != 0 || dest_end >= bytes.len() {
        return None;
    }
    let url = link_destination(&text[dest_start..dest_end]);
    Some((label, url, dest_end + 1))
}

/// Strip the optional `<…>` wrapper and the optional title from a link destination.
fn link_destination(raw: &str) -> &str {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('<') {
        return inner.split('>').next().unwrap_or(inner);
    }
    raw.split_whitespace().next().unwrap_or("")
}

/// Match an emphasis run, returning the node, how many opener bytes stay literal, and the end.
///
/// `use = min(open, close)` capped at three: one delimiter is emphasis, two are strong and
/// three are emphasis around strong. Whatever the opener has left over stays literal text, which
/// is what makes `**bold*` render as `*` followed by `bold` in italics.
fn emphasis(
    text: &str,
    index: usize,
    run: usize,
    marker: u8,
    depth: usize,
) -> Option<(MarkdownInline, usize, usize)> {
    let bytes = text.as_bytes();
    if !can_open(bytes, index, run, marker) {
        return None;
    }
    let mut cursor = index + run;
    while cursor < bytes.len() {
        if bytes[cursor] != marker {
            cursor += 1;
            continue;
        }
        let close = run_len(bytes, cursor, marker);
        if !can_close(bytes, cursor, close, marker) {
            cursor += close;
            continue;
        }
        let taken = run.min(close).min(3);
        let extra = run - taken;
        let content = &text[index + run..cursor];
        let inner = parse_inlines_at(content, depth + 1);
        let node = match taken {
            1 => MarkdownInline::Emphasis(inner),
            2 => MarkdownInline::Strong(inner),
            _ => MarkdownInline::Emphasis(vec![MarkdownInline::Strong(inner)]),
        };
        return Some((node, extra, cursor + taken));
    }
    None
}

/// A run can open when it is followed by something other than whitespace.
fn can_open(bytes: &[u8], index: usize, run: usize, marker: u8) -> bool {
    let after = bytes.get(index + run);
    let opens = after.is_some_and(|byte| !byte.is_ascii_whitespace());
    // Intra-word `_` is a name, not emphasis: `snake_case_name` must survive.
    opens && (marker != b'_' || !index.checked_sub(1).is_some_and(|i| is_word(bytes[i])))
}

/// A run can close when it is preceded by something other than whitespace.
fn can_close(bytes: &[u8], index: usize, run: usize, marker: u8) -> bool {
    let before = index.checked_sub(1).map(|i| bytes[i]);
    let closes = before.is_some_and(|byte| !byte.is_ascii_whitespace());
    closes && (marker != b'_' || !bytes.get(index + run).copied().is_some_and(is_word))
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

// ---------------------------------------------------------------------------------------------
// Line helpers
// ---------------------------------------------------------------------------------------------

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Split a line into its indent width and the rest. A tab counts as one column: the transcript
/// never renders indented code, so the only thing indent width decides is nesting.
fn split_indent(line: &str) -> (usize, &str) {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    (indent, &line[indent..])
}

/// Drop up to `columns` leading spaces or tabs.
fn dedent(line: &str, columns: usize) -> &str {
    let taken = line
        .bytes()
        .take(columns)
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    &line[taken..]
}
