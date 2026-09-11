//! Small pure helpers shared by the Claude mapping modules.

use std::collections::BTreeMap;

use fleet_core::agents::{
    ItemKind, ItemPatch, ItemPayloadPatch, ItemStatus, StreamKind, ToolPatch,
};
use serde_json::Value;

/// An empty assistant-text item.
#[must_use]
pub(crate) fn assistant_item() -> ItemKind {
    ItemKind::AssistantText {
        text: String::new(),
    }
}

/// An empty reasoning item. Claude fills only summary part 0.
#[must_use]
pub(crate) fn reasoning_item() -> ItemKind {
    ItemKind::Reasoning {
        summary: BTreeMap::new(),
        raw: BTreeMap::new(),
    }
}

/// A patch that replaces one stream's accumulated text.
#[must_use]
pub(crate) fn text_replacement(stream: StreamKind, text: &str) -> ItemPatch {
    let payload = match stream {
        StreamKind::AssistantText => ItemPayloadPatch::AssistantText {
            text: text.to_owned(),
        },
        StreamKind::ReasoningSummary { part } => ItemPayloadPatch::Reasoning {
            summary: Some(BTreeMap::from([(part, text.to_owned())])),
            raw: None,
        },
        StreamKind::ReasoningRaw { part } => ItemPayloadPatch::Reasoning {
            summary: None,
            raw: Some(BTreeMap::from([(part, text.to_owned())])),
        },
        StreamKind::PlanText => ItemPayloadPatch::Plan {
            text: text.to_owned(),
        },
        StreamKind::CommandOutput => ItemPayloadPatch::Tool(Box::new(ToolPatch {
            output: Some(text.to_owned()),
            ..ToolPatch::default()
        })),
    };
    ItemPatch {
        payload: Some(payload),
        status: None,
    }
}

/// The top-level bullet or numbered steps of a Markdown plan.
#[must_use]
pub(crate) fn top_level_steps(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let line = line.trim_end();
            ["- ", "* ", "+ "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .or_else(|| {
                    let (number, rest) = line.split_once(". ")?;
                    number
                        .chars()
                        .all(|character| character.is_ascii_digit())
                        .then_some(rest)
                })
        })
        .map(ToOwned::to_owned)
        .collect()
}

/// One provider notice, with the CLI's own terminal affordances removed.
///
/// Claude's copy is written for its TUI and ships the keystroke that would act on it —
/// `Stop hook error occurred · ctrl+o to see`. Fleet draws a notice as one muted line and its
/// only key vocabulary is `^s`-prefixed, so `ctrl+o` is dead copy naming a key nothing here
/// implements. `None` means nothing was left to say.
#[must_use]
pub(crate) fn notice_text(text: &str) -> Option<String> {
    let mut clauses: Vec<&str> = text.split('·').map(str::trim).collect();
    while clauses
        .last()
        .is_some_and(|clause| is_key_affordance(clause))
    {
        clauses.pop();
    }
    let notice = clauses.join(" · ");
    let notice = notice.trim();
    (!notice.is_empty()).then(|| notice.to_owned())
}

/// Whether a notice clause is only a terminal keystroke the user is invited to press.
fn is_key_affordance(clause: &str) -> bool {
    let lower = clause.to_ascii_lowercase();
    [
        "ctrl+", "ctrl-", "cmd+", "shift+", "alt+", "option+", "esc ", "press ",
    ]
    .iter()
    .any(|token| lower.starts_with(token))
        || lower == "esc"
}

/// The `ItemStatus` a Claude task status names.
#[must_use]
pub(crate) fn task_status(status: Option<&str>) -> Option<ItemStatus> {
    match status {
        Some("completed") => Some(ItemStatus::Completed),
        Some("failed") => Some(ItemStatus::Failed),
        // `killed` is a stop, not a failure: the row must not read red.
        Some("killed") => Some(ItemStatus::Stopped),
        _ => None,
    }
}

/// Text extraction from a tool-result `content`, recursive and permissive.
///
/// A string is itself; an array concatenates; an object yields `.text` when it is a string, else
/// recurses into `.content`.
#[must_use]
pub(crate) fn content_text(content: Option<&Value>) -> String {
    fn extract(value: &Value, depth: u8, out: &mut Vec<String>) {
        if depth > 8 {
            return;
        }
        match value {
            Value::String(text) => out.push(text.clone()),
            Value::Array(items) => {
                for item in items {
                    extract(item, depth.saturating_add(1), out);
                }
            }
            Value::Object(fields) => {
                if let Some(Value::String(text)) = fields.get("text") {
                    out.push(text.clone());
                } else if let Some(nested) = fields.get("content") {
                    extract(nested, depth.saturating_add(1), out);
                }
            }
            _ => {}
        }
    }
    let Some(content) = content else {
        return String::new();
    };
    if content.is_null() {
        return String::new();
    }
    let mut parts = Vec::new();
    extract(content, 0, &mut parts);
    if parts.is_empty() {
        return compact_json(content);
    }
    parts.join("\n")
}

/// The names in a `tools`/`slash_commands`/`skills` vocabulary, whichever shape it arrives in.
#[must_use]
pub(crate) fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value.as_str().map(ToOwned::to_owned).or_else(|| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
        })
        .collect()
}

/// A string field of a flattened frame.
#[must_use]
pub(crate) fn string_field(fields: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

/// A `u64` field of a flattened frame, defaulting to zero.
#[must_use]
pub(crate) fn u64_field(fields: &serde_json::Map<String, Value>, key: &str) -> u64 {
    fields.get(key).and_then(Value::as_u64).unwrap_or_default()
}

/// A `u32` field of a flattened frame, saturating.
#[must_use]
pub(crate) fn u32_field(fields: &serde_json::Map<String, Value>, key: &str) -> u32 {
    u64_field(fields, key).try_into().unwrap_or(u32::MAX)
}

/// Compact JSON, for a payload Fleet is *rendering* — never for a log line.
#[must_use]
pub(crate) fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

/// A stable fingerprint of a parsed tool input.
///
/// `ItemUpdated` is emitted only when this changes, which is the difference between ~1 and ~30
/// events per tool call — and over SSH between a live transcript and a slideshow.
#[must_use]
pub(crate) fn value_fingerprint(value: &Value) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    compact_json(value).hash(&mut hasher);
    hasher.finish()
}

/// Removes ANSI SGR sequences from harness-authored copy.
#[must_use]
pub(crate) fn strip_ansi(value: &str) -> String {
    let mut clean = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            clean.push(character);
        }
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_control_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mblocked\u{1b}[0m"), "blocked");
    }

    #[test]
    fn extracts_only_top_level_plan_steps() {
        assert_eq!(
            top_level_steps("# Plan\n\n- first\n  - nested\n2. second\ntext"),
            ["first", "second"]
        );
    }

    #[test]
    fn a_notice_drops_the_clis_own_terminal_affordance() {
        assert_eq!(
            notice_text("Stop hook error occurred · ctrl+o to see"),
            Some("Stop hook error occurred".to_owned())
        );
        assert_eq!(notice_text("  esc  "), None);
    }

    #[test]
    fn tool_result_text_extraction_is_recursive_and_permissive() {
        assert_eq!(content_text(Some(&serde_json::json!("plain"))), "plain");
        assert_eq!(
            content_text(Some(&serde_json::json!([
                {"type": "text", "text": "one"},
                {"content": [{"text": "two"}]}
            ]))),
            "one\ntwo"
        );
        assert_eq!(content_text(None), "");
        assert_eq!(content_text(Some(&Value::Null)), "");
    }

    #[test]
    fn a_reused_input_document_fingerprints_identically() {
        let first = serde_json::json!({"command": "ls -la", "n": 1});
        let same = serde_json::json!({"command": "ls -la", "n": 1});
        let other = serde_json::json!({"command": "ls -la", "n": 2});
        assert_eq!(value_fingerprint(&first), value_fingerprint(&same));
        assert_ne!(value_fingerprint(&first), value_fingerprint(&other));
    }
}
