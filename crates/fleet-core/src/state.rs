//! Persisted state schemas and their domain-level invariants.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::{
    ids::{ContextId, RepoId},
    model::{CloneJob, Context, Repo, Worktree},
};

/// The supported persisted state schema version.
pub const STATE_VERSION: u32 = 1;

/// Recoverable archive of remote worktrees persisted by pre-federation Fleet builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRemoteRecords {
    /// Archive schema version.
    pub version: u32,
    /// Last-known records, keyed logically by host and worktree id.
    pub worktrees: Vec<Worktree>,
}

impl LegacyRemoteRecords {
    /// Creates an empty version-one archive.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            version: STATE_VERSION,
            worktrees: Vec::new(),
        }
    }

    /// Adds records without duplicating a previously archived host/worktree pair.
    pub fn merge(&mut self, records: impl IntoIterator<Item = Worktree>) {
        let mut merged = self
            .worktrees
            .drain(..)
            .map(|worktree| ((worktree.host.clone(), worktree.id.clone()), worktree))
            .collect::<BTreeMap<_, _>>();
        for worktree in records {
            merged.insert((worktree.host.clone(), worktree.id.clone()), worktree);
        }
        self.version = STATE_VERSION;
        self.worktrees = merged.into_values().collect();
    }
}

impl Default for LegacyRemoteRecords {
    fn default() -> Self {
        Self::empty()
    }
}

/// Fleet's complete persisted state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    version: u32,
    contexts: Vec<Context>,
    repos: Vec<Repo>,
    #[serde(default)]
    clones: Vec<CloneJob>,
    worktrees: Vec<Worktree>,
    active_context_id: Option<ContextId>,
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let persisted = PersistedState::deserialize(deserializer)?;
        Ok(Self::from(persisted))
    }
}

impl From<PersistedState> for State {
    fn from(mut persisted: PersistedState) -> Self {
        for repo in &mut persisted.repos {
            repo.owner = repo.id.owner().to_owned();
            repo.name = repo.id.name().to_owned();
        }
        for clone in &mut persisted.clones {
            clone.owner = clone.id.owner().to_owned();
            clone.name = clone.id.name().to_owned();
        }
        for worktree in &mut persisted.worktrees {
            if let Ok(repo_id) = RepoId::try_from(worktree.id.repo()) {
                worktree.repo_id = repo_id;
            }
            worktree.slug = worktree.id.slug().to_owned();
        }
        Self {
            version: persisted.version,
            contexts: persisted.contexts,
            repos: persisted.repos,
            clones: persisted.clones,
            worktrees: persisted.worktrees,
            active_context_id: persisted.active_context_id,
        }
    }
}

impl State {
    /// Validates this state's schema version, uniqueness, and references.
    pub fn validate(&self) -> Result<(), StateValidationError> {
        validate_state(self)
    }

    /// Removes records owned by remote hosts so their daemon mirror is authoritative.
    pub fn take_remote_worktrees(&mut self) -> Vec<Worktree> {
        let (remote, local) = std::mem::take(&mut self.worktrees)
            .into_iter()
            .partition(|worktree| worktree.host.is_some());
        self.worktrees = local;
        remote
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
    /// An entity's redundant identity fields disagree with its canonical identifier.
    #[error("{entity} `{id}` has {field} `{actual}`, expected `{expected}`")]
    MismatchedIdentity {
        /// Entity type containing the mismatch.
        entity: &'static str,
        /// Canonical entity identifier.
        id: String,
        /// Redundant field that disagrees with the identifier.
        field: &'static str,
        /// Persisted field value.
        actual: String,
        /// Value derived from the canonical identifier.
        expected: String,
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
        validate_identity(
            "repository",
            &repo.id,
            "owner",
            &repo.owner,
            repo.id.owner(),
        )?;
        validate_identity("repository", &repo.id, "name", &repo.name, repo.id.name())?;
        if !contexts.contains(&repo.context_id) {
            return Err(missing("repository", &repo.id, "context", &repo.context_id));
        }
    }
    for clone in &state.clones {
        validate_identity("clone", &clone.id, "owner", &clone.owner, clone.id.owner())?;
        validate_identity("clone", &clone.id, "name", &clone.name, clone.id.name())?;
        if !contexts.contains(&clone.context_id) {
            return Err(missing("clone", &clone.id, "context", &clone.context_id));
        }
        if repos.contains(&clone.id) {
            return Err(StateValidationError::CloneRepoConflict(clone.id.clone()));
        }
    }
    for worktree in &state.worktrees {
        validate_identity(
            "worktree",
            &worktree.id,
            "repoId",
            worktree.repo_id.as_str(),
            worktree.id.repo(),
        )?;
        validate_identity(
            "worktree",
            &worktree.id,
            "slug",
            &worktree.slug,
            worktree.id.slug(),
        )?;
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

fn validate_identity(
    entity: &'static str,
    id: impl ToString,
    field: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), StateValidationError> {
    if actual == expected {
        return Ok(());
    }
    Err(StateValidationError::MismatchedIdentity {
        entity,
        id: id.to_string(),
        field,
        actual: actual.to_owned(),
        expected: expected.to_owned(),
    })
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

    #[test]
    fn remote_worktrees_move_into_a_deduplicated_legacy_archive() {
        let remote = Worktree {
            id: "acme/api#remote".parse().unwrap(),
            repo_id: "acme/api".parse().unwrap(),
            slug: "remote".into(),
            branch: "remote".into(),
            base_ref: "origin/main".into(),
            path: "/remote/acme/api/remote".into(),
            session: "api/remote".into(),
            host: Some("dev-box".parse().unwrap()),
            created_at: "2026-09-08T00:00:00Z".into(),
            last_opened_at: None,
            degraded: None,
        };
        let mut local = remote.clone();
        local.id = "acme/api#local".parse().unwrap();
        local.slug = "local".into();
        local.path = "/local/acme/api/local".into();
        local.session = "api/local".into();
        local.host = None;
        let mut state = default_state();
        state.worktrees = vec![remote.clone(), local.clone()];

        let migrated = state.take_remote_worktrees();
        let mut archive = LegacyRemoteRecords::empty();
        archive.merge(migrated.clone());
        archive.merge(migrated);

        assert_eq!(state.worktrees, vec![local]);
        assert_eq!(archive.worktrees, vec![remote]);
    }
}
