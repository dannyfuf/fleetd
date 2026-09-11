//! One canonical rendering of a JSON frame, so a golden compares the *shape* and not a key order
//! no code here decides.
//!
//! `serde_json`'s object representation is a `BTreeMap` by default and an `IndexMap` under its
//! `preserve_order` feature — and `gpui` turns that feature on. Feature unification is per build
//! graph, so the same frame serializes with sorted keys under `cargo test -p fleet-daemon` and in
//! insertion order under `cargo test --workspace`: a literal byte comparison passes one gate and
//! fails the other, which makes the golden a test of the build and not of the adapter.
//!
//! Key order is also not part of any contract on these wires. Both harnesses parse JSON, and
//! neither documents an order. What a golden must catch is a renamed field, a case change, a
//! changed nesting, an added or dropped key and a changed value — all of which survive
//! canonicalisation. So the frame is re-rendered with every object key sorted, on both sides of
//! the assertion, and the comparison stays a string comparison so a failure still prints a diff a
//! human can read.

use serde_json::Value;

/// The frame, re-rendered with every object key sorted at every depth.
pub(crate) fn canonical(value: &Value) -> String {
    serde_json::to_string(&sorted(value)).unwrap_or_else(|error| panic!("{error}"))
}

/// The same, for a golden written as a literal — so a literal in either order still passes.
pub(crate) fn canonical_text(json: &str) -> String {
    canonical(&serde_json::from_str::<Value>(json).unwrap_or_else(|error| panic!("{error}")))
}

/// `value` with every object key sorted. Arrays keep their order, which *is* contractual: the
/// Claude `user` frame's block order decides whether the CLI reads a slash command.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut ordered: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (key, field) in fields {
                ordered.insert(key.clone(), sorted(field));
            }
            Value::Object(ordered.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two renderings of one frame that differ only in key order are the same golden; a renamed
    /// or re-cased key is not.
    #[test]
    fn canonicalisation_ignores_key_order_and_nothing_else() {
        let a = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
        let b = r#"{"message":{"content":"hi","role":"user"},"type":"user"}"#;
        assert_eq!(canonical_text(a), canonical_text(b));

        for different in [
            r#"{"message":{"content":"hi","Role":"user"},"type":"user"}"#,
            r#"{"message":{"content":"hi","role":"user"},"kind":"user"}"#,
            r#"{"message":{"content":"hi","role":"user"},"type":"user","extra":1}"#,
            r#"{"message":{"content":"HI","role":"user"},"type":"user"}"#,
        ] {
            assert_ne!(
                canonical_text(a),
                canonical_text(different),
                "`{different}` is not the same frame"
            );
        }

        // Array order is contractual and is never sorted away.
        assert_ne!(
            canonical_text(r#"{"content":[{"type":"image"},{"type":"text"}]}"#),
            canonical_text(r#"{"content":[{"type":"text"},{"type":"image"}]}"#),
        );
    }
}
