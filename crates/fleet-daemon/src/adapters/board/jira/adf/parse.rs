use super::*;

/// The deepest heading ADF has, and the deepest [`super::render::render_block`] writes hashes for.
///
/// `fleet_ui_kit::parse_markdown` draws only three levels, so `#### x` shows as plain text in
/// Fleet — but it has to *push* as the h4 it came from, or one edit to a description with an h4
/// in it writes the literal text `#### x` into Jira.
const MAX_MARKDOWN_HEADING: usize = 6;

/// Builds an ADF document from markdown.
///
/// Covers what `fleet_ui_kit::parse_markdown` covers — headings, paragraphs, lists, fenced and
/// inline code, bold, links — and falls back to plain paragraphs for anything else, because a
/// body Jira rejects loses the user's comment.
#[must_use]
pub fn markdown_to_adf(text: &str) -> Value {
    json!({ "version": 1, "type": "doc", "content": markdown_blocks(text) })
}

/// Trims the punctuation a sentence leaves after a bare URL, keeping what the URL itself owns.
///
/// A closing parenthesis is only the sentence's when the URL never opened one:
/// `https://wiki/x_(y)` ends in a parenthesis of its own, and cutting it writes a link to a page
/// that does not exist, with a stray `)` beside it.
fn trim_url_end(candidate: &str) -> &str {
    let mut url = candidate;
    loop {
        let mut trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if trimmed.ends_with(')') && trimmed.matches('(').count() < trimmed.matches(')').count() {
            trimmed = &trimmed[..trimmed.len() - 1];
        }
        if trimmed.len() == url.len() {
            return url;
        }
        url = trimmed;
    }
}

/// How deep a `> ` quote may nest before its body is scanned as prose.
///
/// The scanner recurses per quote level and the input is a remote service's document; a
/// pathological one must cost a wrapper, not the process.
const MAX_QUOTE_DEPTH: usize = 8;

/// The block scanner, line for line the same shape as `fleet_ui_kit::parse_markdown`, plus the
/// blocks [`adf_to_markdown`] writes that `parse_markdown` does not read.
fn markdown_blocks(source: &str) -> Vec<Value> {
    markdown_blocks_at(source, 0)
}

fn markdown_blocks_at(source: &str, depth: usize) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut list: Vec<ListLine> = Vec::new();
    let mut lines = source.lines().peekable();

    while let Some(raw) = lines.next() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        // The fence is however many backticks opened it, so a block whose body contains a fence
        // of its own is closed by the longer one that wrapped it — and only by that one.
        let opening = trimmed
            .chars()
            .take_while(|character| *character == '`')
            .count();
        if opening >= 3 {
            close_paragraph(&mut paragraph, &mut blocks);
            close_list(&mut list, &mut blocks);
            let language = trimmed[opening..].trim().to_owned();
            let mut code: Vec<&str> = Vec::new();
            for raw in lines.by_ref() {
                let candidate = raw.trim_start();
                let run = candidate
                    .chars()
                    .take_while(|character| *character == '`')
                    .count();
                if run >= opening && candidate[run..].trim().is_empty() {
                    break;
                }
                code.push(raw.trim_end());
            }
            blocks.push(code_block(&language, &code.join("\n")));
            continue;
        }

        if trimmed.is_empty() {
            close_paragraph(&mut paragraph, &mut blocks);
            close_list(&mut list, &mut blocks);
            continue;
        }

        // `---` is what `render_block` writes an ADF `rule` as; left to the paragraph scanner
        // it went back to Jira as a paragraph containing three hyphens.
        if markdown_rule(trimmed) {
            close_paragraph(&mut paragraph, &mut blocks);
            close_list(&mut list, &mut blocks);
            blocks.push(json!({ "type": "rule" }));
            continue;
        }

        // A `> ` run is what both `blockquote` and `panel` render as. Its body is scanned on
        // its own so a quoted list stays a list; a body ADF will not accept inside a quote is
        // emitted unquoted rather than wrapped into a document Jira would reject outright.
        if depth < MAX_QUOTE_DEPTH && markdown_quote(trimmed).is_some() {
            close_paragraph(&mut paragraph, &mut blocks);
            close_list(&mut list, &mut blocks);
            let mut quoted: Vec<String> =
                vec![markdown_quote(trimmed).unwrap_or_default().to_owned()];
            while let Some(next) = lines.peek() {
                let Some(body) = markdown_quote(next.trim_start()) else {
                    break;
                };
                quoted.push(body.to_owned());
                lines.next();
            }
            let inner = markdown_blocks_at(&quoted.join("\n"), depth + 1);
            if inner.iter().all(|node| QUOTABLE.contains(&node_kind(node))) {
                blocks.push(json!({ "type": "blockquote", "content": inner }));
            } else {
                blocks.extend(inner);
            }
            continue;
        }

        // A pipe table, recognised only by the `| --- |` rule `render_table` always writes
        // under the header: without it the whole table collapsed into one paragraph of pipes
        // the first time anyone edited the description.
        if trimmed.starts_with('|')
            && lines
                .peek()
                .is_some_and(|next| is_table_separator(next.trim_start()))
        {
            close_paragraph(&mut paragraph, &mut blocks);
            close_list(&mut list, &mut blocks);
            let header = table_cells(trimmed);
            lines.next();
            let mut rows = vec![table_row(&header, true)];
            while let Some(next) = lines.peek() {
                let next = next.trim_start();
                if !next.starts_with('|') {
                    break;
                }
                rows.push(table_row(&table_cells(next), false));
                lines.next();
            }
            blocks.push(json!({
                "type": "table",
                "attrs": { "isNumberColumnEnabled": false, "layout": "default" },
                "content": rows,
            }));
            continue;
        }

        if let Some((level, text)) = markdown_heading(trimmed) {
            close_paragraph(&mut paragraph, &mut blocks);
            close_list(&mut list, &mut blocks);
            blocks.push(json!({
                "type": "heading",
                "attrs": { "level": level },
                "content": [{ "type": "text", "text": text }],
            }));
            continue;
        }

        // `line`, not `trimmed`: the indentation is the nesting, and `list_nodes` is what
        // turns the run of collected lines into the tree it describes.
        if let Some(item) = markdown_list_item(line) {
            close_paragraph(&mut paragraph, &mut blocks);
            list.push(item);
            continue;
        }

        close_list(&mut list, &mut blocks);
        paragraph.push(trimmed);
    }

    close_paragraph(&mut paragraph, &mut blocks);
    close_list(&mut list, &mut blocks);
    blocks
}

/// Closes the open paragraph. Its lines join with a space: a soft line break inside a paragraph
/// is a wrap, not a break — the blank line is what breaks.
fn close_paragraph(paragraph: &mut Vec<&str>, blocks: &mut Vec<Value>) {
    if paragraph.is_empty() {
        return;
    }
    let text = paragraph.join(" ");
    paragraph.clear();
    blocks.push(json!({ "type": "paragraph", "content": markdown_inline(&text) }));
}

/// Closes the open list.
fn close_list(list: &mut Vec<ListLine>, blocks: &mut Vec<Value>) {
    if list.is_empty() {
        return;
    }
    blocks.extend(list_nodes(list));
    list.clear();
}

/// One row of a pipe table as ADF, header cells or body cells.
fn table_row(cells: &[String], header: bool) -> Value {
    let kind = if header { "tableHeader" } else { "tableCell" };
    let cells: Vec<Value> = cells
        .iter()
        .map(|cell| {
            json!({
                "type": kind,
                "attrs": {},
                "content": [{ "type": "paragraph", "content": markdown_inline(cell) }],
            })
        })
        .collect();
    json!({ "type": "tableRow", "content": cells })
}

/// A fenced code block, with its language when the fence carried one.
fn code_block(language: &str, code: &str) -> Value {
    let content = if code.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "type": "text", "text": code })]
    };
    let mut node = json!({ "type": "codeBlock", "content": content });
    if !language.is_empty() {
        node["attrs"] = json!({ "language": language });
    }
    node
}

/// `### text` → `(3, "text")`. Deeper than [`MAX_MARKDOWN_HEADING`] is a paragraph.
fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > MAX_MARKDOWN_HEADING {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?.trim();
    if rest.is_empty() {
        return None;
    }
    Some((hashes, rest))
}

/// One line of a list, with the indentation that decides how deep it sits.
struct ListLine {
    /// Leading spaces before the marker; `render_list` indents a nested list by the width of
    /// its parent's marker, so this is the only thing that says where an item belongs.
    indent: usize,
    ordered: bool,
    number: u64,
    text: String,
}

/// `- a` → a level-0 bullet; `   1. a` → a level-3 ordered item starting at 1.
///
/// The marker is stripped, because ADF numbers list items itself. The indent is measured
/// rather than refused: `render_list` writes nested items indented by the parent marker's
/// width, and a scanner that treated `  - b` as prose turned every nested bullet a pull
/// brought back into a literal `- b` paragraph the moment the description was edited.
fn markdown_list_item(line: &str) -> Option<ListLine> {
    let indent = line.len() - line.trim_start().len();
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(ListLine {
            indent,
            ordered: false,
            number: 0,
            text: rest.trim().to_owned(),
        });
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = line[digits..].strip_prefix(". ")?;
    Some(ListLine {
        indent,
        ordered: true,
        number: line[..digits].parse::<u64>().unwrap_or(1),
        text: rest.trim().to_owned(),
    })
}

/// Whether a line is a `---` horizontal rule rather than prose.
fn markdown_rule(line: &str) -> bool {
    line.len() >= 3 && line.chars().all(|character| character == '-')
}

/// The body of one `> ` line, or `None` when the line is not quoted.
fn markdown_quote(line: &str) -> Option<&str> {
    line.strip_prefix("> ")
        .or_else(|| (line.trim_end() == ">").then_some(""))
}

/// The node types ADF allows inside a `blockquote`.
///
/// A document Jira refuses costs the user the whole edit, so a quote whose body does not fit
/// the schema is emitted unquoted rather than wrapped into something invalid.
const QUOTABLE: &[&str] = &["paragraph", "bulletList", "orderedList", "codeBlock"];

/// Splits one pipe-table row into its cells, honouring the `\|` that [`cell_text`] writes.
fn table_cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for character in inner.chars() {
        match character {
            '\\' if !escaped => escaped = true,
            '|' if !escaped => {
                cells.push(cell.trim().to_owned());
                cell = String::new();
            }
            _ => {
                if escaped && character != '|' {
                    cell.push('\\');
                }
                escaped = false;
                cell.push(character);
            }
        }
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(cell.trim().to_owned());
    cells
}

/// Whether a line is the `| --- | --- |` rule that separates a pipe table's header.
fn is_table_separator(line: &str) -> bool {
    let line = line.trim();
    if !line.starts_with('|') {
        return false;
    }
    let cells = table_cells(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim().trim_start_matches(':').trim_end_matches(':');
            cell.len() >= 3 && cell.chars().all(|character| character == '-')
        })
}

/// Builds the ADF list nodes one run of collected [`ListLine`]s describes.
///
/// Lines deeper than the one above them become that item's own nested list, which is what
/// `render_list` wrote them as. A marker that changes at the same depth starts a second list,
/// because merging a `-` line into a `1.` list would silently renumber the author's items.
fn list_nodes(lines: &[ListLine]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let ordered = lines[index].ordered;
        let start = lines[index].number;
        let base = lines[index].indent;
        let mut items = Vec::new();
        while index < lines.len() && lines[index].indent <= base && lines[index].ordered == ordered
        {
            let text = lines[index].text.clone();
            index += 1;
            let children = index;
            while index < lines.len() && lines[index].indent > base {
                index += 1;
            }
            let mut content = vec![json!({
                "type": "paragraph",
                "content": markdown_inline(&text),
            })];
            content.extend(list_nodes(&lines[children..index]));
            items.push(json!({ "type": "listItem", "content": content }));
        }
        let mut node = json!({
            "type": if ordered { "orderedList" } else { "bulletList" },
            "content": items,
        });
        // ADF numbers from 1 unless told otherwise; carrying the authored start keeps
        // `3. a` → `3. a` instead of quietly renumbering it.
        if ordered && start != 1 {
            node["attrs"] = json!({ "order": start });
        }
        out.push(node);
    }
    out
}

/// Splits one line into ADF inline nodes, mark for mark as `parse_inline` splits it into spans.
fn markdown_inline(text: &str) -> Vec<Value> {
    let mut nodes: Vec<Value> = Vec::new();
    let mut plain = String::new();
    let bytes = text.as_bytes();
    let mut index = 0;

    while index < text.len() {
        let rest = &text[index..];

        if rest.starts_with('`')
            && let Some(end) = rest[1..].find('`')
        {
            flush_plain(&mut plain, &mut nodes);
            push_node(
                &mut nodes,
                &rest[1..1 + end],
                vec![json!({ "type": "code" })],
            );
            index += end + 2;
            continue;
        }

        if rest.starts_with("**")
            && let Some(end) = rest[2..].find("**")
            && end > 0
        {
            flush_plain(&mut plain, &mut nodes);
            push_node(
                &mut nodes,
                &rest[2..2 + end],
                vec![json!({ "type": "strong" })],
            );
            index += end + 4;
            continue;
        }

        // `[label](href)` is what `marked_text` writes a link mark as, so not reading it back
        // broke rule 2 on the one construct that carries a user's own words: an edit to any
        // other line of a pulled description rewrote every labelled link in it into the literal
        // characters `[`, `](` and `)` around a link node whose text was the bare href.
        if rest.starts_with('[')
            && let Some((label, href, length)) = markdown_link(rest)
        {
            flush_plain(&mut plain, &mut nodes);
            let mark = json!({ "type": "link", "attrs": { "href": href } });
            // The label is scanned as inline markdown of its own — `[**bold**](url)` is what
            // `marked_text` writes a node carrying both marks as — and the link mark is added
            // to every span it produced. A label can hold no `[`, so this cannot recurse twice.
            for mut node in markdown_inline(label) {
                match node.get_mut("marks").and_then(Value::as_array_mut) {
                    Some(marks) => marks.push(mark.clone()),
                    None => node["marks"] = json!([mark.clone()]),
                }
                nodes.push(node);
            }
            index += length;
            continue;
        }

        // A URL only starts at a word boundary, so `shttp://x` stays text.
        let at_boundary =
            index == 0 || bytes[index - 1].is_ascii_whitespace() || bytes[index - 1] == b'(';
        if at_boundary && (rest.starts_with("http://") || rest.starts_with("https://")) {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = trim_url_end(&rest[..end]);
            let scheme = if rest.starts_with("https://") {
                "https://".len()
            } else {
                "http://".len()
            };
            if url.len() > scheme {
                flush_plain(&mut plain, &mut nodes);
                push_node(
                    &mut nodes,
                    url,
                    vec![json!({ "type": "link", "attrs": { "href": url } })],
                );
                index += url.len();
                continue;
            }
        }

        let Some(character) = rest.chars().next() else {
            break;
        };
        plain.push(character);
        index += character.len_utf8();
    }

    flush_plain(&mut plain, &mut nodes);
    nodes
}

/// Splits a leading `[label](href)` into its label, its href and the bytes it spans.
///
/// `None` for anything that is not one, so the brackets stay the literal text they were: a
/// label carrying a bracket of its own, a href carrying whitespace (markdown's `(url "title")`
/// form, which ADF has nowhere to put), and an empty half of either.
fn markdown_link(rest: &str) -> Option<(&str, &str, usize)> {
    let close = rest.find("](")?;
    let label = &rest[1..close];
    if label.is_empty() || label.contains(['[', ']', '\n']) {
        return None;
    }
    let after = &rest[close + 2..];
    let mut depth = 1_usize;
    let mut end = None;
    for (at, character) in after.char_indices() {
        match character {
            // A href may hold balanced parentheses of its own — `https://wiki/x_(y)` — and it
            // is the one that closes the link that ends it.
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(at);
                    break;
                }
            }
            character if character.is_whitespace() => return None,
            _ => {}
        }
    }
    let end = end?;
    let href = &after[..end];
    (!href.is_empty()).then(|| (label, href, close + 2 + end + 1))
}

/// Flushes the pending unmarked run.
fn flush_plain(plain: &mut String, nodes: &mut Vec<Value>) {
    if !plain.is_empty() {
        nodes.push(json!({ "type": "text", "text": std::mem::take(plain) }));
    }
}

/// Appends a marked text node, skipping the empty one ADF rejects (`` `` `` is invisible).
fn push_node(nodes: &mut Vec<Value>, text: &str, marks: Vec<Value>) {
    if text.is_empty() {
        return;
    }
    nodes.push(json!({ "type": "text", "text": text, "marks": marks }));
}
