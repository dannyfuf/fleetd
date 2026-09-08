//! Provider-neutral human-decision gates and answers.

use serde::{Deserialize, Serialize};

use super::{GateId, Seq, ToolKind, TurnId};

/// An opaque provider-native permission option identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderOptionId(pub String);

/// Effective scope or denial represented by a permission action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionChoice {
    /// Allow this invocation only.
    AllowOnce,
    /// Allow matching calls for the current provider session.
    AllowSession,
    /// Allow matching calls for this worktree directory.
    AllowDirectory,
    /// Deny this invocation while the agent may continue.
    Deny,
    /// Let the user edit the provider payload before allowing it.
    Edit,
    /// Deny and interrupt the current turn.
    DenyAndStop,
}

/// One permission choice as displayed by Fleet and mapped by the adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    /// Provider-native reply or permission-suggestion identity.
    pub id: ProviderOptionId,
    /// Provider-neutral effective choice.
    pub label: PermissionChoice,
}

/// One selectable answer to a provider question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    /// Answer text returned to the provider.
    pub label: String,
    /// Human-readable consequence or clarification.
    pub description: String,
}

/// One provider question, including selection shape and custom-answer policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    /// Full question text; Claude uses it as the answer-map key.
    pub text: String,
    /// Short card heading.
    pub header: String,
    /// Ordered predefined choices.
    pub options: Vec<QuestionOption>,
    /// Whether more than one choice may be selected.
    #[serde(default)]
    pub multi_select: bool,
    /// Whether the user may supply free text.
    #[serde(default)]
    pub allow_other: bool,
}

/// The three human-in-the-loop gate shapes rendered in a thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum GateKind {
    /// Permission to invoke a protected tool.
    Permission {
        /// Normalized tool category.
        tool: ToolKind,
        /// Provider-supplied card title.
        title: String,
        /// Human-readable command, path, or JSON payload.
        payload: String,
        /// Optional sanitized reason for the gate.
        rationale: Option<String>,
        /// Ordered actions the provider can map exactly.
        options: Vec<PermissionOption>,
    },
    /// One to four provider questions.
    Question {
        /// Ordered questions; answers preserve this order.
        questions: Vec<Question>,
    },
    /// A completed plan awaiting approval or feedback.
    Plan {
        /// Full Markdown proposal.
        markdown: String,
        /// Short ordered step summaries.
        steps: Vec<String>,
    },
}

/// User decision for a plan gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PlanAnswer {
    /// Approve the plan and proceed to implementation.
    Approve,
    /// Return feedback while remaining in planning mode.
    AskForChanges {
        /// User-authored plan feedback.
        note: String,
    },
}

/// A normalized answer mapped back to the provider-native protocol by an adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum GateAnswer {
    /// Permission choice, optionally with an edited command or payload.
    Permission {
        /// Effective scope or denial.
        choice: PermissionChoice,
        /// Replacement provider payload for the Edit action.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edited_payload: Option<String>,
    },
    /// One string array per question, preserving question order.
    Question {
        /// Selected labels and custom text.
        answers: Vec<Vec<String>>,
    },
    /// Approval or feedback for a plan.
    Plan(PlanAnswer),
}

/// Actor that authoritatively closed a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateResolver {
    /// A person answered through a Fleet client.
    User,
    /// Fleet's selected mode answered automatically.
    Auto,
    /// The adapter's response budget expired.
    Timeout,
    /// The provider withdrew or closed the request.
    ProviderClosed,
}

/// A currently actionable gate in a thread projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenGate {
    /// Stable normalized gate identity.
    pub id: GateId,
    /// Associated turn, if the provider supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnId>,
    /// Gate presentation and mapping payload.
    pub kind: GateKind,
    /// Sequence at which the gate became actionable.
    pub opened_seq: Seq,
    /// Reducer time from which the thread has been blocked on the user for this gate.
    ///
    /// A turn parked on a gate is not a turn that is working, so the projection charges the
    /// window between this and the answer to the turn and takes it back off the footer. It is
    /// normally the moment the gate opened; a gate that outlives an earlier, overlapping one
    /// inherits that one's start, so a run of overlapping gates is one wait rather than one per
    /// gate. `None` on a gate decoded from a log written before the field existed, which costs
    /// that one gate's wait and nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_since: Option<chrono::DateTime<chrono::Utc>>,
}
