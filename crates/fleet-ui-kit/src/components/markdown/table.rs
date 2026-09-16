//! The GFM pipe-table opener and row scanner.

use gpui::TextAlign;

use super::MarkdownBlock;
use super::parser::parse_inlines;

const MAX_OPENER_INDENT: usize = 3;

/// Whether the last line could still grow into the delimiter below `header`.
///
/// This reserves the ambiguous `header | cell\n---` prefix for the open paragraph. Once a
/// newline fixes that `---` as its own line, the ordinary thematic-break rule can decide it.
pub(super) fn is_opening_prefix(header: &str, delimiter: &str) -> bool {
    let (header_indent, header) = split_indent(header);
    let (delimiter_indent, delimiter) = split_indent(delimiter);
    header_indent <= MAX_OPENER_INDENT
        && delimiter_indent <= MAX_OPENER_INDENT
        && cells(header).is_some()
        && delimiter.bytes().any(|byte| byte == b'-')
        && delimiter
            .bytes()
            .all(|byte| matches!(byte, b'-' | b':' | b'|' | b' ' | b'\t'))
}

/// Collect a GFM table. The header is not a table until the complete delimiter row arrives;
/// before then the ordinary paragraph path owns it, so only the document's last block changes.
pub(super) fn parse(lines: &[&str], start: usize) -> Option<(MarkdownBlock, usize)> {
    let (_, header_line) = split_indent(lines.get(start)?);
    let (delimiter_indent, delimiter_line) = split_indent(lines.get(start + 1)?);
    if delimiter_indent > MAX_OPENER_INDENT {
        return None;
    }
    let header_cells = cells(header_line)?;
    let delimiter_cells = cells(delimiter_line)?;
    if header_cells.len() != delimiter_cells.len() {
        return None;
    }
    let alignments: Vec<TextAlign> = delimiter_cells
        .iter()
        .map(|cell| alignment(cell))
        .collect::<Option<_>>()?;
    let columns = header_cells.len();
    let header = header_cells
        .iter()
        .map(|cell| parse_inlines(cell))
        .collect();

    let mut rows = Vec::new();
    let mut index = start + 2;
    while index < lines.len() && !lines[index].trim().is_empty() {
        let (indent, rest) = split_indent(lines[index]);
        if indent > MAX_OPENER_INDENT {
            break;
        }
        let mut row = match cells(rest) {
            Some(row) => row,
            // The last physical line is still growing. Keep it inside the table as a
            // single-cell row until either a pipe arrives or a newline decides that it starts
            // the next block. This is the body-row counterpart of the two-line opener rule.
            None if index + 1 == lines.len() => vec![rest.trim().to_owned()],
            None => break,
        };
        row.resize(columns, String::new());
        row.truncate(columns);
        rows.push(row.iter().map(|cell| parse_inlines(cell)).collect());
        index += 1;
    }
    Some((
        MarkdownBlock::Table {
            header,
            alignments,
            rows,
        },
        index,
    ))
}

/// Split a row at unescaped pipes, preserving escapes for the inline parser.
fn cells(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    let mut separators = 0usize;
    for character in line.chars() {
        if escaped {
            cell.push('\\');
            cell.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '|' {
            cells.push(std::mem::take(&mut cell));
            separators += 1;
        } else {
            cell.push(character);
        }
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(cell);
    if separators == 0 {
        return None;
    }
    if line.starts_with('|') {
        cells.remove(0);
    }
    if ends_with_unescaped_pipe(line) {
        cells.pop();
    }
    (!cells.is_empty()).then(|| {
        cells
            .into_iter()
            .map(|cell| cell.trim().to_owned())
            .collect()
    })
}

fn ends_with_unescaped_pipe(line: &str) -> bool {
    let bytes = line.as_bytes();
    if bytes.last() != Some(&b'|') {
        return false;
    }
    let slashes = bytes[..bytes.len() - 1]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count();
    slashes % 2 == 0
}

/// Decode one GFM delimiter cell. Three dashes keep a partial `|-|` line as prose while it is
/// still streaming and match the grammar real Claude and Codex answers produce.
fn alignment(cell: &str) -> Option<TextAlign> {
    let cell = cell.trim();
    let left = cell.starts_with(':');
    let right = cell.ends_with(':');
    let dashes = cell.trim_matches(':');
    if dashes.len() < 3 || !dashes.bytes().all(|byte| byte == b'-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => TextAlign::Center,
        (false, true) => TextAlign::Right,
        _ => TextAlign::Left,
    })
}

fn split_indent(line: &str) -> (usize, &str) {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    (indent, &line[indent..])
}
