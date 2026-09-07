use super::*;

/// What an image, file or embedded media renders as: markdown has no attachment syntax, and a
/// silent drop reads as a truncated description.
const ATTACHMENT: &str = "[attachment]";

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
pub(super) fn document_nodes(doc: &Value) -> &[Value] {
    let kind = node_kind(doc);
    if kind.is_empty() || kind == "doc" {
        children(doc)
    } else {
        std::slice::from_ref(doc)
    }
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
