//! Provider start and user-input contracts shared by daemon adapters and clients.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{AgentKind, ModelSelection, PermissionMode, ThreadId};

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
    /// A URL accepted by providers such as OpenCode.
    Url(String),
    /// Base64-encoded bytes accepted by Claude stream-json.
    Base64(String),
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
}
