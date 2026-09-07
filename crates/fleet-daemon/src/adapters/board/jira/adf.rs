//! Atlassian Document Format ⇄ markdown, pure and side-effect free.
//!
//! Jira descriptions and comment bodies are ADF JSON, and every local surface — the card
//! detail, the CLI, a diff in a conflict — speaks markdown. The node coverage the conversion
//! owes is listed in `docs/BOARD-JIRA.md` §5.
//!
//! Three rules shape the whole module:
//!
//! 1. **Nothing panics.** The input is JSON a remote service produced; a missing field, a
//!    string where an array was promised or a `null` where a node was must cost that node, not
//!    the description. Every accessor goes through [`node_kind`] / [`children`], which answer
//!    for any `Value` at all.
//! 2. **Markdown in reads back everything markdown out writes.** `markdown → adf → markdown`
//!    is the identity on everything `fleet_ui_kit::parse_markdown` draws — headings, paragraphs,
//!    `-`/`*`/`1.` lists, fenced code, inline code, `**bold**`, bare URLs — *and* on the
//!    constructs [`adf_to_markdown`] emits for richer ADF: `####`–`######`, nested list items,
//!    `---`, `> ` quotes and pipe tables. That second half is not decoration. A description
//!    pulled from a Jira issue with a table in it is markdown the user edits one word of and
//!    pushes back; a scanner that cannot read its own output rewrote that table into Jira as a
//!    single paragraph of pipes, the rule as the text `---`, and the panel as a paragraph
//!    beginning `> `. What the two directions cannot carry is *identity of node type* — a panel
//!    comes back a blockquote, an h6 stays an h6 — never the user's content.
//! 3. **Unknown nodes recurse.** A node type Atlassian ships tomorrow costs its wrapper, never
//!    its text.

use serde_json::{Value, json};

/// What an image, file or embedded media renders as: markdown has no attachment syntax, and a
/// silent drop reads as a truncated description.
const ATTACHMENT: &str = "[attachment]";

/// The deepest heading ADF has, and the deepest [`render_block`] writes hashes for.
///
/// `fleet_ui_kit::parse_markdown` draws only three levels, so `#### x` shows as plain text in
/// Fleet — but it has to *push* as the h4 it came from, or one edit to a description with an h4
/// in it writes the literal text `#### x` into Jira.
const MAX_MARKDOWN_HEADING: usize = 6;

// ---------------------------------------------------------------------------------------
// ADF → markdown
// ---------------------------------------------------------------------------------------

/// Renders one ADF document as markdown.
///
/// Unknown nodes recurse into their `content` and are otherwise dropped, so a node type
/// Atlassian adds tomorrow costs a paragraph, never the whole description.
#[must_use]
pub fn adf_to_markdown(doc: &Value) -> String {
    block_list(document_nodes(doc)).join("\n\n")
}

/// Flattens an ADF document to unformatted text, for values rendered in a single-line field.
#[must_use]
pub fn adf_plain_text(doc: &Value) -> String {
    let mut out = String::new();
    plain_node(doc, &mut out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The top-level nodes of `doc`.
///
/// A `doc` node (or any object handed over without a type) contributes its `content`; anything
/// else is treated as a single node, so callers may pass a bare paragraph or comment body.
fn document_nodes(doc: &Value) -> &[Value] {
    let kind = node_kind(doc);
    if kind.is_empty() || kind == "doc" {
        children(doc)
    } else {
        std::slice::from_ref(doc)
    }
}

/// The `type` of a node, or `""` for anything that is not a typed object.
fn node_kind(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or("")
}

/// The `content` of a node, or an empty slice when it has none (or a malformed one).
fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// One attribute of a node, looked up without assuming `attrs` exists.
fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("attrs").and_then(|attrs| attrs.get(name))
}

/// Renders a run of block nodes, one markdown chunk per block.
fn block_list(nodes: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for node in nodes {
        render_block(node, &mut out);
    }
    out
}

/// Appends `text` as a block unless it is blank — an empty ADF paragraph is a spacer, and the
/// `\n\n` join already spaces the blocks around it.
fn push_block(out: &mut Vec<String>, text: String) {
    if !text.trim().is_empty() {
        out.push(text);
    }
}

/// Renders one block node into `out` (a node may contribute zero or several chunks).
fn render_block(node: &Value, out: &mut Vec<String>) {
    match node_kind(node) {
        "paragraph" => push_block(out, inline(children(node))),
        "heading" => {
            let level = attr(node, "level")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            // Checked before formatting: `# ` is not blank, so `push_block` would keep the
            // lone hash of a heading whose text did not survive.
            let text = inline(children(node));
            if !text.trim().is_empty() {
                out.push(format!("{} {text}", "#".repeat(level)));
            }
        }
        "bulletList" => push_block(out, render_list(node, false)),
        "orderedList" => push_block(out, render_list(node, true)),
        "codeBlock" => out.push(render_code_block(node)),
        // A panel is a callout: markdown has no callout, and a blockquote is the one construct
        // that keeps the "this is set apart" reading without inventing syntax.
        "blockquote" | "panel" => push_block(out, quote(&block_list(children(node)))),
        "rule" => out.push("---".to_owned()),
        "table" => push_block(out, render_table(node)),
        "mediaSingle" | "media" | "mediaInline" => out.push(ATTACHMENT.to_owned()),
        kind if is_inline(kind) => push_block(out, inline(std::slice::from_ref(node))),
        _ => {
            for child in children(node) {
                render_block(child, out);
            }
        }
    }
}

/// Renders a `bulletList` / `orderedList`, including nested ones.
///
/// Continuation lines are indented by the marker's width, so a nested list under `1. ` lines up
/// under its text the way every markdown renderer expects.
fn render_list(node: &Value, ordered: bool) -> String {
    let mut number = attr(node, "order")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1);
    let mut lines: Vec<String> = Vec::new();

    for item in children(node) {
        let body = if node_kind(item) == "listItem" {
            block_list(children(item))
        } else {
            // A malformed list whose children are paragraphs rather than list items still
            // reads as a list; dropping them would lose the text.
            block_list(std::slice::from_ref(item))
        }
        // Blocks inside one item join with a single newline: a blank line between a bullet and
        // its nested list re-parses as two lists.
        .join("\n");

        let marker = if ordered {
            let marker = format!("{number}. ");
            number += 1;
            marker
        } else {
            "- ".to_owned()
        };
        let indent = " ".repeat(marker.len());
        let mut body_lines = body.lines();
        lines.push(
            format!("{marker}{}", body_lines.next().unwrap_or(""))
                .trim_end()
                .to_owned(),
        );
        for line in body_lines {
            if line.is_empty() {
                lines.push(String::new());
            } else {
                lines.push(format!("{indent}{line}"));
            }
        }
    }

    lines.join("\n")
}

/// Renders a `codeBlock`, keeping its language on the fence so the round-trip survives.
fn render_code_block(node: &Value) -> String {
    let language = attr(node, "language")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut code = String::new();
    raw_text(children(node), &mut code);
    // The fence must outrun the longest run of backticks in the body, or a code block that
    // contains one closes early: the tail re-parses as loose paragraphs, and editing that
    // description in Fleet writes the mangled version back to Jira.
    let longest = code
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2).saturating_add(1));
    format!("{fence}{language}\n{code}\n{fence}")
}

/// Prefixes every line of `blocks` with `> `.
fn quote(blocks: &[String]) -> String {
    blocks
        .join("\n\n")
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders a `table` as a pipe table, best effort: the first row is the header, short rows are
/// padded, and cell text is flattened to one line because a pipe table has no line breaks.
fn render_table(node: &Value) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in children(node) {
        if node_kind(row) == "tableRow" {
            rows.push(children(row).iter().map(cell_text).collect());
        }
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return String::new();
    }

    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut cells = row.clone();
        cells.resize(width, String::new());
        lines.push(format!("| {} |", cells.join(" | ")));
        if index == 0 {
            lines.push(format!("| {} |", vec!["---"; width].join(" | ")));
        }
    }
    lines.join("\n")
}

/// One table cell, flattened to a single line with its pipes escaped.
fn cell_text(cell: &Value) -> String {
    block_list(children(cell))
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

/// Whether a node type belongs inside a paragraph rather than beside one.
fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        "text" | "hardBreak" | "mention" | "emoji" | "inlineCard" | "embedCard"
    )
}

/// Renders a run of inline nodes.
fn inline(nodes: &[Value]) -> String {
    let mut out = String::new();
    for node in nodes {
        push_inline(node, &mut out);
    }
    out
}

/// Renders one inline node into `out`.
fn push_inline(node: &Value, out: &mut String) {
    match node_kind(node) {
        "text" => out.push_str(&marked_text(node)),
        "hardBreak" => out.push('\n'),
        "mention" => {
            let name = mention_name(node);
            if !name.is_empty() {
                out.push('@');
                out.push_str(&name);
            }
        }
        "emoji" => out.push_str(emoji_text(node)),
        "inlineCard" | "embedCard" => out.push_str(card_url(node)),
        "media" | "mediaSingle" | "mediaInline" => out.push_str(ATTACHMENT),
        _ => {
            for child in children(node) {
                push_inline(child, out);
            }
        }
    }
}

/// Renders a `text` node with its marks applied.
///
/// Surrounding whitespace is kept *outside* the marks: Jira happily stores `"bold "` as one
/// strong run, and `**bold **` is not bold in any markdown renderer.
fn marked_text(node: &Value) -> String {
    let text = node.get("text").and_then(Value::as_str).unwrap_or("");
    let core = text.trim();
    if core.is_empty() {
        return text.to_owned();
    }
    let leading = &text[..text.len() - text.trim_start().len()];
    let trailing = &text[text.trim_end().len()..];

    let marks = node
        .get("marks")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let has = |name: &str| marks.iter().any(|mark| node_kind(mark) == name);

    let mut rendered = core.to_owned();
    if has("code") {
        // Code is literal by definition: emphasis inside it would be text, not emphasis.
        rendered = format!("`{rendered}`");
    } else {
        if has("strike") {
            rendered = format!("~~{rendered}~~");
        }
        // Markdown has no underline. `_x_` is the closest convention and, unlike `<u>`, reads
        // as intended when it lands in a renderer that does not know the mark.
        if has("underline") {
            rendered = format!("_{rendered}_");
        }
        if has("em") {
            rendered = format!("*{rendered}*");
        }
        if has("strong") {
            rendered = format!("**{rendered}**");
        }
    }

    if let Some(href) = marks
        .iter()
        .find(|mark| node_kind(mark) == "link")
        .and_then(|mark| mark.get("attrs"))
        .and_then(|attrs| attrs.get("href"))
        .and_then(Value::as_str)
        .filter(|href| !href.is_empty())
        && rendered != href
    {
        rendered = format!("[{rendered}]({href})");
    }

    format!("{leading}{rendered}{trailing}")
}

/// The display name behind a `mention`, without the `@` Jira stores in `text`.
fn mention_name(node: &Value) -> String {
    let name = attr(node, "text")
        .and_then(Value::as_str)
        .or_else(|| attr(node, "displayName").and_then(Value::as_str))
        .unwrap_or("");
    name.trim().trim_start_matches('@').to_owned()
}

/// The `:shortname:` of an `emoji`, falling back to the literal glyph Jira stores beside it.
fn emoji_text(node: &Value) -> &str {
    attr(node, "shortName")
        .and_then(Value::as_str)
        .or_else(|| attr(node, "text").and_then(Value::as_str))
        .unwrap_or("")
}

/// The URL of an `inlineCard` / `embedCard`, whether stored inline or under `data`.
fn card_url(node: &Value) -> &str {
    attr(node, "url")
        .and_then(Value::as_str)
        .or_else(|| {
            attr(node, "data")
                .and_then(|data| data.get("url"))
                .and_then(Value::as_str)
        })
        .unwrap_or("")
}

/// Collects the unformatted text of a subtree (used for code blocks, where marks are noise).
fn raw_text(nodes: &[Value], out: &mut String) {
    for node in nodes {
        match node_kind(node) {
            "text" => out.push_str(node.get("text").and_then(Value::as_str).unwrap_or("")),
            "hardBreak" => out.push('\n'),
            _ => raw_text(children(node), out),
        }
    }
}

/// Flattens one node for [`adf_plain_text`]; block nodes emit a trailing space so their text
/// does not fuse with the next block's once the whitespace is collapsed.
fn plain_node(node: &Value, out: &mut String) {
    match node_kind(node) {
        "text" => out.push_str(node.get("text").and_then(Value::as_str).unwrap_or("")),
        "hardBreak" => out.push(' '),
        "mention" => {
            let name = mention_name(node);
            if !name.is_empty() {
                out.push('@');
                out.push_str(&name);
            }
        }
        "emoji" => out.push_str(emoji_text(node)),
        "inlineCard" | "embedCard" => out.push_str(card_url(node)),
        // The trailing space matters here: a block-level attachment sits between two blocks.
        "media" | "mediaSingle" | "mediaInline" => {
            out.push_str(ATTACHMENT);
            out.push(' ');
        }
        _ => {
            for child in children(node) {
                plain_node(child, out);
            }
            out.push(' ');
        }
    }
}

// ---------------------------------------------------------------------------------------
// markdown → ADF
// ---------------------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A description shaped like the ones the real backend pulls: headings, marked text, a
    /// mention, an emoji, a link, nested lists, code, a panel, a table, a rule and an image.
    const JIRA_DESCRIPTION: &str = r#"{
      "version": 1,
      "type": "doc",
      "content": [
        { "type": "heading", "attrs": { "level": 2 },
          "content": [{ "type": "text", "text": "Contexto" }] },
        { "type": "paragraph", "content": [
          { "type": "text", "text": "El " },
          { "type": "text", "text": "sync", "marks": [{ "type": "code" }] },
          { "type": "text", "text": " falla cuando el board tiene " },
          { "type": "text", "text": "más de 200", "marks": [{ "type": "strong" }] },
          { "type": "text", "text": " tarjetas. Reportado por " },
          { "type": "mention", "attrs": { "id": "557058:abc", "text": "@Danny Fuentes" } },
          { "type": "text", "text": " " },
          { "type": "emoji", "attrs": { "shortName": ":warning:", "text": "⚠️" } }
        ]},
        { "type": "paragraph", "content": [
          { "type": "text", "text": "Ver ", "marks": [] },
          { "type": "text", "text": "el runbook",
            "marks": [{ "type": "link", "attrs": { "href": "https://buk.dev/runbook" } }] },
          { "type": "text", "text": " y " },
          { "type": "inlineCard", "attrs": { "url": "https://buk.atlassian.net/browse/SP-1" } }
        ]},
        { "type": "heading", "attrs": { "level": 3 },
          "content": [{ "type": "text", "text": "Pasos" }] },
        { "type": "orderedList", "content": [
          { "type": "listItem", "content": [
            { "type": "paragraph", "content": [{ "type": "text", "text": "Abrir el board" }] } ] },
          { "type": "listItem", "content": [
            { "type": "paragraph", "content": [{ "type": "text", "text": "Sincronizar" }] },
            { "type": "bulletList", "content": [
              { "type": "listItem", "content": [
                { "type": "paragraph", "content": [
                  { "type": "text", "text": "con " },
                  { "type": "text", "text": "--full", "marks": [{ "type": "code" }] } ] } ] } ] } ] }
        ]},
        { "type": "codeBlock", "attrs": { "language": "bash" },
          "content": [{ "type": "text", "text": "fleet board sync --full\necho ok" }] },
        { "type": "panel", "attrs": { "panelType": "warning" }, "content": [
          { "type": "paragraph", "content": [
            { "type": "text", "text": "No correr en producción." } ] } ]},
        { "type": "rule" },
        { "type": "table", "attrs": { "isNumberColumnEnabled": false }, "content": [
          { "type": "tableRow", "content": [
            { "type": "tableHeader", "content": [
              { "type": "paragraph", "content": [{ "type": "text", "text": "Entorno" }] } ] },
            { "type": "tableHeader", "content": [
              { "type": "paragraph", "content": [{ "type": "text", "text": "Estado" }] } ] } ] },
          { "type": "tableRow", "content": [
            { "type": "tableCell", "content": [
              { "type": "paragraph", "content": [{ "type": "text", "text": "staging" }] } ] },
            { "type": "tableCell", "content": [
              { "type": "paragraph", "content": [
                { "type": "text", "text": "ok", "marks": [{ "type": "em" }] } ] } ] } ] }
        ]},
        { "type": "mediaSingle", "attrs": { "layout": "center" }, "content": [
          { "type": "media", "attrs": { "id": "9f", "type": "file", "collection": "c" } } ]},
        { "type": "paragraph", "content": [
          { "type": "text", "text": "línea uno" },
          { "type": "hardBreak" },
          { "type": "text", "text": "línea dos" }
        ]}
      ]
    }"#;

    fn doc(value: Value) -> Value {
        json!({ "version": 1, "type": "doc", "content": value })
    }

    fn paragraph(text: &str) -> Value {
        json!({ "type": "paragraph", "content": [{ "type": "text", "text": text }] })
    }

    fn fixture() -> Value {
        serde_json::from_str(JIRA_DESCRIPTION).expect("the fixture is valid JSON")
    }

    // -- ADF → markdown ------------------------------------------------------------------

    /// The daemon feeds this whatever `acli` printed; a description that is `null` (Jira's
    /// answer for "no description") must be the empty string, not a panic.
    #[test]
    fn a_missing_or_malformed_document_renders_as_nothing() {
        assert_eq!(adf_to_markdown(&Value::Null), "");
        assert_eq!(adf_to_markdown(&json!("just a string")), "");
        assert_eq!(adf_to_markdown(&json!(42)), "");
        assert_eq!(adf_to_markdown(&json!([1, 2, 3])), "");
        assert_eq!(adf_to_markdown(&json!({ "type": "doc" })), "");
        assert_eq!(
            adf_to_markdown(&json!({ "type": "doc", "content": "nonsense" })),
            ""
        );
        assert_eq!(adf_to_markdown(&doc(json!([]))), "");
    }

    /// Every field of every node can be the wrong type and the walk still terminates.
    #[test]
    fn nodes_with_wrong_typed_fields_never_panic() {
        let broken = doc(json!([
            { "type": 7 },
            { "type": "paragraph", "content": { "not": "an array" } },
            { "type": "heading", "attrs": "nope", "content": [{ "type": "text" }] },
            { "type": "heading", "attrs": { "level": "two" },
              "content": [{ "type": "text", "text": "still a heading" }] },
            { "type": "text", "text": 12, "marks": "no" },
            { "type": "codeBlock", "attrs": { "language": 3 } },
            { "type": "bulletList", "content": [{ "type": "listItem" }] },
            { "type": "table", "content": [{ "type": "tableRow" }] },
            { "type": "mention" },
            { "type": "emoji" },
            { "type": "inlineCard" },
            {}
        ]));
        // The heading with an unreadable level falls back to level 1, the empty code block to
        // a bare fence and the empty list item to a bare marker; nothing else survives.
        assert_eq!(
            adf_to_markdown(&broken),
            "# still a heading\n\n```\n\n```\n\n-"
        );
        assert_eq!(adf_plain_text(&broken), "still a heading");
    }

    #[test]
    fn paragraphs_join_with_a_blank_line_and_empty_ones_drop_out() {
        let value = doc(json!([
            paragraph("one"),
            { "type": "paragraph", "content": [] },
            paragraph("two"),
        ]));
        assert_eq!(adf_to_markdown(&value), "one\n\ntwo");
    }

    #[test]
    fn headings_render_one_hash_per_level() {
        for level in 1..=6u64 {
            let value = doc(json!([
                { "type": "heading", "attrs": { "level": level },
                  "content": [{ "type": "text", "text": "H" }] }
            ]));
            assert_eq!(
                adf_to_markdown(&value),
                format!("{} H", "#".repeat(level as usize))
            );
        }
    }

    /// Jira only writes 1–6, but a level of 0 or 99 must not produce `H` with no hashes or a
    /// hundred-hash line.
    #[test]
    fn an_out_of_range_heading_level_clamps() {
        let low = doc(json!([
            { "type": "heading", "attrs": { "level": 0 },
              "content": [{ "type": "text", "text": "H" }] }
        ]));
        let high = doc(json!([
            { "type": "heading", "attrs": { "level": 99 },
              "content": [{ "type": "text", "text": "H" }] }
        ]));
        assert_eq!(adf_to_markdown(&low), "# H");
        assert_eq!(adf_to_markdown(&high), "###### H");
    }

    #[test]
    fn bullet_and_ordered_lists_render_their_markers() {
        let value = doc(json!([
            { "type": "bulletList", "content": [
                { "type": "listItem", "content": [paragraph("a")] },
                { "type": "listItem", "content": [paragraph("b")] } ]},
            { "type": "orderedList", "content": [
                { "type": "listItem", "content": [paragraph("x")] },
                { "type": "listItem", "content": [paragraph("y")] } ]}
        ]));
        assert_eq!(adf_to_markdown(&value), "- a\n- b\n\n1. x\n2. y");
    }

    /// `attrs.order` is how ADF says "this list starts at 3"; ignoring it renumbers the author.
    #[test]
    fn an_ordered_list_starts_at_its_order_attribute() {
        let value = doc(json!([
            { "type": "orderedList", "attrs": { "order": 3 }, "content": [
                { "type": "listItem", "content": [paragraph("a")] },
                { "type": "listItem", "content": [paragraph("b")] } ]}
        ]));
        assert_eq!(adf_to_markdown(&value), "3. a\n4. b");
    }

    #[test]
    fn nested_lists_indent_under_their_parent_item() {
        let value = doc(json!([
            { "type": "orderedList", "content": [
                { "type": "listItem", "content": [
                    paragraph("outer"),
                    { "type": "bulletList", "content": [
                        { "type": "listItem", "content": [
                            paragraph("inner"),
                            { "type": "bulletList", "content": [
                                { "type": "listItem", "content": [paragraph("deep")] } ]} ]} ]} ]} ]}
        ]));
        assert_eq!(adf_to_markdown(&value), "1. outer\n   - inner\n     - deep");
    }

    #[test]
    fn a_code_block_keeps_its_language_and_its_lines() {
        let with = doc(json!([
            { "type": "codeBlock", "attrs": { "language": "rust" },
              "content": [{ "type": "text", "text": "fn main() {}\nlet x = 1;" }] }
        ]));
        let without = doc(json!([
            { "type": "codeBlock", "content": [{ "type": "text", "text": "plain" }] }
        ]));
        assert_eq!(
            adf_to_markdown(&with),
            "```rust\nfn main() {}\nlet x = 1;\n```"
        );
        assert_eq!(adf_to_markdown(&without), "```\nplain\n```");
    }

    #[test]
    fn a_blockquote_prefixes_every_line_including_the_blank_one() {
        let value = doc(json!([
            { "type": "blockquote", "content": [paragraph("first"), paragraph("second")] }
        ]));
        assert_eq!(adf_to_markdown(&value), "> first\n>\n> second");
    }

    #[test]
    fn a_panel_renders_as_a_blockquote() {
        let value = doc(json!([
            { "type": "panel", "attrs": { "panelType": "info" }, "content": [paragraph("heads up")] }
        ]));
        assert_eq!(adf_to_markdown(&value), "> heads up");
    }

    #[test]
    fn a_rule_renders_as_three_dashes() {
        let value = doc(json!([paragraph("a"), { "type": "rule" }, paragraph("b")]));
        assert_eq!(adf_to_markdown(&value), "a\n\n---\n\nb");
    }

    #[test]
    fn a_hard_break_breaks_the_line_inside_the_paragraph() {
        let value = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "one" },
                { "type": "hardBreak" },
                { "type": "text", "text": "two" } ]}
        ]));
        assert_eq!(adf_to_markdown(&value), "one\ntwo");
    }

    #[test]
    fn text_marks_render_as_their_markdown_syntax() {
        let mark = |name: &str| {
            doc(json!([
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "x", "marks": [{ "type": name }] } ]}
            ]))
        };
        assert_eq!(adf_to_markdown(&mark("strong")), "**x**");
        assert_eq!(adf_to_markdown(&mark("em")), "*x*");
        assert_eq!(adf_to_markdown(&mark("code")), "`x`");
        assert_eq!(adf_to_markdown(&mark("strike")), "~~x~~");
        assert_eq!(adf_to_markdown(&mark("underline")), "_x_");
        assert_eq!(adf_to_markdown(&mark("subsup")), "x");
    }

    #[test]
    fn stacked_marks_nest_and_code_wins_over_emphasis() {
        let stacked = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "x",
                  "marks": [{ "type": "strong" }, { "type": "em" }, { "type": "strike" }] } ]}
        ]));
        let coded = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "x",
                  "marks": [{ "type": "code" }, { "type": "strong" }] } ]}
        ]));
        assert_eq!(adf_to_markdown(&stacked), "***~~x~~***");
        assert_eq!(adf_to_markdown(&coded), "`x`");
    }

    /// Jira stores the trailing space inside the strong run; `**bold **` is not bold anywhere.
    #[test]
    fn marks_keep_their_surrounding_whitespace_outside_the_syntax() {
        let value = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "a" },
                { "type": "text", "text": " bold ", "marks": [{ "type": "strong" }] },
                { "type": "text", "text": "b" } ]}
        ]));
        assert_eq!(adf_to_markdown(&value), "a **bold** b");
    }

    #[test]
    fn a_link_renders_bare_when_its_text_is_its_href() {
        let bare = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "https://buk.dev",
                  "marks": [{ "type": "link", "attrs": { "href": "https://buk.dev" } }] } ]}
        ]));
        let titled = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "docs",
                  "marks": [{ "type": "link", "attrs": { "href": "https://buk.dev" } },
                            { "type": "strong" }] } ]}
        ]));
        assert_eq!(adf_to_markdown(&bare), "https://buk.dev");
        assert_eq!(adf_to_markdown(&titled), "[**docs**](https://buk.dev)");
    }

    #[test]
    fn a_mention_renders_as_an_at_name_however_it_is_spelled() {
        let value = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "mention", "attrs": { "text": "@Ana Díaz" } },
                { "type": "text", "text": " y " },
                { "type": "mention", "attrs": { "displayName": "Beto" } },
                { "type": "text", "text": " y " },
                { "type": "mention", "attrs": { "id": "557058:x" } } ]}
        ]));
        assert_eq!(adf_to_markdown(&value), "@Ana Díaz y @Beto y ");
    }

    #[test]
    fn an_emoji_renders_as_its_short_name_or_its_glyph() {
        let value = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "emoji", "attrs": { "shortName": ":tada:", "text": "🎉" } },
                { "type": "text", "text": " " },
                { "type": "emoji", "attrs": { "text": "🙂" } } ]}
        ]));
        assert_eq!(adf_to_markdown(&value), ":tada: 🙂");
    }

    #[test]
    fn inline_and_embed_cards_render_their_url() {
        let value = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "inlineCard", "attrs": { "url": "https://a.dev/1" } },
                { "type": "text", "text": " " },
                { "type": "embedCard", "attrs": { "data": { "url": "https://b.dev/2" } } } ]}
        ]));
        assert_eq!(adf_to_markdown(&value), "https://a.dev/1 https://b.dev/2");
    }

    /// Markdown cannot carry a Jira attachment, and a silent drop reads as a truncated body.
    #[test]
    fn media_renders_as_an_attachment_placeholder() {
        let value = doc(json!([
            { "type": "mediaSingle", "content": [{ "type": "media", "attrs": { "id": "1" } }] },
            { "type": "mediaGroup", "content": [
                { "type": "media", "attrs": { "id": "2" } },
                { "type": "media", "attrs": { "id": "3" } } ]},
            { "type": "paragraph", "content": [
                { "type": "text", "text": "ver " },
                { "type": "mediaInline", "attrs": { "id": "4" } } ]}
        ]));
        assert_eq!(
            adf_to_markdown(&value),
            "[attachment]\n\n[attachment]\n\n[attachment]\n\nver [attachment]"
        );
    }

    #[test]
    fn a_table_renders_as_a_pipe_table_with_padded_rows() {
        let value = doc(json!([
            { "type": "table", "content": [
                { "type": "tableRow", "content": [
                    { "type": "tableHeader", "content": [paragraph("a")] },
                    { "type": "tableHeader", "content": [paragraph("b | c")] } ]},
                { "type": "tableRow", "content": [
                    { "type": "tableCell", "content": [paragraph("1")] } ]}
            ]}
        ]));
        assert_eq!(
            adf_to_markdown(&value),
            "| a | b \\| c |\n| --- | --- |\n| 1 |  |"
        );
    }

    #[test]
    fn an_unknown_node_keeps_its_content_and_a_childless_one_is_dropped() {
        let value = doc(json!([
            { "type": "expand", "attrs": { "title": "Detalles" },
              "content": [paragraph("inside"), { "type": "rule" }] },
            { "type": "status", "attrs": { "text": "DONE" } },
            paragraph("after"),
        ]));
        assert_eq!(adf_to_markdown(&value), "inside\n\n---\n\nafter");
    }

    /// A comment body arrives as a bare node often enough that rejecting one would be a bug.
    #[test]
    fn a_bare_block_node_renders_without_a_doc_wrapper() {
        assert_eq!(adf_to_markdown(&paragraph("hola")), "hola");
        assert_eq!(
            adf_to_markdown(&json!({ "content": [paragraph("hola")] })),
            "hola"
        );
    }

    // -- plain text ----------------------------------------------------------------------

    #[test]
    fn plain_text_flattens_every_block_to_one_line() {
        let value = doc(json!([
            { "type": "heading", "attrs": { "level": 1 },
              "content": [{ "type": "text", "text": "Title" }] },
            { "type": "paragraph", "content": [
                { "type": "text", "text": "bold", "marks": [{ "type": "strong" }] },
                { "type": "hardBreak" },
                { "type": "text", "text": "and  code", "marks": [{ "type": "code" }] } ]},
            { "type": "bulletList", "content": [
                { "type": "listItem", "content": [paragraph("a")] },
                { "type": "listItem", "content": [paragraph("b")] } ]}
        ]));
        assert_eq!(adf_plain_text(&value), "Title bold and code a b");
        assert_eq!(adf_plain_text(&Value::Null), "");
        assert_eq!(adf_plain_text(&json!({ "type": "doc" })), "");
    }

    /// A custom field holding an ADF value is rendered in a one-line property row, so the
    /// mention, the emoji and the card must survive the flattening as text.
    #[test]
    fn plain_text_keeps_mentions_emoji_and_card_urls() {
        let value = doc(json!([
            { "type": "paragraph", "content": [
                { "type": "mention", "attrs": { "text": "@Ana" } },
                { "type": "text", "text": " " },
                { "type": "emoji", "attrs": { "shortName": ":tada:" } },
                { "type": "text", "text": " " },
                { "type": "inlineCard", "attrs": { "url": "https://a.dev" } } ]}
        ]));
        assert_eq!(adf_plain_text(&value), "@Ana :tada: https://a.dev");
    }

    // -- markdown → ADF ------------------------------------------------------------------

    #[test]
    fn an_empty_markdown_body_is_an_empty_document() {
        assert_eq!(markdown_to_adf(""), doc(json!([])));
        assert_eq!(markdown_to_adf("\n\n   \n"), doc(json!([])));
    }

    #[test]
    fn markdown_headings_and_paragraphs_become_their_adf_nodes() {
        assert_eq!(
            markdown_to_adf("# One\n\nline a\nline b"),
            doc(json!([
                { "type": "heading", "attrs": { "level": 1 },
                  "content": [{ "type": "text", "text": "One" }] },
                paragraph("line a line b"),
            ]))
        );
    }

    /// What the scanner still refuses: a hash with no space is not a heading, and a pipe row
    /// with no `| --- |` under it is not a table. A bracket pair that is not a link — no
    /// `](`, a href carrying whitespace, an empty half — stays the text it was written as.
    #[test]
    fn markdown_beyond_the_subset_stays_plain_text() {
        assert_eq!(
            markdown_to_adf("#nospace\n| a | b |\n[t] (u)"),
            doc(json!([paragraph("#nospace | a | b | [t] (u)")]))
        );
        for text in ["[t]()", "[](u)", r#"[t](u "title")"#, "[a[b]](u)"] {
            assert_eq!(
                markdown_to_adf(text),
                doc(json!([paragraph(text)])),
                "{text}"
            );
        }
    }

    /// Rule 2 on the one construct that carries a user's own words. `marked_text` writes a link
    /// mark as `[label](href)`, and a scanner that could not read its own output turned every
    /// labelled link of a pulled description into the literal characters `[`, `](` and `)`
    /// around a link node whose text was the bare href — from an edit to another line entirely.
    #[test]
    fn a_labelled_link_reads_back_as_the_link_it_was_written_from() {
        let link = |text: &str, href: &str| {
            json!({ "type": "text", "text": text,
                    "marks": [{ "type": "link", "attrs": { "href": href } }] })
        };
        assert_eq!(
            markdown_to_adf("see [the runbook](https://buk.dev/x) now"),
            doc(json!([{ "type": "paragraph", "content": [
                { "type": "text", "text": "see " },
                link("the runbook", "https://buk.dev/x"),
                { "type": "text", "text": " now" },
            ]}]))
        );
        // A href of its own parentheses is not ended by the first `)`, exactly as `trim_url_end`
        // refuses to cut one off a bare URL.
        assert_eq!(
            markdown_to_adf("[wiki](https://wiki/x_(y))"),
            doc(json!([{ "type": "paragraph",
                         "content": [link("wiki", "https://wiki/x_(y)")] }]))
        );
        // The label is markdown of its own: `marked_text` writes a node carrying both marks
        // this way, and both marks have to survive the trip back.
        assert_eq!(
            markdown_to_adf("[**bold**](u)"),
            doc(json!([{ "type": "paragraph", "content": [
                json!({ "type": "text", "text": "bold",
                        "marks": [{ "type": "strong" },
                                  { "type": "link", "attrs": { "href": "u" } }] })
            ]}]))
        );
        // The full round trip, on the shape a pulled description actually holds.
        let source = "Ver [el runbook](https://buk.dev/runbook) y `sync`";
        assert_eq!(adf_to_markdown(&markdown_to_adf(source)), source);
    }

    /// The other half of rule 2: every block `adf_to_markdown` writes reads back as the node
    /// it came from. Before this, one edited word rewrote a Jira table as a paragraph of
    /// pipes, a rule as the text `---`, a panel as a paragraph starting `> `, and an h4 as the
    /// literal `#### x`.
    #[test]
    fn markdown_reads_back_every_block_the_renderer_writes() {
        assert_eq!(
            markdown_to_adf("#### deep"),
            doc(json!([{ "type": "heading", "attrs": { "level": 4 },
                        "content": [{ "type": "text", "text": "deep" }] }]))
        );
        assert_eq!(markdown_to_adf("---"), doc(json!([{ "type": "rule" }])));
        assert_eq!(
            markdown_to_adf("> quoted\n> - a"),
            doc(json!([{
                "type": "blockquote",
                "content": [
                    paragraph("quoted"),
                    { "type": "bulletList", "content": [
                        { "type": "listItem", "content": [paragraph("a")] }] },
                ],
            }]))
        );
        let cell = |kind: &str, text: &str| json!({ "type": kind, "attrs": {}, "content": [paragraph(text)] });
        assert_eq!(
            markdown_to_adf("| campo | valor |\n| --- | --- |\n| a | 1 |"),
            doc(json!([{
                "type": "table",
                "attrs": { "isNumberColumnEnabled": false, "layout": "default" },
                "content": [
                    { "type": "tableRow", "content": [
                        cell("tableHeader", "campo"), cell("tableHeader", "valor")] },
                    { "type": "tableRow", "content": [
                        cell("tableCell", "a"), cell("tableCell", "1")] },
                ],
            }]))
        );
        assert_eq!(
            markdown_to_adf("- uno\n  - uno.a"),
            doc(json!([{ "type": "bulletList", "content": [{
                "type": "listItem",
                "content": [
                    paragraph("uno"),
                    { "type": "bulletList", "content": [
                        { "type": "listItem", "content": [paragraph("uno.a")] }] },
                ],
            }] }]))
        );
    }

    /// The whole point, end to end: a rich Jira description that a user edits one line of has
    /// to come back as the same node types it went out as.
    #[test]
    fn a_rich_description_survives_an_edit_and_a_push_back() {
        let original = doc(json!([
            { "type": "heading", "attrs": { "level": 4 },
              "content": [{ "type": "text", "text": "Contexto" }] },
            paragraph("uno"),
            { "type": "bulletList", "content": [{
                "type": "listItem",
                "content": [
                    paragraph("dos"),
                    { "type": "bulletList", "content": [
                        { "type": "listItem", "content": [paragraph("dos.a")] }] },
                ],
            }] },
            { "type": "rule" },
            { "type": "panel", "attrs": { "panelType": "info" },
              "content": [paragraph("ojo con esto")] },
            { "type": "table", "attrs": { "isNumberColumnEnabled": false, "layout": "default" },
              "content": [
                { "type": "tableRow", "content": [
                    { "type": "tableHeader", "attrs": {}, "content": [paragraph("campo")] },
                    { "type": "tableHeader", "attrs": {}, "content": [paragraph("valor")] }] },
                { "type": "tableRow", "content": [
                    { "type": "tableCell", "attrs": {}, "content": [paragraph("a")] },
                    { "type": "tableCell", "attrs": {}, "content": [paragraph("1")] }] },
              ] },
        ]));
        let edited = format!("{}\n\nuna línea más", adf_to_markdown(&original));
        let pushed = markdown_to_adf(&edited);
        let kinds: Vec<&str> = document_nodes(&pushed).iter().map(node_kind).collect();
        assert_eq!(
            kinds,
            vec![
                "heading",
                "paragraph",
                "bulletList",
                "rule",
                // A panel comes back as the blockquote it rendered as; its text is intact.
                "blockquote",
                "table",
                "paragraph",
            ]
        );
        // And the round trip is stable from there on.
        assert_eq!(adf_to_markdown(&pushed), edited);
    }

    #[test]
    fn markdown_lists_become_adf_lists_and_do_not_merge() {
        let list = |kind: &str, items: Value| json!({ "type": kind, "content": items });
        let item = |text: &str| json!({ "type": "listItem", "content": [paragraph(text)] });
        assert_eq!(
            markdown_to_adf("- a\n* b\n1. c\n2. d"),
            doc(json!([
                list("bulletList", json!([item("a"), item("b")])),
                list("orderedList", json!([item("c"), item("d")])),
            ]))
        );
    }

    #[test]
    fn an_ordered_list_that_does_not_start_at_one_carries_its_order() {
        let content = markdown_to_adf("3. a\n4. b");
        assert_eq!(content["content"][0]["attrs"], json!({ "order": 3 }));
        assert_eq!(markdown_to_adf("1. a")["content"][0].get("attrs"), None);
    }

    #[test]
    fn markdown_fenced_code_carries_its_language_and_survives_an_open_fence() {
        assert_eq!(
            markdown_to_adf("```sh\nfleet board sync\n  --full\n```"),
            doc(json!([
                { "type": "codeBlock", "attrs": { "language": "sh" },
                  "content": [{ "type": "text", "text": "fleet board sync\n  --full" }] }
            ]))
        );
        assert_eq!(
            markdown_to_adf("```\nleft open"),
            doc(json!([
                { "type": "codeBlock", "content": [{ "type": "text", "text": "left open" }] }
            ]))
        );
    }

    #[test]
    fn markdown_inline_marks_and_urls_become_adf_marks() {
        assert_eq!(
            markdown_to_adf("run `make ci` then **ship** via https://fleet.dev/docs ok"),
            doc(json!([
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "run " },
                    { "type": "text", "text": "make ci", "marks": [{ "type": "code" }] },
                    { "type": "text", "text": " then " },
                    { "type": "text", "text": "ship", "marks": [{ "type": "strong" }] },
                    { "type": "text", "text": " via " },
                    { "type": "text", "text": "https://fleet.dev/docs",
                      "marks": [{ "type": "link",
                                  "attrs": { "href": "https://fleet.dev/docs" } }] },
                    { "type": "text", "text": " ok" } ]}
            ]))
        );
    }

    /// A URL that ends in a parenthesis of its own keeps it; one the sentence put there does
    /// not. Cutting the wrong one writes a link to a page that does not exist.
    #[test]
    fn a_url_keeps_the_parenthesis_it_opened_and_drops_the_sentence_s() {
        let href = |value: &Value| {
            value["content"][0]["content"][0]["marks"][0]["attrs"]["href"]
                .as_str()
                .map(str::to_owned)
        };
        assert_eq!(
            href(&markdown_to_adf("https://wiki.dev/x_(y)")).as_deref(),
            Some("https://wiki.dev/x_(y)")
        );
        assert_eq!(
            markdown_to_adf("(see https://wiki.dev/x)")["content"][0]["content"][1]["marks"][0]
                ["attrs"]["href"]
                .as_str(),
            Some("https://wiki.dev/x")
        );
        // Sentence punctuation after the URL's own parenthesis still goes.
        assert_eq!(
            href(&markdown_to_adf("https://wiki.dev/x_(y).")).as_deref(),
            Some("https://wiki.dev/x_(y)")
        );
    }

    /// The renderer leaves an unterminated `**` as two asterisks; Jira must show the same.
    #[test]
    fn unterminated_marks_stay_text_and_never_produce_an_empty_node() {
        assert_eq!(
            markdown_to_adf("**bold and `code"),
            doc(json!([paragraph("**bold and `code")]))
        );
        assert_eq!(
            markdown_to_adf("empty **** marks"),
            doc(json!([paragraph("empty **** marks")]))
        );
        // An empty inline-code span has no ADF spelling, so it drops out rather than becoming
        // the `{"type":"text","text":""}` Jira rejects.
        assert_eq!(
            markdown_to_adf("``"),
            doc(json!([{
                "type": "paragraph", "content": []
            }]))
        );
    }

    #[test]
    fn multibyte_markdown_survives_span_splitting() {
        assert_eq!(
            markdown_to_adf("añ **ñu** fin"),
            doc(json!([
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "añ " },
                    { "type": "text", "text": "ñu", "marks": [{ "type": "strong" }] },
                    { "type": "text", "text": " fin" } ]}
            ]))
        );
    }

    // -- round trips ---------------------------------------------------------------------

    /// The contract that matters: anything a user can type into a Fleet card description comes
    /// back byte for byte after a trip through Jira.
    #[test]
    fn markdown_round_trips_through_adf_for_the_supported_subset() {
        for source in [
            "just a paragraph",
            "# One\n\n## Two\n\n### Three",
            "one paragraph\n\nanother paragraph",
            "- a\n- b",
            "1. a\n2. b",
            "3. a\n4. b",
            "- a\n\n1. b",
            "```rust\nfn main() {}\n```",
            "```\nno language\n```",
            "run `make ci` then **ship** via https://fleet.dev/docs ok",
            "see http://x.dev/a. and shttp://y.dev",
            "# Título\n\nañ **ñu** y `ñ`\n\n- ítem uno\n- ítem dos",
            "**all bold**",
            "a paragraph\n\n- a list\n- of two\n\n```sh\necho ok\n```\n\nand a tail",
        ] {
            let round_tripped = adf_to_markdown(&markdown_to_adf(source));
            assert_eq!(round_tripped, source, "round trip changed {source:?}");
        }
    }

    /// The other direction is not an identity — ADF is richer — but it must be stable: a body
    /// pulled, rendered, edited and pushed twice must not drift.
    #[test]
    fn rendering_is_stable_across_a_second_trip() {
        let once = adf_to_markdown(&fixture());
        let twice = adf_to_markdown(&markdown_to_adf(&once));
        let thrice = adf_to_markdown(&markdown_to_adf(&twice));
        assert_eq!(twice, thrice);
        assert!(twice.contains("## Contexto"));
    }

    // -- the realistic fixture -----------------------------------------------------------

    #[test]
    fn a_realistic_jira_description_renders_as_readable_markdown() {
        assert_eq!(
            adf_to_markdown(&fixture()),
            concat!(
                "## Contexto\n",
                "\n",
                "El `sync` falla cuando el board tiene **más de 200** tarjetas. ",
                "Reportado por @Danny Fuentes :warning:\n",
                "\n",
                "Ver [el runbook](https://buk.dev/runbook) y ",
                "https://buk.atlassian.net/browse/SP-1\n",
                "\n",
                "### Pasos\n",
                "\n",
                "1. Abrir el board\n",
                "2. Sincronizar\n",
                "   - con `--full`\n",
                "\n",
                "```bash\n",
                "fleet board sync --full\n",
                "echo ok\n",
                "```\n",
                "\n",
                "> No correr en producción.\n",
                "\n",
                "---\n",
                "\n",
                "| Entorno | Estado |\n",
                "| --- | --- |\n",
                "| staging | *ok* |\n",
                "\n",
                "[attachment]\n",
                "\n",
                "línea uno\n",
                "línea dos",
            )
        );
    }

    #[test]
    fn a_realistic_jira_description_flattens_to_one_line() {
        let plain = adf_plain_text(&fixture());
        assert!(!plain.contains('\n'));
        assert!(plain.starts_with("Contexto El sync falla cuando el board tiene más de 200"));
        assert!(plain.contains("@Danny Fuentes :warning:"));
        assert!(plain.contains("fleet board sync --full echo ok"));
        assert!(plain.contains("[attachment] línea uno línea dos"));
    }

    /// A Jira code block whose body contains a fence used to close early: the tail re-parsed as
    /// loose paragraphs, and editing that description in Fleet wrote the mangled text back.
    #[test]
    fn a_code_block_containing_a_fence_round_trips_through_a_longer_one() {
        let adf = json!({
            "type": "doc",
            "version": 1,
            "content": [{
                "type": "codeBlock",
                "attrs": { "language": "md" },
                "content": [{ "type": "text", "text": "```sh\necho hi\n```" }]
            }]
        });
        let markdown = adf_to_markdown(&adf);
        assert!(
            markdown.starts_with("````md\n"),
            "the fence must outrun the body: {markdown}"
        );
        assert_eq!(
            markdown_to_adf(&markdown),
            adf,
            "and it has to come back whole"
        );
    }
}
