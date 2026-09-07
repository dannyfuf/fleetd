//! Board, card, and board-backend operations.

use fleet_core::{
    board::{
        BackendDescriptor, BackendRef, BackendSchema, BoardPatch, BoardSummary, BoardView, Card,
        CardDraft, CardPatch, ConflictResolution,
    },
    ids::{BoardId, CardId, ContextId, HostId, JobId, RepoId, StatusId},
    model::Worktree,
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

use super::{Result, unexpected};
use crate::Client;

impl Client {
    /// List boards through the daemon.
    pub async fn list_boards(&self, context_id: Option<ContextId>) -> Result<Vec<BoardSummary>> {
        match self.request(RequestBody::ListBoards { context_id }).await? {
            ResponseBody::Boards(value) => Ok(value),
            response => Err(unexpected("list_boards", response)),
        }
    }
    /// Get board through the daemon.
    pub async fn get_board(&self, board_id: BoardId) -> Result<BoardView> {
        match self.request(RequestBody::GetBoard { board_id }).await? {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("get_board", response)),
        }
    }
    /// Ensure board through the daemon.
    pub async fn ensure_board(&self, context_id: ContextId) -> Result<BoardView> {
        match self
            .request(RequestBody::EnsureBoard { context_id })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("ensure_board", response)),
        }
    }
    /// Create board through the daemon.
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
    /// Update board through the daemon.
    pub async fn update_board(&self, board_id: BoardId, patch: BoardPatch) -> Result<BoardView> {
        match self
            .request(RequestBody::UpdateBoard { board_id, patch })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("update_board", response)),
        }
    }
    /// Delete board through the daemon.
    pub async fn delete_board(&self, board_id: BoardId) -> Result<()> {
        match self.request(RequestBody::DeleteBoard { board_id }).await? {
            ResponseBody::Ack => Ok(()),
            response => Err(unexpected("delete_board", response)),
        }
    }
    /// Create card through the daemon.
    pub async fn create_card(&self, board_id: BoardId, draft: CardDraft) -> Result<Card> {
        match self
            .request(RequestBody::CreateCard { board_id, draft })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("create_card", response)),
        }
    }
    /// Update card through the daemon.
    pub async fn update_card(&self, card_id: CardId, patch: CardPatch) -> Result<Card> {
        match self
            .request(RequestBody::UpdateCard { card_id, patch })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("update_card", response)),
        }
    }
    /// Move card through the daemon.
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
    /// Delete card through the daemon.
    pub async fn delete_card(&self, card_id: CardId) -> Result<()> {
        match self.request(RequestBody::DeleteCard { card_id }).await? {
            ResponseBody::Ack => Ok(()),
            response => Err(unexpected("delete_card", response)),
        }
    }
    /// Add card comment through the daemon.
    pub async fn add_card_comment(&self, card_id: CardId, body: String) -> Result<Card> {
        match self
            .request(RequestBody::AddCardComment { card_id, body })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("add_card_comment", response)),
        }
    }
    /// Create worktree from card through the daemon; the flag says whether it was created.
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
    /// Sync board through the daemon; `full` ignores the stored incremental cursor.
    pub async fn sync_board(&self, board_id: BoardId, full: bool) -> Result<JobId> {
        match self
            .request(RequestBody::SyncBoard { board_id, full })
            .await?
        {
            ResponseBody::Job(job) => Ok(job.id),
            response => Err(unexpected("sync_board", response)),
        }
    }
    /// Resolve card conflict through the daemon.
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
    /// Describe board backend through the daemon.
    pub async fn describe_board_backend(&self, board_id: BoardId) -> Result<BackendSchema> {
        match self
            .request(RequestBody::DescribeBoardBackend { board_id })
            .await?
        {
            ResponseBody::BoardBackendSchema(value) => Ok(value),
            response => Err(unexpected("describe_board_backend", response)),
        }
    }

    /// List the board backend kinds this daemon registers.
    pub async fn list_board_backends(&self) -> Result<Vec<BackendDescriptor>> {
        match self.request(RequestBody::ListBoardBackends {}).await? {
            ResponseBody::BoardBackends(value) => Ok(value),
            response => Err(unexpected("list_board_backends", response)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::board::{new_board, summarize};
    use fleet_core::model::Context;
    use fleet_core::paths::FleetHome;
    use fleet_proto::{
        PROTOCOL_VERSION,
        error::{ErrorKind, ProtoError},
        job::JobRecord,
    };
    use fleet_proto::{
        codec::FleetCodec,
        job::{JobKind, JobStatus},
        request::Request,
        response::Response,
    };
    use futures_util::{SinkExt, StreamExt};
    use std::{future::Future, time::Duration};
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::Framed;

    type Transport = Framed<UnixStream, FleetCodec<Response, Request>>;

    async fn exchange<T>(
        transport: &mut Transport,
        expected: RequestBody,
        result: Result<ResponseBody>,
        operation: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let server = async {
            let request = transport.next().await.unwrap().unwrap();
            assert_eq!(request.body, expected);
            transport
                .send(Response {
                    id: request.id,
                    result,
                })
                .await
                .unwrap();
        };
        let (_, result) = tokio::join!(server, operation);
        result
    }

    #[tokio::test]
    async fn board_api_round_trips_over_the_unix_socket() {
        tokio::time::timeout(Duration::from_secs(10), board_api_round_trips())
            .await
            .unwrap();
    }

    async fn board_api_round_trips() {
        let home = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(FleetHome::new(home.path()).socket_path()).unwrap();
        let server = async {
            let (socket, _) = listener.accept().await.unwrap();
            let mut transport = Transport::new(socket, FleetCodec::new());
            let hello = transport.next().await.unwrap().unwrap();
            assert!(matches!(
                hello.body,
                RequestBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    ..
                }
            ));
            transport
                .send(Response {
                    id: hello.id,
                    result: Ok(ResponseBody::Hello {
                        protocol: PROTOCOL_VERSION,
                        server: "board-test".into(),
                    }),
                })
                .await
                .unwrap();
            let subscribe = transport.next().await.unwrap().unwrap();
            assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
            transport
                .send(Response {
                    id: subscribe.id,
                    result: Ok(ResponseBody::Ack),
                })
                .await
                .unwrap();
            transport
        };
        let (client, mut transport) = tokio::join!(Client::connect(home.path()), server);
        let client = client.unwrap();
        let context = Context {
            id: "work".parse().unwrap(),
            name: "Work".into(),
            owners: vec![],
            created_at: "now".into(),
        };
        let board = new_board(&context, "now");
        let card: Card = serde_json::from_value(serde_json::json!({
            "id":"card-1", "boardId":"work", "number":1, "title":"Task", "statusId":"todo",
            "createdAt":"now", "updatedAt":"now"
        }))
        .unwrap();
        let view = BoardView {
            board: board.clone(),
            cards: vec![card.clone()],
        };
        let summary = summarize(&board, &view.cards);
        let worktree: Worktree = serde_json::from_value(serde_json::json!({
            "id":"acme/api#task", "repoId":"acme/api", "slug":"task", "branch":"task",
            "baseRef":"main", "path":"/tmp/task", "session":"task", "createdAt":"now"
        }))
        .unwrap();
        let job = JobRecord {
            id: "sync-job".parse().unwrap(),
            kind: JobKind::Custom("board.sync".into()),
            target: board.id.to_string(),
            title: "Sync board".into(),
            status: JobStatus::Queued,
            progress: None,
            log_path: "/tmp/sync.log".into(),
            started_at: "now".into(),
            finished_at: None,
            cancellable: true,
            retryable: false,
        };
        // Each operation is checked at the socket boundary, including every argument and typed result.
        macro_rules! check {
            ($request:expr, $response:expr, $operation:expr, $expected:expr) => {
                assert_eq!(
                    exchange(&mut transport, $request, Ok($response), $operation)
                        .await
                        .unwrap(),
                    $expected
                );
            };
        }
        check!(
            RequestBody::ListBoards {
                context_id: Some(context.id.clone())
            },
            ResponseBody::Boards(vec![summary.clone()]),
            client.list_boards(Some(context.id.clone())),
            vec![summary]
        );
        check!(
            RequestBody::GetBoard {
                board_id: board.id.clone()
            },
            ResponseBody::Board(view.clone()),
            client.get_board(board.id.clone()),
            view
        );
        check!(
            RequestBody::EnsureBoard {
                context_id: context.id.clone()
            },
            ResponseBody::Board(view.clone()),
            client.ensure_board(context.id.clone()),
            view
        );
        check!(
            RequestBody::CreateBoard {
                context_id: context.id.clone(),
                name: Some("Team".into()),
                prefix: Some("TM".into()),
                backend: Some(BackendRef::default())
            },
            ResponseBody::Board(view.clone()),
            client.create_board(
                context.id.clone(),
                Some("Team".into()),
                Some("TM".into()),
                Some(BackendRef::default())
            ),
            view
        );
        let board_patch = BoardPatch {
            default_repo_id: Some(None),
            ..Default::default()
        };
        check!(
            RequestBody::UpdateBoard {
                board_id: board.id.clone(),
                patch: board_patch.clone()
            },
            ResponseBody::Board(view.clone()),
            client.update_board(board.id.clone(), board_patch),
            view
        );
        let draft = CardDraft {
            title: "Task".into(),
            ..Default::default()
        };
        check!(
            RequestBody::CreateCard {
                board_id: board.id.clone(),
                draft: draft.clone()
            },
            ResponseBody::Card(card.clone()),
            client.create_card(board.id.clone(), draft),
            card
        );
        let patch = CardPatch {
            assignee: Some(None),
            estimate: Some(Some(3)),
            ..Default::default()
        };
        check!(
            RequestBody::UpdateCard {
                card_id: card.id.clone(),
                patch: patch.clone()
            },
            ResponseBody::Card(card.clone()),
            client.update_card(card.id.clone(), patch),
            card
        );
        check!(
            RequestBody::MoveCard {
                card_id: card.id.clone(),
                status_id: card.status_id.clone(),
                index: Some(2)
            },
            ResponseBody::Card(card.clone()),
            client.move_card(card.id.clone(), card.status_id.clone(), Some(2)),
            card
        );
        check!(
            RequestBody::AddCardComment {
                card_id: card.id.clone(),
                body: "Hello".into()
            },
            ResponseBody::Card(card.clone()),
            client.add_card_comment(card.id.clone(), "Hello".into()),
            card
        );
        check!(
            RequestBody::CreateWorktreeFromCard {
                card_id: card.id.clone(),
                repo_id: Some(worktree.repo_id.clone()),
                base: Some("main".into()),
                host: Some("devbox".parse().unwrap())
            },
            ResponseBody::CardWorktree {
                card: card.clone(),
                worktree: worktree.clone(),
                created: true
            },
            client.create_worktree_from_card(
                card.id.clone(),
                Some(worktree.repo_id.clone()),
                Some("main".into()),
                Some("devbox".parse().unwrap())
            ),
            (card.clone(), worktree, true)
        );
        check!(
            RequestBody::SyncBoard {
                board_id: board.id.clone(),
                full: false
            },
            ResponseBody::Job(job.clone()),
            client.sync_board(board.id.clone(), false),
            job.id
        );
        check!(
            RequestBody::SyncBoard {
                board_id: board.id.clone(),
                full: true
            },
            ResponseBody::Job(job.clone()),
            client.sync_board(board.id.clone(), true),
            job.id
        );
        let descriptors = vec![BackendDescriptor {
            kind: "local".into(),
            label: "Local".into(),
            capabilities: Default::default(),
            settings_schema: vec![],
        }];
        check!(
            RequestBody::ListBoardBackends {},
            ResponseBody::BoardBackends(descriptors.clone()),
            client.list_board_backends(),
            descriptors
        );
        check!(
            RequestBody::ResolveCardConflict {
                card_id: card.id.clone(),
                resolution: ConflictResolution::TakeRemote
            },
            ResponseBody::Card(card.clone()),
            client.resolve_card_conflict(card.id.clone(), ConflictResolution::TakeRemote),
            card
        );
        let schema = BackendSchema {
            key_prefix: Some("EXT".into()),
            ..Default::default()
        };
        check!(
            RequestBody::DescribeBoardBackend {
                board_id: board.id.clone()
            },
            ResponseBody::BoardBackendSchema(schema.clone()),
            client.describe_board_backend(board.id.clone()),
            schema
        );
        check!(
            RequestBody::DeleteCard {
                card_id: card.id.clone()
            },
            ResponseBody::Ack,
            client.delete_card(card.id.clone()),
            ()
        );
        check!(
            RequestBody::DeleteBoard {
                board_id: board.id.clone()
            },
            ResponseBody::Ack,
            client.delete_board(board.id.clone()),
            ()
        );

        let error = ProtoError {
            kind: ErrorKind::NotFound,
            message: "board missing".into(),
        };
        assert_eq!(
            exchange(
                &mut transport,
                RequestBody::GetBoard {
                    board_id: board.id.clone()
                },
                Err(error.clone()),
                client.get_board(board.id.clone())
            )
            .await
            .unwrap_err(),
            error
        );
        let unexpected = exchange(
            &mut transport,
            RequestBody::SyncBoard {
                board_id: board.id.clone(),
                full: false,
            },
            Ok(ResponseBody::Ack),
            client.sync_board(board.id, false),
        )
        .await
        .unwrap_err();
        assert_eq!(unexpected.kind, ErrorKind::Unknown);
        assert!(unexpected.message.contains("sync_board"));
    }
}
