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

mod parse;
mod render;
#[cfg(test)]
mod tests;

pub use parse::markdown_to_adf;
pub use render::{adf_plain_text, adf_to_markdown};

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
