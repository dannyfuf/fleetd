//! Structural descriptions of a decode failure, carrying no wire data.
//!
//! Agent transcripts carry source, credentials and customer data, so `tracing::warn!(?payload)`
//! on a decode failure is an exfiltration bug, not a debugging convenience (NATIVE-AGENTS.md
//! §3.1). Everything a harness may say about a frame it could not decode goes through this
//! module: a count, a set of structural issue kinds, the depth of the deepest path, and the
//! *names* of the fields that were present. Never a value, never a message derived from one.
//!
//! t3code's `schemaIssueDiagnostics` (`errors.ts:33-63`) is the shape; the three tests per
//! harness that enforce it come with it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How many field names one fingerprint may name.
///
/// A wide object would otherwise turn the field-name list into a payload-sized log line, and the
/// first two dozen names are enough to identify which frame drifted.
const MAX_FIELD_NAMES: usize = 24;

/// The kind of structural problem a decode hit.
///
/// Deliberately an enum and not the serde message: serde's own text embeds field values
/// ("unknown variant `sk-live-…`"), so the message can never be stored or logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// The bytes were not JSON at all.
    Syntax,
    /// The stream ended inside a value.
    Eof,
    /// The reader itself failed.
    Io,
    /// A required field was absent.
    MissingField,
    /// A field held the wrong JSON type.
    TypeMismatch,
    /// A tagged union carried a variant this build does not know.
    UnknownVariant,
    /// A struct carried a field this build does not know.
    UnknownField,
    /// The payload decoded as JSON but not as the expected shape, in some other way.
    Shape,
}

/// A decode failure described without any of the data that caused it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFingerprint {
    /// How many problems were observed.
    pub issue_count: u32,
    /// The structural kinds of those problems.
    pub issue_kinds: BTreeSet<IssueKind>,
    /// Depth of the deepest path in the payload.
    pub max_path_depth: u16,
    /// Names — never values — of the fields the payload carried.
    pub present_fields: BTreeSet<String>,
}

impl SchemaFingerprint {
    /// Describes one serde failure over `payload`.
    #[must_use]
    pub fn of(error: &serde_json::Error, payload: &Value) -> Self {
        let mut fingerprint = Self::structural(payload);
        fingerprint.issue_count = 1;
        fingerprint.issue_kinds.insert(classify(error));
        fingerprint
    }

    /// Describes a payload with no serde error behind it, such as an unroutable envelope.
    #[must_use]
    pub fn of_value(kind: IssueKind, payload: &Value) -> Self {
        let mut fingerprint = Self::structural(payload);
        fingerprint.issue_count = 1;
        fingerprint.issue_kinds.insert(kind);
        fingerprint
    }

    /// Describes a failure whose payload could not even be parsed into a `Value`.
    #[must_use]
    pub fn of_unparsable(error: &serde_json::Error) -> Self {
        Self {
            issue_count: 1,
            issue_kinds: BTreeSet::from([classify(error)]),
            max_path_depth: 0,
            present_fields: BTreeSet::new(),
        }
    }

    /// Merges another fingerprint into this one, keeping the count additive.
    ///
    /// This is what makes "decode history per item, not per response" (§4.5 rule 3) reportable:
    /// one `thread/read` with three unrecognised items is one fingerprint with three issues.
    pub fn merge(&mut self, other: &Self) {
        self.issue_count = self.issue_count.saturating_add(other.issue_count);
        self.issue_kinds.extend(other.issue_kinds.iter().copied());
        self.max_path_depth = self.max_path_depth.max(other.max_path_depth);
        for field in &other.present_fields {
            if self.present_fields.len() >= MAX_FIELD_NAMES {
                break;
            }
            self.present_fields.insert(field.clone());
        }
    }

    fn structural(payload: &Value) -> Self {
        let mut present_fields = BTreeSet::new();
        if let Some(object) = payload.as_object() {
            for key in object.keys().take(MAX_FIELD_NAMES) {
                present_fields.insert(key.clone());
            }
        }
        Self {
            issue_count: 0,
            issue_kinds: BTreeSet::new(),
            max_path_depth: depth(payload, 0),
            present_fields,
        }
    }

    /// A one-line rendering safe for a log line: counts and names only.
    #[must_use]
    pub fn summary(&self) -> String {
        let kinds = self
            .issue_kinds
            .iter()
            .map(|kind| format!("{kind:?}"))
            .collect::<Vec<_>>()
            .join(",");
        let fields = self
            .present_fields
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "issues={} kinds=[{kinds}] depth={} fields=[{fields}]",
            self.issue_count, self.max_path_depth
        )
    }
}

/// Classifies a serde failure without retaining its message.
///
/// The keyword scan reads serde's own wording to pick a *kind*; the message itself never leaves
/// this function, which is why the classification is a match on `contains` rather than a capture.
fn classify(error: &serde_json::Error) -> IssueKind {
    match error.classify() {
        serde_json::error::Category::Syntax => return IssueKind::Syntax,
        serde_json::error::Category::Eof => return IssueKind::Eof,
        serde_json::error::Category::Io => return IssueKind::Io,
        serde_json::error::Category::Data => {}
    }
    let message = error.to_string();
    if message.starts_with("missing field") {
        IssueKind::MissingField
    } else if message.starts_with("unknown variant") {
        IssueKind::UnknownVariant
    } else if message.starts_with("unknown field") {
        IssueKind::UnknownField
    } else if message.starts_with("invalid type") || message.starts_with("invalid value") {
        IssueKind::TypeMismatch
    } else {
        IssueKind::Shape
    }
}

/// The depth of the deepest path in a payload, saturating so a pathological frame cannot hang.
fn depth(value: &Value, level: u16) -> u16 {
    if level > 64 {
        return level;
    }
    match value {
        Value::Object(fields) => fields
            .values()
            .map(|value| depth(value, level.saturating_add(1)))
            .max()
            .unwrap_or(level),
        Value::Array(items) => items
            .iter()
            .map(|value| depth(value, level.saturating_add(1)))
            .max()
            .unwrap_or(level),
        _ => level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "sk-live-DEADBEEF-not-a-real-token";

    fn payload() -> Value {
        serde_json::json!({
            "type": "control_request",
            "request": {"subtype": "can_use_tool", "input": {"token": SECRET}},
        })
    }

    #[test]
    fn a_fingerprint_names_fields_and_never_their_values() {
        #[derive(serde::Deserialize)]
        #[expect(
            dead_code,
            reason = "the field exists so the decode fails by name; nothing reads it"
        )]
        struct Expected {
            missing: String,
        }
        let error = serde_json::from_value::<Expected>(payload())
            .err()
            .unwrap_or_else(|| panic!("the decode must fail"));
        let fingerprint = SchemaFingerprint::of(&error, &payload());

        assert_eq!(fingerprint.issue_count, 1);
        assert_eq!(
            fingerprint.issue_kinds,
            BTreeSet::from([IssueKind::MissingField])
        );
        assert_eq!(fingerprint.max_path_depth, 3);
        assert_eq!(
            fingerprint.present_fields,
            BTreeSet::from(["request".to_owned(), "type".to_owned()])
        );

        let rendered = format!("{fingerprint:?} {}", fingerprint.summary());
        assert!(
            !rendered.contains(SECRET) && !rendered.contains("token"),
            "a fingerprint must not carry payload data: {rendered}"
        );
    }

    #[test]
    fn an_unknown_variant_message_never_reaches_the_fingerprint() {
        #[derive(Debug, serde::Deserialize)]
        #[serde(tag = "type")]
        enum Closed {
            #[serde(rename = "known")]
            Known,
        }
        let value = serde_json::json!({"type": SECRET});
        let error = serde_json::from_value::<Closed>(value.clone())
            .err()
            .unwrap_or_else(|| panic!("the decode must fail"));
        // serde's own message embeds the value; the fingerprint must not.
        assert!(error.to_string().contains(SECRET));
        let fingerprint = SchemaFingerprint::of(&error, &value);
        assert_eq!(
            fingerprint.issue_kinds,
            BTreeSet::from([IssueKind::UnknownVariant])
        );
        assert!(!format!("{fingerprint:?}").contains(SECRET));
    }

    #[test]
    fn merging_keeps_the_count_additive_and_bounds_the_field_names() {
        let mut first = SchemaFingerprint::of_value(IssueKind::Shape, &payload());
        let wide = Value::Object(
            (0..64)
                .map(|index| (format!("field{index}"), Value::Null))
                .collect(),
        );
        first.merge(&SchemaFingerprint::of_value(IssueKind::TypeMismatch, &wide));
        assert_eq!(first.issue_count, 2);
        assert_eq!(first.issue_kinds.len(), 2);
        assert!(first.present_fields.len() <= MAX_FIELD_NAMES);
    }

    #[test]
    fn unparsable_bytes_are_a_syntax_issue_with_no_fields() {
        let error = serde_json::from_str::<Value>("{not json")
            .err()
            .unwrap_or_else(|| panic!("the decode must fail"));
        let fingerprint = SchemaFingerprint::of_unparsable(&error);
        assert_eq!(fingerprint.issue_kinds, BTreeSet::from([IssueKind::Syntax]));
        assert!(fingerprint.present_fields.is_empty());
    }
}
