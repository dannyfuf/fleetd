//! Pluggable board synchronization boundaries.

use crate::adapters::{clock::Clock, shell::Shell};
use fleet_core::board::{
    BackendCapabilities, BackendDescriptor, BackendSchema, Board, BoardError, Card, PropertySchema,
    PullResult, PushOp, PushResult,
};
use std::{collections::HashMap, sync::Arc};

/// Jira boards mirrored through the Atlassian CLI.
pub mod jira;
/// Local-only board backend.
pub mod local;
pub use jira::JiraBackend;
pub use local::LocalBackend;

/// Maps a remote issue system into the backend-independent board model.
#[async_trait::async_trait]
pub trait BoardBackend: Send + Sync {
    /// Registry key identifying this backend.
    fn kind(&self) -> &'static str;
    /// Human name shown by clients wherever the kind would be unreadable.
    fn label(&self) -> &'static str;
    /// Operations supported by this backend.
    fn capabilities(&self) -> BackendCapabilities;
    /// Generic description of `BackendRef.settings`, so clients render it without knowing
    /// anything about this backend.
    fn settings_schema(&self) -> Vec<PropertySchema>;
    /// Validates backend-owned configuration.
    async fn validate(&self, settings: &serde_json::Value) -> Result<(), BoardError>;
    /// The settings as this backend will actually read them, for the service to store.
    ///
    /// A backend that rewrites what it was given — dropping a clause every search would append
    /// its own copy of, trimming a host — must say so here, or the settings dialog and
    /// `board show --json` keep showing a value the backend has been ignoring all along.
    /// Only keys the caller supplied may be rewritten: filling defaults in would turn every
    /// unset optional row into a value the user never chose.
    async fn normalize(
        &self,
        settings: &serde_json::Value,
    ) -> Result<serde_json::Value, BoardError> {
        Ok(settings.clone())
    }
    /// Describes remote statuses and custom properties.
    async fn describe(&self, board: &Board) -> Result<BackendSchema, BoardError>;
    /// Fetches remote changes since an optional cursor.
    async fn pull(&self, board: &Board, cursor: Option<&str>) -> Result<PullResult, BoardError>;
    /// Applies pending operations using current local card data.
    async fn push(
        &self,
        board: &Board,
        cards: &[Card],
        ops: &[PushOp],
    ) -> Result<PushResult, BoardError>;
}

/// Cloneable registry of available board backends.
#[derive(Clone, Default)]
pub struct BoardBackends {
    inner: Arc<HashMap<String, Arc<dyn BoardBackend>>>,
}

impl BoardBackends {
    /// Registers implementations by their stable kind.
    pub fn new(backends: Vec<Arc<dyn BoardBackend>>) -> Self {
        Self {
            inner: Arc::new(
                backends
                    .into_iter()
                    .map(|b| (b.kind().to_owned(), b))
                    .collect(),
            ),
        }
    }
    /// Resolves a backend or returns a domain validation error.
    pub fn get(&self, kind: &str) -> Result<Arc<dyn BoardBackend>, BoardError> {
        self.inner
            .get(kind)
            .cloned()
            .ok_or_else(|| BoardError::UnknownBackend(kind.into()))
    }
    /// Returns sorted registered backend keys.
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut kinds: Vec<_> = self.inner.values().map(|b| b.kind()).collect();
        kinds.sort_unstable();
        kinds
    }
    /// Describes every registered backend, sorted by kind with `local` first.
    pub fn descriptors(&self) -> Vec<BackendDescriptor> {
        let mut descriptors: Vec<_> = self
            .inner
            .values()
            .map(|backend| BackendDescriptor {
                kind: backend.kind().to_owned(),
                label: backend.label().to_owned(),
                capabilities: backend.capabilities(),
                settings_schema: backend.settings_schema(),
            })
            .collect();
        // `local` is what every board starts as, so it heads the list a settings dialog cycles.
        descriptors.sort_by(|a, b| {
            (a.kind != fleet_core::board::BackendRef::LOCAL, &a.kind)
                .cmp(&(b.kind != fleet_core::board::BackendRef::LOCAL, &b.kind))
        });
        descriptors
    }

    /// Builds the production registry over the adapters its backends shell out through.
    pub fn system(shell: Arc<dyn Shell>, clock: Arc<dyn Clock>) -> Self {
        Self::new(vec![
            Arc::new(LocalBackend),
            Arc::new(JiraBackend::new(shell, clock)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fakes::{FakeBackend, FakeBackendCall};
    use fleet_core::{board::new_board, model::Context};

    fn board() -> Board {
        new_board(
            &Context {
                id: "work".parse().unwrap(),
                name: "Work".into(),
                owners: vec![],
                created_at: "now".into(),
            },
            "now",
        )
    }
    #[tokio::test]
    async fn local_registry_and_backend_are_safe_noops() {
        let backends = BoardBackends::system(
            Arc::new(crate::testing::fakes::FakeShell::new()),
            Arc::new(crate::adapters::clock::SystemClock),
        );
        assert_eq!(backends.kinds(), vec!["jira", "local"]);
        assert_eq!(
            backends
                .descriptors()
                .iter()
                .map(|descriptor| (descriptor.kind.as_str(), descriptor.label.as_str()))
                .collect::<Vec<_>>(),
            vec![("local", "Local"), ("jira", "Jira (acli)")]
        );
        assert!(backends.get("missing").is_err());
        let backend = backends.get("local").unwrap();
        let board = board();
        assert_eq!(backend.capabilities(), BackendCapabilities::default());
        backend.validate(&serde_json::Value::Null).await.unwrap();
        assert_eq!(
            backend.describe(&board).await.unwrap(),
            BackendSchema::default()
        );
        assert_eq!(
            backend.pull(&board, None).await.unwrap(),
            PullResult::default()
        );
        assert_eq!(
            backend.push(&board, &[], &[]).await.unwrap(),
            PushResult::default()
        );
    }
    #[tokio::test]
    async fn fake_consumes_scripted_responses_and_records_arguments() {
        let backend = FakeBackend::default();
        let board = board();
        backend
            .describe_responses
            .lock()
            .unwrap()
            .push_back(Ok(BackendSchema {
                key_prefix: Some("EXT".into()),
                ..Default::default()
            }));
        backend
            .pull_responses
            .lock()
            .unwrap()
            .push_back(Err(BoardError::Backend("offline".into())));
        backend
            .push_responses
            .lock()
            .unwrap()
            .push_back(Ok(PushResult::default()));
        assert_eq!(
            backend
                .describe(&board)
                .await
                .unwrap()
                .key_prefix
                .as_deref(),
            Some("EXT")
        );
        assert!(backend.pull(&board, Some("cursor")).await.is_err());
        backend.push(&board, &[], &[]).await.unwrap();
        assert_eq!(
            backend.calls(),
            vec![
                FakeBackendCall::Describe(board.clone()),
                FakeBackendCall::Pull(board.clone(), Some("cursor".into())),
                FakeBackendCall::Push(board.clone(), vec![], vec![])
            ]
        );
        assert_eq!(
            backend.pull(&board, None).await.unwrap(),
            PullResult::default()
        );
    }
}
