//! Context selection and lifecycle operation contracts.

use std::sync::Arc;

use fleet_core::{ids::ContextId, model::Context, slug::normalize_context_id};

use crate::{DaemonError, DaemonResult, stores::state::StateStore};

/// Context domain service backed by durable state.
#[derive(Clone)]
pub struct Contexts {
    state: Arc<StateStore>,
}

impl Contexts {
    /// Creates the context service.
    #[must_use]
    pub fn new(state: Arc<StateStore>) -> Self {
        Self { state }
    }

    /// Creates a normalized context and persists it transactionally (inventory sections 1 and 2).
    pub async fn create(&self, name: String, owners: Vec<String>) -> DaemonResult<Context> {
        let normalized = normalize_context_id(&name);
        if normalized.is_empty() {
            return Err(DaemonError::Validation(
                "context name must contain a letter or number".to_owned(),
            ));
        }
        let id = ContextId::try_from(normalized)
            .map_err(|error| DaemonError::Validation(error.to_string()))?;
        let context = Context {
            id: id.clone(),
            name,
            owners,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.state
            .transaction(move |state| {
                if state.contexts.iter().any(|existing| existing.id == id) {
                    return Err(DaemonError::Conflict(format!("context {id}")));
                }
                state.contexts.push(context.clone());
                Ok(context)
            })
            .await
    }

    /// Updates context display fields while preserving referential integrity (inventory section 1).
    pub async fn update(
        &self,
        id: ContextId,
        name: Option<String>,
        owners: Option<Vec<String>>,
    ) -> DaemonResult<Context> {
        self.state
            .transaction(move |state| {
                let context = state
                    .contexts
                    .iter_mut()
                    .find(|context| context.id == id)
                    .ok_or_else(|| DaemonError::NotFound(format!("context {id}")))?;
                if let Some(name) = name {
                    context.name = name;
                }
                if let Some(owners) = owners {
                    context.owners = owners;
                }
                Ok(context.clone())
            })
            .await
    }

    /// Deletes a context and cascades repositories, worktrees, and sessions (inventory section 1).
    pub async fn delete(&self, id: ContextId) -> DaemonResult<()> {
        self.state
            .transaction(move |state| {
                if !state.contexts.iter().any(|context| context.id == id) {
                    return Err(DaemonError::NotFound(format!("context {id}")));
                }

                let repo_ids = state
                    .repos
                    .iter()
                    .filter(|repo| repo.context_id == id)
                    .map(|repo| repo.id.clone())
                    .collect::<std::collections::HashSet<_>>();
                if state
                    .worktrees
                    .iter()
                    .any(|worktree| repo_ids.contains(&worktree.repo_id) && worktree.host.is_some())
                {
                    return Err(DaemonError::Unsupported(
                        "remote hosts are not supported yet".to_owned(),
                    ));
                }
                state
                    .worktrees
                    .retain(|worktree| !repo_ids.contains(&worktree.repo_id));
                state.repos.retain(|repo| repo.context_id != id);
                state.clones.retain(|clone| clone.context_id != id);
                state.contexts.retain(|context| context.id != id);
                if state.active_context_id.as_ref() == Some(&id) {
                    state.active_context_id = None;
                }
                Ok(())
            })
            .await
    }

    /// Changes or clears the active context in one state transaction (inventory sections 1 and 6).
    pub async fn set_active(&self, id: Option<ContextId>) -> DaemonResult<()> {
        self.state
            .transaction(move |state| {
                if let Some(id) = &id
                    && !state.contexts.iter().any(|context| &context.id == id)
                {
                    return Err(DaemonError::NotFound(format!("context {id}")));
                }
                state.active_context_id = id;
                Ok(())
            })
            .await
    }
}
