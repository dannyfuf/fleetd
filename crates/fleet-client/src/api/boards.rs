use super::{Result, expect_ack, unexpected};
use crate::Client;
use fleet_core::{
    board::{
        BackendDescriptor, BackendRef, BackendSchema, BoardPatch, BoardSummary, BoardView, Card,
        CardDraft, CardPatch, ConflictResolution,
    },
    ids::{BoardId, CardId, ContextId, HostId, JobId, RepoId, StatusId},
    model::Worktree,
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

impl Client {
    /// Lists the boards in one context, or in every context.
    pub async fn list_boards(&self, context_id: Option<ContextId>) -> Result<Vec<BoardSummary>> {
        match self.request(RequestBody::ListBoards { context_id }).await? {
            ResponseBody::Boards(value) => Ok(value),
            response => Err(unexpected("list_boards", response)),
        }
    }

    /// Returns a board and its cards.
    pub async fn get_board(&self, board_id: BoardId) -> Result<BoardView> {
        match self.request(RequestBody::GetBoard { board_id }).await? {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("get_board", response)),
        }
    }

    /// Returns the context's board, creating it when the context has none.
    pub async fn ensure_board(&self, context_id: ContextId) -> Result<BoardView> {
        match self
            .request(RequestBody::EnsureBoard { context_id })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("ensure_board", response)),
        }
    }

    /// Creates a board in a context.
    pub async fn create_board(
        &self,
        context_id: ContextId,
        name: Option<String>,
        prefix: Option<String>,
        backend: Option<BackendRef>,
    ) -> Result<BoardView> {
        match self
            .request(RequestBody::CreateBoard {
                context_id,
                name,
                prefix,
                backend,
            })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("create_board", response)),
        }
    }

    /// Applies a patch to a board's configuration.
    pub async fn update_board(&self, board_id: BoardId, patch: BoardPatch) -> Result<BoardView> {
        match self
            .request(RequestBody::UpdateBoard { board_id, patch })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("update_board", response)),
        }
    }

    /// Deletes a board and every card on it.
    pub async fn delete_board(&self, board_id: BoardId) -> Result<()> {
        expect_ack(
            "delete_board",
            self.request(RequestBody::DeleteBoard { board_id }).await?,
        )
    }

    /// Creates a card on a board.
    pub async fn create_card(&self, board_id: BoardId, draft: CardDraft) -> Result<Card> {
        match self
            .request(RequestBody::CreateCard { board_id, draft })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("create_card", response)),
        }
    }

    /// Applies a patch to one card.
    pub async fn update_card(&self, card_id: CardId, patch: CardPatch) -> Result<Card> {
        match self
            .request(RequestBody::UpdateCard { card_id, patch })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("update_card", response)),
        }
    }

    /// Moves a card to a status, optionally at a position inside its column.
    pub async fn move_card(
        &self,
        card_id: CardId,
        status_id: StatusId,
        index: Option<usize>,
    ) -> Result<Card> {
        match self
            .request(RequestBody::MoveCard {
                card_id,
                status_id,
                index,
            })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("move_card", response)),
        }
    }

    /// Deletes one card.
    pub async fn delete_card(&self, card_id: CardId) -> Result<()> {
        expect_ack(
            "delete_card",
            self.request(RequestBody::DeleteCard { card_id }).await?,
        )
    }

    /// Appends a comment to a card and returns the updated card.
    pub async fn add_card_comment(&self, card_id: CardId, body: String) -> Result<Card> {
        match self
            .request(RequestBody::AddCardComment { card_id, body })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("add_card_comment", response)),
        }
    }

    /// Creates or adopts the worktree for a card; the flag says whether it was created.
    pub async fn create_worktree_from_card(
        &self,
        card_id: CardId,
        repo_id: Option<RepoId>,
        base: Option<String>,
        host: Option<HostId>,
    ) -> Result<(Card, Worktree, bool)> {
        match self
            .request(RequestBody::CreateWorktreeFromCard {
                card_id,
                repo_id,
                base,
                host,
            })
            .await?
        {
            ResponseBody::CardWorktree {
                card,
                worktree,
                created,
            } => Ok((card, worktree, created)),
            response => Err(unexpected("create_worktree_from_card", response)),
        }
    }

    /// Starts a board synchronization job; `full` ignores the stored incremental cursor.
    pub async fn sync_board(&self, board_id: BoardId, full: bool) -> Result<JobId> {
        match self
            .request(RequestBody::SyncBoard { board_id, full })
            .await?
        {
            ResponseBody::Job(job) => Ok(job.id),
            response => Err(unexpected("sync_board", response)),
        }
    }

    /// Resolves a card's remote conflict by keeping the local or the remote side.
    pub async fn resolve_card_conflict(
        &self,
        card_id: CardId,
        resolution: ConflictResolution,
    ) -> Result<Card> {
        match self
            .request(RequestBody::ResolveCardConflict {
                card_id,
                resolution,
            })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("resolve_card_conflict", response)),
        }
    }

    /// Returns what a board's backend reports about itself.
    pub async fn describe_board_backend(&self, board_id: BoardId) -> Result<BackendSchema> {
        match self
            .request(RequestBody::DescribeBoardBackend { board_id })
            .await?
        {
            ResponseBody::BoardBackendSchema(value) => Ok(value),
            response => Err(unexpected("describe_board_backend", response)),
        }
    }

    /// Lists the board backend kinds this daemon registers.
    pub async fn list_board_backends(&self) -> Result<Vec<BackendDescriptor>> {
        match self.request(RequestBody::ListBoardBackends {}).await? {
            ResponseBody::BoardBackends(value) => Ok(value),
            response => Err(unexpected("list_board_backends", response)),
        }
    }
}
