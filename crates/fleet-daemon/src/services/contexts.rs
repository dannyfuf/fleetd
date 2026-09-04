//! Context selection and lifecycle operation contracts.

use std::sync::Arc;

use fleet_core::{ids::ContextId, model::Context};

use crate::{DaemonError, DaemonResult, stores::state::StateStore};

/// Context domain service backed by durable state.
#[derive(Clone)]
pub struct Contexts {
    _state: Arc<StateStore>,
}

impl Contexts {
    /// Creates the context service.
    #[must_use]
    pub fn new(state: Arc<StateStore>) -> Self {
        Self { _state: state }
    }

    /// Creates a normalized context and persists it transactionally (inventory sections 1 and 2).
    pub async fn create(&self, _name: String, _owners: Vec<String>) -> DaemonResult<Context> {
        Err(DaemonError::Unimplemented("contexts::create"))
    }

    /// Updates context display fields while preserving referential integrity (inventory section 1).
    pub async fn update(
        &self,
        _id: ContextId,
        _name: Option<String>,
        _owners: Option<Vec<String>>,
    ) -> DaemonResult<Context> {
        Err(DaemonError::Unimplemented("contexts::update"))
    }

    /// Deletes a context and cascades repositories, worktrees, and sessions (inventory section 1).
    pub async fn delete(&self, _id: ContextId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("contexts::delete"))
    }

    /// Changes or clears the active context in one state transaction (inventory sections 1 and 6).
    pub async fn set_active(&self, _id: Option<ContextId>) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("contexts::set_active"))
    }
}
