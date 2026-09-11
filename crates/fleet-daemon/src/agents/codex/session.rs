//! Codex session, turn, item and gate state, and the id derivations that make replay idempotent.
//!
//! Codex names its own threads, turns and items, and Fleet's identities are UUIDs. Turn ids are
//! UUIDv7 and parse directly; item ids (`exec-de76e8df-…`, `msg_0c574b…`) and JSON-RPC request
//! ids do not, so they are **derived deterministically** with a UUIDv5 over the provider's own
//! string. Deterministic derivation is what makes a replay or a reconnect re-derive the same
//! identity instead of duplicating a row (spec A.7.5).

use std::collections::{BTreeSet, HashMap};

use fleet_core::agents::{
    AgentEvent, ApprovalPolicy, ControlCost, GateAnswer, GateId, HarnessCapabilities,
    InterruptSupport, ItemId, ItemStatus, LifecycleKind, ModelSelection, PermissionMode,
    ReasoningChannels, ResumeSupport, SandboxAxes, SandboxPolicy, SteerSupport, TurnId,
    should_apply_lifecycle,
};
use semver::Version;
use serde_json::Value;
use uuid::Uuid;

use super::routing::Subagents;
use crate::agents::harness::{HarnessError, HarnessResult};

/// The namespace every derived Codex identity hangs off.
///
/// A fixed UUID, so the derivation is stable across daemon restarts, hosts and Fleet versions.
const NAMESPACE: Uuid = Uuid::from_u128(0x0fee_7cd0_0000_5000_8000_c0de_0000_0001);

/// The Fleet `ItemId` for one Codex item, derived from the thread and item ids.
#[must_use]
pub fn item_id(thread: &str, item: &str) -> ItemId {
    ItemId::from_uuid(Uuid::new_v5(
        &NAMESPACE,
        format!("codex:item:{thread}:{item}").as_bytes(),
    ))
}

/// The Fleet `TurnId` for one Codex turn: its own UUIDv7 when it parses, else derived.
#[must_use]
pub fn turn_id(thread: &str, turn: &str) -> TurnId {
    match Uuid::parse_str(turn) {
        Ok(parsed) => TurnId::from_uuid(parsed),
        Err(_) => TurnId::from_uuid(Uuid::new_v5(
            &NAMESPACE,
            format!("codex:turn:{thread}:{turn}").as_bytes(),
        )),
    }
}

/// The Fleet `GateId` for one server request.
///
/// **The key is the JSON-RPC request id**, never `approvalId ?? itemId`: `approvalId` may be null
/// and several approvals can share one `itemId` for zsh-exec-bridge subcommands, so keying on
/// those collides and answers the wrong card.
#[must_use]
pub fn gate_id(thread: &str, request_id: &Value) -> GateId {
    let key = super::envelope::correlation_key(request_id);
    GateId::from_uuid(Uuid::new_v5(
        &NAMESPACE,
        format!("codex:gate:{thread}:{key}").as_bytes(),
    ))
}

/// The Fleet `GateId` for an async question, which has no request behind it.
///
/// Deterministic — `codex-async:<thread>:<item>` — so a replay or a reconnect re-derives the same
/// gate rather than duplicating it.
#[must_use]
pub fn async_gate_id(thread: &str, item: &str) -> GateId {
    GateId::from_uuid(Uuid::new_v5(
        &NAMESPACE,
        format!("codex-async:{thread}:{item}").as_bytes(),
    ))
}

/// What kind of answer a pending server request takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ApprovalShape {
    /// `item/commandExecution/requestApproval`.
    CommandExecution,
    /// `item/fileChange/requestApproval`.
    FileChange,
    /// `item/tool/requestUserInput`.
    Questions {
        /// Question ids, in the order the card shows them.
        ids: Vec<String>,
        /// Whether the turn is parked on the answer.
        blocking: bool,
    },
    /// `item/permissions/requestApproval`.
    Permissions {
        /// The profile the agent asked for, echoed back on a grant.
        profile: Value,
    },
    /// `mcpServer/elicitation/request`.
    Elicitation,
    /// An async `agentMessage.questions` block: no request is pending and the answer is a message.
    AsyncQuestions {
        /// Question titles, in order.
        titles: Vec<String>,
    },
}

/// One server request awaiting a Fleet answer.
#[derive(Debug, Clone)]
pub(super) struct PendingApproval {
    /// The server's own request id, echoed back verbatim. Empty for an async question.
    pub(super) request_id: Value,
    /// The thread the request belongs to, which may be a subagent.
    pub(super) thread: String,
    /// The item the card joins onto.
    pub(super) item: Option<String>,
    /// What the answer looks like on the wire.
    pub(super) shape: ApprovalShape,
    /// The decisions Codex itself offered, in Codex's order.
    pub(super) decisions: Vec<String>,
}

/// The runtime controls Codex re-asserts on every turn.
///
/// **Every optional override is sticky** — the schema says "for this turn *and subsequent
/// turns*" — so Fleet re-sends the approval policy, the reviewer and the sandbox on every turn.
/// It is the only way to keep them honest after a mode switch, and `approvalsReviewer` in
/// particular: omitting it leaves `auto_review` sticky after the user switched out of auto mode,
/// which is a security-relevant stickiness rather than a cosmetic one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TurnControls {
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) approval_policy: ApprovalPolicy,
    pub(super) sandbox: SandboxPolicy,
    pub(super) reviewer: &'static str,
    pub(super) permission_profile: Option<String>,
}

impl Default for TurnControls {
    fn default() -> Self {
        Self {
            model: None,
            effort: None,
            approval_policy: ApprovalPolicy::default(),
            sandbox: SandboxPolicy::default(),
            reviewer: "user",
            permission_profile: None,
        }
    }
}

impl TurnControls {
    /// The controls one Fleet access mode selects.
    ///
    /// Fleet's four-mode ladder is a **presentation** over three orthogonal Codex axes —
    /// approval policy, sandbox policy, permission profile — not a replacement for them, and
    /// "Auto" differs from "Auto-accept edits" *only* by the reviewer: `auto_review` is Codex
    /// running its own risk-assessing subagent in the user's place, which has no Claude
    /// equivalent.
    pub(super) fn from_mode(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Ask | PermissionMode::Plan => Self {
                approval_policy: ApprovalPolicy::Untrusted,
                sandbox: SandboxPolicy::ReadOnly,
                ..Self::default()
            },
            PermissionMode::AcceptEdits => Self {
                approval_policy: ApprovalPolicy::OnRequest,
                sandbox: SandboxPolicy::WorkspaceWrite,
                ..Self::default()
            },
            PermissionMode::FullAccess => Self {
                approval_policy: ApprovalPolicy::Never,
                sandbox: SandboxPolicy::DangerFullAccess,
                ..Self::default()
            },
        }
    }

    /// The wire spelling of the approval policy.
    pub(super) fn approval_policy_wire(&self) -> Value {
        match &self.approval_policy {
            ApprovalPolicy::Untrusted => Value::String("untrusted".to_owned()),
            ApprovalPolicy::OnRequest => Value::String("on-request".to_owned()),
            ApprovalPolicy::Never => Value::String("never".to_owned()),
            // The `granular` object is the extension seam if the four-mode ladder proves too
            // coarse; Fleet does not select it, but a thread configured with one is echoed back
            // rather than silently widened.
            ApprovalPolicy::Granular(switches) => serde_json::json!({"granular": switches}),
        }
    }

    /// The **thread-level** sandbox, which is a string enum.
    pub(super) const fn sandbox_wire(&self) -> &'static str {
        match self.sandbox {
            SandboxPolicy::ReadOnly => "read-only",
            SandboxPolicy::WorkspaceWrite => "workspace-write",
            SandboxPolicy::DangerFullAccess => "danger-full-access",
        }
    }

    /// The **turn-level** sandbox policy, which is a tagged union.
    ///
    /// Two encodings for one concept, and mixing them is a decode failure.
    pub(super) fn sandbox_policy_wire(&self) -> Value {
        let kind = match self.sandbox {
            SandboxPolicy::ReadOnly => "readOnly",
            SandboxPolicy::WorkspaceWrite => "workspaceWrite",
            SandboxPolicy::DangerFullAccess => "dangerFullAccess",
        };
        serde_json::json!({"type": kind})
    }
}

/// Everything one Codex thread knows about its own session.
#[derive(Debug, Default)]
pub(super) struct CodexSession {
    /// The Codex thread id, which is the resume cursor.
    pub(super) root: Option<String>,
    /// The `userAgent` the handshake returned, and the version scraped out of it.
    pub(super) user_agent: Option<String>,
    pub(super) version: Option<Version>,
    /// The resolved config root Codex reported, so Fleet never guesses it.
    pub(super) codex_home: Option<String>,
    /// Whether the server accepted `optOutNotificationMethods`.
    pub(super) suppression_accepted: bool,
    /// Set once a legacy v1 approval request arrives, which is a refusal.
    pub(super) legacy_approval_seen: bool,
    /// Whether any approval has published `availableDecisions`.
    pub(super) available_decisions_seen: bool,
    /// The live turn, in both id spaces.
    pub(super) active_turn: Option<TurnId>,
    pub(super) active_provider_turn: Option<String>,
    pub(super) pending_start: Option<TurnId>,
    pub(super) last_turn: Option<TurnId>,
    /// Provider item id to Fleet item id, for the items this turn is still touching.
    pub(super) items: HashMap<String, ItemId>,
    /// Items that have not settled yet, and the turn that owns them.
    pub(super) open_items: HashMap<ItemId, (TurnId, ItemStatus)>,
    /// Reasoning summary parts already opened, so a `summaryPartAdded` is a boundary and not a
    /// duplicate.
    pub(super) reasoning_parts: HashMap<ItemId, u32>,
    /// Pending server requests, keyed by the JSON-RPC request id.
    pub(super) gates: HashMap<GateId, PendingApproval>,
    /// The subagent registry and the live turns Stop has to reach.
    pub(super) subagents: Subagents,
    /// The controls re-sent on every turn.
    pub(super) controls: TurnControls,
    /// The context-window denominator, which Codex pushes live.
    pub(super) context_window: Option<u64>,
    /// The optimistic user item of each submitted turn, so the echoed `userMessage` reconciles
    /// against it instead of appending a duplicate of the user's own bubble.
    pub(super) user_items: HashMap<TurnId, ItemId>,
    /// `clientUserMessageId` to item, which is the reconciliation key Codex echoes back.
    pub(super) client_items: HashMap<String, ItemId>,
    /// Codex turn id to the `TurnId` Fleet minted for that submission.
    ///
    /// Codex names its own turns, and Fleet's caller has already minted an id for the turn it is
    /// sending: the alias keeps one identity for the turn across both id spaces, so a settlement
    /// arriving under Codex's name still settles the turn the caller is waiting on.
    pub(super) turn_aliases: HashMap<String, TurnId>,
}

impl CodexSession {
    /// The capabilities of this process.
    ///
    /// Codex declares **nothing**, so this is the parsed `userAgent` semver plus behaviour: what
    /// Fleet knows it can do because it has seen the protocol do it.
    pub(super) fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            version: self
                .version
                .clone()
                .unwrap_or_else(|| Version::new(0, 0, 0)),
            resume: ResumeSupport::ByCursor { fork: true },
            // `turn/steer` takes `expectedTurnId`, a compare-and-swap against the active turn, so
            // the queue-or-steer question is answerable by the harness rather than guessed.
            steer: SteerSupport::Explicit {
                compare_and_swap: true,
            },
            // `turn/interrupt` is a plain RPC whose return is a receipt, not a settlement, and
            // there is nothing to cancel queued input with: a queued turn is a separate turn id
            // and **survives** Stop.
            interrupt: InterruptSupport::Receipted {
                cancel_queued: false,
            },
            history_readback: true,
            native_compaction: true,
            turn_diff: true,
            attention_flags: true,
            async_questions: true,
            secret_answers: true,
            sandbox_axes: SandboxAxes::Three,
            reasoning_channels: ReasoningChannels::Two,
            live_context_meter: true,
            // Model, effort and mode are per-turn parameters: no restart, ever.
            model_switch: ControlCost::InPlace,
            effort_switch: ControlCost::InPlace,
            mode_switch: ControlCost::InPlace,
            declared: BTreeSet::new(),
        }
    }

    /// The Fleet turn id for a provider turn id.
    ///
    /// An alias recorded at submit time wins, so the caller's own id survives; anything Fleet did
    /// not submit — a queued turn, a replayed one — is derived deterministically instead.
    pub(super) fn turn_for(&self, provider_turn: &str) -> TurnId {
        self.turn_aliases
            .get(provider_turn)
            .copied()
            .unwrap_or_else(|| turn_id(self.root.as_deref().unwrap_or_default(), provider_turn))
    }

    /// Records that `provider_turn` is the turn the caller minted `turn` for.
    pub(super) fn alias_turn(&mut self, provider_turn: &str, turn: TurnId) {
        self.turn_aliases.insert(provider_turn.to_owned(), turn);
    }

    /// The Fleet item id for a provider item id, remembering the mapping.
    pub(super) fn item_for(&mut self, thread: &str, provider_item: &str) -> ItemId {
        let item = item_id(thread, provider_item);
        self.items.insert(provider_item.to_owned(), item);
        item
    }

    /// Records the optimistic user item of a turn Fleet is submitting.
    ///
    /// `clientUserMessageId` is **always** set by Fleet and comes back as `clientId` on the
    /// echoed `userMessage` item: it is the reconciliation key that stops Fleet appending a
    /// duplicate of the user's own bubble.
    pub(super) fn remember_user_item(&mut self, turn: TurnId, item: ItemId) {
        self.user_items.insert(turn, item);
        self.client_items.insert(item.to_string(), item);
    }

    /// The optimistic user item of a provider turn, if Fleet submitted it.
    pub(super) fn user_item_for(&self, provider_turn: &str) -> Option<ItemId> {
        self.user_items.get(&self.turn_for(provider_turn)).copied()
    }

    /// The item a `clientId` names, if Fleet minted it.
    pub(super) fn item_for_client(&self, client_id: &str) -> Option<ItemId> {
        self.client_items.get(client_id).copied()
    }

    /// Opens a turn Fleet is about to submit.
    pub(super) fn begin_turn(&mut self, turn: TurnId) {
        self.pending_start = Some(turn);
        self.last_turn = Some(turn);
    }

    /// Adopts the turn the harness confirmed.
    ///
    /// **`active_turn` is never overwritten while a turn is running:** `turn/start` during an
    /// active turn returns a *queued* turn id, and `turn/interrupt` only accepts the id that is
    /// active now. The first time a user types a second message and then hits Stop, the naive
    /// implementation interrupts nothing.
    pub(super) fn adopt_turn(&mut self, turn: TurnId, provider_turn: &str) {
        if self.active_turn.is_none() {
            self.active_turn = Some(turn);
            self.active_provider_turn = Some(provider_turn.to_owned());
        }
        self.last_turn = Some(turn);
        if self.pending_start == Some(turn) {
            self.pending_start = None;
        }
    }

    /// Whether a lifecycle event naming `turn` may move the state.
    pub(super) fn may_apply(&self, kind: LifecycleKind, turn: Option<TurnId>) -> bool {
        should_apply_lifecycle(kind, turn, self.active_turn, self.pending_start)
    }

    /// Clears the live turn after a settlement.
    pub(super) fn settle_turn(&mut self, turn: TurnId) {
        if self.active_turn == Some(turn) {
            self.active_turn = None;
            self.active_provider_turn = None;
        }
        if self.pending_start == Some(turn) {
            self.pending_start = None;
        }
        self.reasoning_parts.clear();
    }

    /// Registers a pending gate and answers its stable id.
    pub(super) fn open_gate(&mut self, gate: GateId, pending: PendingApproval) {
        self.gates.insert(gate, pending);
    }

    /// Takes a pending gate, or reports that it is gone.
    pub(super) fn take_gate(&mut self, gate: GateId) -> HarnessResult<PendingApproval> {
        self.gates
            .remove(&gate)
            .ok_or(HarnessError::GateGone { gate })
    }

    /// Closes every open gate, for a session that can no longer answer them.
    pub(super) fn drain_gates(&mut self) -> Vec<(GateId, PendingApproval)> {
        self.gates.drain().collect()
    }

    /// Records an item as open, so a settlement can close it.
    pub(super) fn note_open_item(&mut self, item: ItemId, turn: TurnId) {
        self.open_items.insert(item, (turn, ItemStatus::InProgress));
    }

    /// Closes an item once.
    pub(super) fn close_item(
        &mut self,
        item: ItemId,
        status: ItemStatus,
        events: &mut Vec<AgentEvent>,
    ) {
        if let Some((_, current)) = self.open_items.get_mut(&item) {
            if matches!(
                current,
                ItemStatus::Completed
                    | ItemStatus::Failed
                    | ItemStatus::Denied
                    | ItemStatus::Stopped
            ) {
                return;
            }
            *current = status;
            events.push(AgentEvent::ItemCompleted { item, status });
        }
    }

    /// Closes every item a settling turn still owns, before the settlement is applied.
    pub(super) fn close_turn_items(
        &mut self,
        turn: TurnId,
        status: ItemStatus,
        events: &mut Vec<AgentEvent>,
    ) {
        let open = self
            .open_items
            .iter()
            .filter(|(_, (owner, state))| *owner == turn && matches!(state, ItemStatus::InProgress))
            .map(|(item, _)| *item)
            .collect::<Vec<_>>();
        for item in open {
            self.close_item(item, status, events);
        }
        self.open_items.retain(|_, (owner, _)| *owner != turn);
        self.items.clear();
    }

    /// The answer a gate gets when nobody can answer it any more.
    pub(super) fn closed_answer(shape: &ApprovalShape) -> GateAnswer {
        match shape {
            ApprovalShape::Questions { ids, .. } => GateAnswer::Question {
                answers: vec![Vec::new(); ids.len()],
            },
            ApprovalShape::AsyncQuestions { titles } => GateAnswer::Question {
                answers: vec![Vec::new(); titles.len()],
            },
            _ => GateAnswer::Permission {
                choice: fleet_core::agents::PermissionChoice::Deny,
                edited_payload: None,
            },
        }
    }

    /// The model selection Codex reported, if any.
    pub(super) fn model_selection(&self) -> Option<ModelSelection> {
        self.controls.model.as_ref().map(|model| ModelSelection {
            model: model.clone(),
            effort: self.controls.effort.clone(),
            provider: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_identities_are_stable_and_distinct() {
        let first = item_id("thread-a", "exec-1");
        assert_eq!(
            first,
            item_id("thread-a", "exec-1"),
            "replay must re-derive"
        );
        assert_ne!(first, item_id("thread-a", "exec-2"));
        assert_ne!(first, item_id("thread-b", "exec-1"));

        // A UUIDv7 turn id is adopted verbatim rather than re-derived.
        let seven = "01a089f2-579b-75c2-8e51-3492ec617046";
        assert_eq!(turn_id("thread-a", seven).to_string(), seven);
        // A non-UUID turn id still yields a stable identity.
        assert_eq!(turn_id("t", "turn-1"), turn_id("t", "turn-1"));
    }

    #[test]
    fn a_gate_is_keyed_on_the_request_id_and_not_on_the_item() {
        let numeric = gate_id("t", &serde_json::json!(7));
        // `1` and `"1"` are the same pending request, so they are the same gate.
        assert_eq!(numeric, gate_id("t", &serde_json::json!("7")));
        // Two approvals sharing one item id are two gates.
        assert_ne!(numeric, gate_id("t", &serde_json::json!(8)));
        assert_eq!(
            async_gate_id("t", "item-1"),
            async_gate_id("t", "item-1"),
            "an async question re-derives its own gate on replay"
        );
        assert_ne!(
            async_gate_id("t", "item-1"),
            gate_id("t", &serde_json::json!("item-1"))
        );
    }

    #[test]
    fn the_active_turn_is_never_overwritten_by_a_queued_one() {
        let mut session = CodexSession::default();
        let first = TurnId::new();
        let queued = TurnId::new();
        session.begin_turn(first);
        session.adopt_turn(first, "turn-1");
        session.adopt_turn(queued, "turn-2");
        assert_eq!(session.active_turn, Some(first));
        assert_eq!(session.active_provider_turn.as_deref(), Some("turn-1"));
        session.settle_turn(first);
        session.adopt_turn(queued, "turn-2");
        assert_eq!(session.active_turn, Some(queued));
    }

    #[test]
    fn every_mode_maps_to_three_axes_and_the_two_sandbox_encodings_differ() {
        let ask = TurnControls::from_mode(PermissionMode::Ask);
        assert_eq!(ask.sandbox_wire(), "read-only");
        assert_eq!(
            ask.sandbox_policy_wire(),
            serde_json::json!({"type": "readOnly"})
        );
        assert_eq!(ask.approval_policy_wire(), serde_json::json!("untrusted"));
        let full = TurnControls::from_mode(PermissionMode::FullAccess);
        assert_eq!(full.sandbox_wire(), "danger-full-access");
        assert_eq!(full.approval_policy_wire(), serde_json::json!("never"));
        // The reviewer is re-sent every turn, and it is `user` unless Fleet's auto mode selects
        // `auto_review` explicitly.
        assert_eq!(full.reviewer, "user");
    }
}
