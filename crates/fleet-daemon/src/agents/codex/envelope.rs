//! The four `codex app-server` envelopes, and the structural rule that tells them apart.
//!
//! **It is JSON-RPC-*shaped* and it is not JSON-RPC 2.0.** There is no `jsonrpc` field in either
//! direction, and `params` is omitted entirely when absent — not `null`, not `{}`. A strict
//! JSON-RPC crate is the wrong tool here and will produce frames Codex may or may not tolerate,
//! which is why these four structs are hand-written while everything they carry is generated.
//!
//! Classification is **structural**, in this order:
//!
//! 1. `method` plus a string-or-number `id` ⇒ a request;
//! 2. `method` with the key `id` **absent** ⇒ a notification. Note the exactness:
//!    `{"method":…,"id":null}` is *not* a notification;
//! 3. a valid id with `result` or `error` ⇒ a response.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Fleet's own outbound request. `id` is written first so a mock peer can find it with `sed`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutboundRequest<'a> {
    /// Monotonic per-connection id.
    pub id: u64,
    /// The method name.
    pub method: &'a str,
    /// Params, omitted entirely when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Fleet's own outbound notification.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutboundNotification<'a> {
    /// The method name.
    pub method: &'a str,
    /// Params, omitted entirely when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Fleet's own outbound response to a server request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutboundResponse {
    /// The server's own request id, echoed back verbatim.
    pub id: Value,
    /// The result payload.
    pub result: Value,
}

/// Fleet's own outbound error response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutboundError<'a> {
    /// The server's own request id, echoed back verbatim.
    pub id: Value,
    /// The error body.
    pub error: ErrorBody<'a>,
}

/// A JSON-RPC-shaped error body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody<'a> {
    /// The numeric code.
    pub code: i64,
    /// A short message. Fleet's own copy, never an echo of a payload.
    pub message: &'a str,
}

/// The error code for a method Fleet does not implement.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// The error code Fleet answers when its inbound handler budget is full.
pub const SERVER_BUSY: i64 = -32001;

/// One inbound line, classified.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// A server request Fleet must answer.
    Request {
        /// The server's id, kept verbatim: correlate by direction, never by id alone.
        id: Value,
        /// The method name.
        method: String,
        /// The params, or `Value::Null` when the frame carried none.
        params: Value,
    },
    /// A server notification.
    Notification {
        /// The method name.
        method: String,
        /// The params, or `Value::Null` when the frame carried none.
        params: Value,
        /// The server's own emission clock, in milliseconds.
        ///
        /// A **sibling** of `params`, not a field inside it. It is the only server-side clock
        /// Fleet gets and what makes ordering measurable across a remote link, so it is kept.
        emitted_at_ms: Option<i64>,
    },
    /// A response to one of Fleet's own requests.
    Response {
        /// The id Fleet sent.
        id: Value,
        /// The result payload.
        result: Value,
    },
    /// An error response to one of Fleet's own requests.
    Error {
        /// The id Fleet sent.
        id: Value,
        /// The error code.
        code: i64,
        /// The server's own message.
        message: String,
    },
}

/// Whether a JSON value is a usable envelope id: a string or a number, never null.
#[must_use]
pub fn is_id(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(_) | Value::Number(_)))
}

/// The correlation key for an id, so `1` and `"1"` are the same pending request.
#[must_use]
pub fn correlation_key(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Classifies one inbound line, or answers `None` when it is not an envelope at all.
///
/// An unroutable line is a **counted warning, not a session kill**: t3code terminates the
/// connection on one, and that is wrong when the protocol shares stdout with anything that might
/// ever write a stray line.
#[must_use]
pub fn classify(value: Value) -> Option<Inbound> {
    let object = value.as_object()?;
    let method = object.get("method").and_then(Value::as_str);
    let id = object.get("id");
    if let Some(method) = method {
        if is_id(id) {
            return Some(Inbound::Request {
                id: id.cloned().unwrap_or(Value::Null),
                method: method.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        // `{"method":…}` with no `id` key at all is a notification; `"id":null` is not.
        if !object.contains_key("id") {
            return Some(Inbound::Notification {
                method: method.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
                emitted_at_ms: object.get("emittedAtMs").and_then(Value::as_i64),
            });
        }
        return None;
    }
    if !is_id(id) {
        return None;
    }
    let id = id.cloned().unwrap_or(Value::Null);
    if let Some(error) = object.get("error") {
        return Some(Inbound::Error {
            id,
            code: error
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    object.get("result").map(|result| Inbound::Response {
        id,
        result: result.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte-exact golden for the two frames Fleet writes most: no `jsonrpc`, and `params`
    /// omitted rather than nulled.
    #[test]
    fn outbound_frames_carry_no_jsonrpc_field_and_omit_absent_params() {
        let notification = OutboundNotification {
            method: "initialized",
            params: None,
        };
        assert_eq!(
            serde_json::to_string(&notification).unwrap_or_else(|error| panic!("{error}")),
            r#"{"method":"initialized"}"#
        );
        let request = OutboundRequest {
            id: 1,
            method: "thread/unsubscribe",
            params: Some(serde_json::json!({"threadId": "t"})),
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap_or_else(|error| panic!("{error}")),
            r#"{"id":1,"method":"thread/unsubscribe","params":{"threadId":"t"}}"#
        );
    }

    #[test]
    fn classification_is_structural_and_id_null_is_not_a_notification() {
        let request = classify(serde_json::json!({
            "id": "abc", "method": "item/fileChange/requestApproval", "params": {"itemId": "i"}
        }));
        assert!(matches!(request, Some(Inbound::Request { .. })));

        let notification = classify(serde_json::json!({
            "method": "turn/started", "params": {}, "emittedAtMs": 17
        }));
        assert!(matches!(
            notification,
            Some(Inbound::Notification {
                emitted_at_ms: Some(17),
                ..
            })
        ));

        // `"id": null` is neither a request nor a notification.
        assert_eq!(
            classify(serde_json::json!({"method": "turn/started", "id": null})),
            None
        );

        let response = classify(serde_json::json!({"id": 2, "result": {"ok": true}}));
        assert!(matches!(response, Some(Inbound::Response { .. })));
        let error = classify(serde_json::json!({
            "id": 2, "error": {"code": -32601, "message": "no"}
        }));
        assert!(matches!(error, Some(Inbound::Error { code: -32601, .. })));
        // A stray line is unroutable, not fatal.
        assert_eq!(classify(serde_json::json!({"hello": "world"})), None);
        assert_eq!(classify(serde_json::json!("plain")), None);
    }

    #[test]
    fn a_notification_with_no_params_key_still_classifies() {
        let value = classify(serde_json::json!({"method": "skills/changed"}));
        assert!(matches!(
            value,
            Some(Inbound::Notification {
                params: Value::Null,
                ..
            })
        ));
    }

    #[test]
    fn correlation_ignores_the_ids_json_type() {
        assert_eq!(
            correlation_key(&serde_json::json!(1)),
            correlation_key(&serde_json::json!("1"))
        );
    }
}
