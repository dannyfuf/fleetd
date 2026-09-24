//! Local boards need no remote I/O.
use super::BoardBackend;
use fleet_core::board::{
    BackendCapabilities, BackendSchema, Board, BoardError, Card, PropertySchema, PullResult,
    PushOp, PushResult, RemoteStatus,
};

/// No-op backend for locally persisted boards.
pub struct LocalBackend;

#[async_trait::async_trait]
impl BoardBackend for LocalBackend {
    fn kind(&self) -> &'static str {
        "local"
    }
    fn label(&self) -> &'static str {
        "Local"
    }
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }
    fn settings_schema(&self) -> Vec<PropertySchema> {
        Vec::new()
    }
    async fn validate(&self, settings: &serde_json::Value) -> Result<(), BoardError> {
        let _ = settings;
        Ok(())
    }
    /// The board itself: a local board has no remote to ask, so its own columns, labels,
    /// properties and prefix are the schema. `fleet board describe` is how an agent — a
    /// scheduled run's footer says so — finds the labels it may put on a card.
    async fn describe(&self, board: &Board) -> Result<BackendSchema, BoardError> {
        Ok(BackendSchema {
            statuses: board
                .statuses
                .iter()
                .map(|status| RemoteStatus {
                    id: status.id.to_string(),
                    name: status.name.clone(),
                    category: Some(status.category),
                })
                .collect(),
            labels: board
                .labels
                .iter()
                .map(|label| label.id.to_string())
                .collect(),
            properties: board.properties.clone(),
            assignees: Vec::new(),
            key_prefix: Some(board.prefix.clone()),
            readonly_fields: Vec::new(),
        })
    }
    async fn pull(&self, board: &Board, cursor: Option<&str>) -> Result<PullResult, BoardError> {
        let _ = (board, cursor);
        Ok(PullResult::default())
    }
    async fn push(
        &self,
        board: &Board,
        cards: &[Card],
        ops: &[PushOp],
    ) -> Result<PushResult, BoardError> {
        let _ = (board, cards, ops);
        Ok(PushResult::default())
    }
}
