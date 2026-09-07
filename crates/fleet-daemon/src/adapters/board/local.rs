//! Local boards need no remote I/O.
use super::BoardBackend;
use fleet_core::board::{
    BackendCapabilities, BackendSchema, Board, BoardError, Card, PropertySchema, PullResult,
    PushOp, PushResult,
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
    async fn describe(&self, board: &Board) -> Result<BackendSchema, BoardError> {
        let _ = board;
        Ok(BackendSchema::default())
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
