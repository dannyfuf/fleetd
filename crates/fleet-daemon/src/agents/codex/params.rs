//! The frames Fleet writes to Codex, and only those.
//!
//! Every one is pinned by a byte-exact golden: `params` is omitted when absent, there is no
//! `jsonrpc` field, and every sticky override is re-sent on each turn because the schema says
//! each one applies "for this turn **and subsequent turns**" — re-asserting them is the only way
//! to keep a mode switch honest.

use fleet_core::agents::{ItemId, UserInput};
use serde_json::{Value, json};

use super::{CodexHarness, methods, session::TurnControls};
use crate::agents::harness::OpenSession;

impl CodexHarness {
    /// The `initialize` params, with the suppression list either declared or dropped.
    pub(super) fn initialize_params(&self, suppress: bool) -> Value {
        let mut capabilities = json!({
            // What unlocks `item/tool/requestUserInput` and the modern request set.
            "experimentalApi": true,
            "mcpServerOpenaiFormElicitation": false,
            "requestAttestation": false,
        });
        if suppress && let Some(object) = capabilities.as_object_mut() {
            object.insert(
                "optOutNotificationMethods".to_owned(),
                json!(methods::OPT_OUT_NOTIFICATION_METHODS),
            );
        }
        json!({
            "clientInfo": {
                "name": "fleet",
                "title": "Fleet",
                "version": self.config.client_version,
            },
            "capabilities": capabilities,
        })
    }

    /// The start params both `thread/start` and `thread/resume` carry.
    ///
    /// Re-asserted in full on resume, `approvalsReviewer` in particular: omitting it keeps the
    /// thread's previous reviewer, which leaves `auto_review` sticky after the user switched out
    /// of auto mode.
    pub(super) fn start_params(&self, request: &OpenSession, controls: &TurnControls) -> Value {
        let mut params = json!({
            "cwd": request.start.worktree_path.to_string_lossy(),
            "approvalPolicy": controls.approval_policy_wire(),
            "approvalsReviewer": controls.reviewer,
            "sandbox": controls.sandbox_wire(),
        });
        if let Some(object) = params.as_object_mut() {
            if let Some(model) = &request.start.model {
                object.insert("model".to_owned(), json!(model.model));
            }
            if let Some(profile) = &controls.permission_profile {
                object.insert("permissions".to_owned(), json!(profile));
            }
        }
        params
    }

    /// The `turn/steer` params: the same input, plus the compare-and-swap.
    ///
    /// The per-turn overrides belong to `turn/start`; a steer carries only what to say next.
    pub(super) fn steer_params(
        &self,
        thread: &str,
        input: &UserInput,
        user_item: ItemId,
        active_turn: &str,
    ) -> Value {
        json!({
            "threadId": thread,
            "expectedTurnId": active_turn,
            "input": user_input(input),
            "clientUserMessageId": user_item.to_string(),
        })
    }

    /// The per-turn params, with every sticky override re-sent.
    pub(super) fn turn_params(
        &self,
        thread: &str,
        input: &UserInput,
        user_item: ItemId,
        controls: &TurnControls,
    ) -> Value {
        let mut params = json!({
            "threadId": thread,
            "input": user_input(input),
            // Always set: it comes back as `clientId` on the echoed `userMessage`, and it is the
            // key that stops Fleet appending a duplicate of the user's own bubble.
            "clientUserMessageId": user_item.to_string(),
            "approvalPolicy": controls.approval_policy_wire(),
            "approvalsReviewer": controls.reviewer,
            "sandboxPolicy": controls.sandbox_policy_wire(),
        });
        if let Some(object) = params.as_object_mut() {
            if let Some(model) = &controls.model {
                object.insert("model".to_owned(), json!(model));
            }
            if let Some(effort) = &controls.effort {
                // A non-empty **string**, not an enum: the legal set is per model and comes from
                // `model/list`, so Fleet never hardcodes a ladder.
                object.insert("effort".to_owned(), json!(effort));
            }
        }
        params
    }
}

/// The `UserInput[]` one submission becomes.
///
/// `skill` and `mention` are emitted as **typed parts**, not by interpolating `@path` into prose:
/// t3code leaves both unused and pastes paths into text, which is a downgrade Fleet does not
/// inherit.
pub(super) fn user_input(input: &UserInput) -> Value {
    use fleet_core::agents::AttachmentSource;

    let mut parts = vec![json!({"type": "text", "text": input.text})];
    for attachment in &input.attachments {
        let part = match &attachment.source {
            AttachmentSource::Url(url) => json!({"type": "image", "url": url}),
            AttachmentSource::Base64(data) => json!({
                "type": "image",
                "url": format!("data:{};base64,{data}", attachment.media_type),
            }),
            AttachmentSource::Path(path) => {
                if attachment.media_type.starts_with("image/") {
                    json!({"type": "localImage", "path": path.to_string_lossy()})
                } else {
                    json!({
                        "type": "mention",
                        "name": attachment
                            .name
                            .clone()
                            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
                        "path": path.to_string_lossy(),
                    })
                }
            }
        };
        parts.push(part);
    }
    Value::Array(parts)
}

/// The `turn/interrupt` params. The RPC's return is a **receipt, not a settlement**.
pub(super) fn interrupt_params(thread: &str, turn: &str) -> Value {
    json!({"threadId": thread, "turnId": turn})
}

/// The `thread/compact/start` params.
pub(super) fn compact_params(thread: &str) -> Value {
    json!({"threadId": thread})
}

/// The `thread/settings/update` params, which apply to **subsequent** turns.
pub(super) fn settings_params(thread: &str, controls: &TurnControls) -> Value {
    let mut params = json!({
        "threadId": thread,
        "approvalPolicy": controls.approval_policy_wire(),
        "approvalsReviewer": controls.reviewer,
        "sandboxPolicy": controls.sandbox_policy_wire(),
    });
    if let Some(object) = params.as_object_mut()
        && let Some(model) = &controls.model
    {
        object.insert("model".to_owned(), json!(model));
    }
    params
}
