//! Pure board validation, ordering, and mutation contracts.

use super::{
    model::{
        Activity, ActivityKind, BackendRef, Board, BoardSettings, BoardSummary, Card, Comment,
        Label, Priority, Status, StatusCategory,
    },
    property::{PropertySchema, PropertyValue},
};
use crate::ids::{CardId, LabelId, RepoId, StatusId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

mod cards;
mod patches;
mod query;
#[cfg(test)]
mod tests;
mod validation;

pub use cards::{add_comment, create_card, move_card, push_activity};
pub use patches::{apply_board_patch, apply_card_patch, merge_settings};
pub use query::{column_cards, first_status_in, summarize, worktree_slug};
pub use validation::{check_draft_writable, valid_date, validate_board, validate_card};

use cards::dedupe_labels;
use validation::{check_editable, check_writable};

/// Values used to create a new card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardDraft {
    /// Title.
    pub title: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Status id.
    #[serde(default)]
    pub status_id: Option<StatusId>,
    /// Priority.
    #[serde(default)]
    pub priority: Priority,
    /// Labels.
    #[serde(default)]
    pub labels: Vec<LabelId>,
    /// Assignee.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Estimate.
    #[serde(default)]
    pub estimate: Option<u32>,
    /// Due date.
    #[serde(default)]
    pub due_date: Option<String>,
    /// Parent id.
    #[serde(default)]
    pub parent_id: Option<CardId>,
    /// Repo id.
    #[serde(default)]
    pub repo_id: Option<RepoId>,
    /// Properties.
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
}

/// `None` = leave unchanged; `Some(None)` = clear. All fields optional.
/// Optional changes; nested options distinguish clearing from omission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardPatch {
    /// Title.
    pub title: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Status id.
    pub status_id: Option<StatusId>,
    /// Priority.
    pub priority: Option<Priority>,
    /// Labels.
    pub labels: Option<Vec<LabelId>>,
    /// Assignee.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee: Option<Option<String>>,
    /// Estimate.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub estimate: Option<Option<u32>>,
    /// Due date.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_date: Option<Option<String>>,
    /// Parent id.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Option<Option<CardId>>,
    /// Repo id.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub repo_id: Option<Option<RepoId>>,
    /// Properties.
    pub properties: Option<BTreeMap<String, PropertyValue>>, /* merge; Null removes */
    /// Archived.
    pub archived: Option<bool>,
}
impl CardPatch {
    /// Whether every patch field is absent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Optional board configuration changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoardPatch {
    /// Name.
    pub name: Option<String>,
    /// Prefix.
    pub prefix: Option<String>,
    /// Backend.
    pub backend: Option<BackendRef>,
    /// Statuses.
    pub statuses: Option<Vec<Status>>,
    /// Labels.
    pub labels: Option<Vec<Label>>,
    /// Properties.
    pub properties: Option<Vec<PropertySchema>>,
    /// Default repo id.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_repo_id: Option<Option<RepoId>>,
    /// Settings.
    pub settings: Option<BoardSettings>,
}
impl BoardPatch {
    /// Whether every patch field is absent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Validation, lookup, or backend failure in a board operation.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BoardError {
    /// Board not found.
    #[error("board not found: {0}")]
    BoardNotFound(String),
    /// Card not found.
    #[error("card not found: {0}")]
    CardNotFound(String),
    /// Unknown status.
    #[error("unknown status: {0}")]
    UnknownStatus(String),
    /// Unknown label.
    #[error("unknown label: {0}")]
    UnknownLabel(String),
    /// A named input field violates its validation rules.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// Rejected field name.
        field: String,
        /// Explanation of the violated rule.
        reason: String,
    },
    /// The context already owns a board.
    #[error("board already exists for context {0}")]
    Duplicate(String),
    /// Unknown backend.
    #[error("backend `{0}` is not registered")]
    UnknownBackend(String),
    /// Unsupported.
    #[error("backend does not support {0}")]
    Unsupported(&'static str),
    /// Backend.
    #[error("backend error: {0}")]
    Backend(String),
    /// Conflicted.
    #[error("card {0} has an unresolved conflict")]
    Conflicted(String),
    /// A standard card field the board's backend declared it cannot write back.
    #[error("{0} is read-only on this board's backend")]
    ReadOnlyField(String),
}

fn nested_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn invalid(field: &str, reason: &str) -> BoardError {
    BoardError::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

/// Serde default for enabled settings.
#[must_use]
pub fn default_true() -> bool {
    true
}
/// Default worktree branch naming template.
#[must_use]
pub fn default_branch_template() -> String {
    "{key}-{slug}".into()
}
