//! Provider start and user-input contracts shared by daemon adapters and clients.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::{AgentKind, ItemId, ModelSelection, PermissionMode, ThreadId};

/// One user-supplied non-text input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// Optional display filename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// MIME type sent to the provider.
    pub media_type: String,
    /// Provider-neutral attachment location or contents.
    pub source: AttachmentSource,
}

/// Where attachment bytes are obtained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AttachmentSource {
    /// A local file path, resolved by the daemon.
    Path(PathBuf),
    /// A URL accepted by providers such as Codex.
    Url(String),
    /// Base64-encoded bytes accepted by Claude stream-json.
    Base64(String),
}

/// Codex sandbox policy selected for a new or resumed thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxPolicy {
    /// The harness may read but not modify the worktree.
    ReadOnly,
    /// The harness may modify the selected workspace roots.
    #[default]
    WorkspaceWrite,
    /// The harness runs without filesystem sandboxing.
    DangerFullAccess,
}

/// Codex approval policy selected for a new or resumed thread.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    /// Only commands already trusted by policy run without asking.
    #[default]
    Untrusted,
    /// The harness requests approval when it determines one is needed.
    OnRequest,
    /// The harness never opens an approval request.
    Never,
    /// Additive granular switches supplied by a newer Codex build.
    Granular(BTreeMap<String, bool>),
}

/// Everything required to start or resume a provider process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    /// Fleet thread identity.
    pub thread: ThreadId,
    /// Canonical worktree directory.
    pub worktree_path: PathBuf,
    /// Provider implementation to launch.
    pub provider: AgentKind,
    /// Optional initial model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    /// Initial permission policy.
    pub mode: PermissionMode,
    /// Provider-native session cursor to resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cursor: Option<String>,
    /// Fork the resumed cursor rather than continuing it in place.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fork: bool,
    /// Explicit child-process environment additions or overrides.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Codex filesystem sandbox policy.
    #[serde(default)]
    pub sandbox: SandboxPolicy,
    /// Codex approval policy, independent from the sandbox policy.
    #[serde(default)]
    pub approval_policy: ApprovalPolicy,
    /// Optional Codex permission-profile identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<String>,
    /// Optional thread title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A user message submitted to a provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInput {
    /// Message text.
    pub text: String,
    /// Ordered image, file, agent, or resource attachments.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// The identity the client already drew this message under, when it drew one.
    ///
    /// The app paints an optimistic bubble the instant the user presses `⏎` and has to recognise
    /// the daemon's own copy of it when the projection catches up. Without this field the join is
    /// the message text plus an echo count, which mis-joins the moment the same text is sent
    /// twice; with it the adapter adopts the client's id and the row keeps one identity from the
    /// keystroke to the transcript. Absent from an older client, in which case the daemon mints
    /// one exactly as it did before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemId>,
}
