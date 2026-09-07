//! Persisted state schemas and their domain-level invariants.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ids::{ContextId, RepoId},
    model::{CloneJob, Context, Repo, Worktree},
};

/// The supported persisted state schema version.
pub const STATE_VERSION: u32 = 1;

/// Fleet's complete persisted state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Persisted schema version, always one.
    pub version: u32,
    /// Registered contexts.
    pub contexts: Vec<Context>,
    /// Registered pristine repositories.
    pub repos: Vec<Repo>,
    /// Detached clone jobs awaiting reconciliation.
    #[serde(default)]
    pub clones: Vec<CloneJob>,
    /// Published worktrees.
    pub worktrees: Vec<Worktree>,
    /// Currently selected context, omitted when none exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_context_id: Option<ContextId>,
}

impl State {
    /// Validates this state's schema version, uniqueness, and references.
    pub fn validate(&self) -> Result<(), StateValidationError> {
        validate_state(self)
    }
}

/// A state schema or referential-integrity violation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StateValidationError {
    /// The state schema version is not supported.
    #[error("state version must be 1, got {0}")]
    UnsupportedVersion(u32),
    /// An entity identifier occurs more than once in its collection.
    #[error("duplicate {entity} id `{id}`")]
    DuplicateId {
        /// Entity collection containing the duplicate.
        entity: &'static str,
        /// Duplicate identifier.
        id: String,
    },
    /// An entity references an identifier absent from its parent collection.
    #[error("{entity} `{id}` references missing {target} `{target_id}`")]
    MissingReference {
        /// Referring entity type.
        entity: &'static str,
        /// Referring entity identifier.
        id: String,
        /// Referenced entity type.
        target: &'static str,
        /// Missing referenced identifier.
        target_id: String,
    },
    /// A clone job conflicts with an already registered repository.
    #[error("clone `{0}` conflicts with an existing repository")]
    CloneRepoConflict(RepoId),
}

/// Returns the exact empty version-one state.
#[must_use]
pub fn default_state() -> State {
    State {
        version: STATE_VERSION,
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context_id: None,
    }
}

/// Validates duplicate identifiers and every persisted reference.
pub fn validate_state(state: &State) -> Result<(), StateValidationError> {
    if state.version != STATE_VERSION {
        return Err(StateValidationError::UnsupportedVersion(state.version));
    }

    let contexts = collect_unique("context", state.contexts.iter().map(|context| &context.id))?;
    let repos = collect_unique("repository", state.repos.iter().map(|repo| &repo.id))?;
    collect_unique("clone", state.clones.iter().map(|clone| &clone.id))?;
    collect_unique(
        "worktree",
        state.worktrees.iter().map(|worktree| &worktree.id),
    )?;

    if let Some(active) = &state.active_context_id
        && !contexts.contains(active)
    {
        return Err(missing("state", "activeContextId", "context", active));
    }
    for repo in &state.repos {
        if !contexts.contains(&repo.context_id) {
            return Err(missing("repository", &repo.id, "context", &repo.context_id));
        }
    }
    for clone in &state.clones {
        if !contexts.contains(&clone.context_id) {
            return Err(missing("clone", &clone.id, "context", &clone.context_id));
        }
        if repos.contains(&clone.id) {
            return Err(StateValidationError::CloneRepoConflict(clone.id.clone()));
        }
    }
    for worktree in &state.worktrees {
        if !repos.contains(&worktree.repo_id) {
            return Err(missing(
                "worktree",
                &worktree.id,
                "repository",
                &worktree.repo_id,
            ));
        }
    }
    Ok(())
}

fn collect_unique<'a, T>(
    entity: &'static str,
    values: impl Iterator<Item = &'a T>,
) -> Result<HashSet<&'a T>, StateValidationError>
where
    T: 'a + Eq + std::hash::Hash + ToString,
{
    let mut ids = HashSet::new();
    for id in values {
        if !ids.insert(id) {
            return Err(StateValidationError::DuplicateId {
                entity,
                id: id.to_string(),
            });
        }
    }
    Ok(ids)
}

fn missing(
    entity: &'static str,
    id: impl ToString,
    target: &'static str,
    target_id: impl ToString,
) -> StateValidationError {
    StateValidationError::MissingReference {
        entity,
        id: id.to_string(),
        target,
        target_id: target_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn default_json_is_exact() {
        let actual =
            serde_json::to_value(default_state()).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            actual,
            json!({"version":1,"contexts":[],"repos":[],"clones":[],"worktrees":[]})
        );
    }

    #[test]
    fn missing_clones_default_to_empty() {
        let state: State =
            serde_json::from_value(json!({"version":1,"contexts":[],"repos":[],"worktrees":[]}))
                .unwrap_or_else(|error| panic!("{error}"));
        assert!(state.clones.is_empty());
    }

    #[test]
    fn rejects_duplicate_contexts() {
        let id = ContextId::try_from("one").unwrap_or_else(|error| panic!("{error}"));
        let context = Context {
            id,
            name: "One".to_owned(),
            owners: vec![],
            created_at: "2026-01-01T00:00:00Z".to_owned(),
        };
        let mut state = default_state();
        state.contexts = vec![context.clone(), context];
        assert!(matches!(
            validate_state(&state),
            Err(StateValidationError::DuplicateId {
                entity: "context",
                ..
            })
        ));
    }
}
