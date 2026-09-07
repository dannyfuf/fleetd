//! Board persistence, backend synchronization jobs, and card worktree orchestration.
use super::worktrees::Worktrees;
use crate::{
    DaemonError, DaemonResult,
    adapters::{board::BoardBackends, clock::Clock},
    jobs::{JobCtx, JobManager},
    server::BroadcastBus,
    stores::{board::BoardStore, state::StateStore},
};
use fleet_core::{
    board::*,
    ids::{BoardId, CardId, ContextId, HostId, JobId, RepoId, StatusId},
    model::Worktree,
};
use fleet_proto::{
    event::{BoardChangeReason, Event},
    job::JobKind,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

/// Coordinates board persistence, backend synchronization, and worktree creation.
#[derive(Clone)]
pub struct Boards {
    store: Arc<BoardStore>,
    state_store: Arc<StateStore>,
    backends: BoardBackends,
    clock: Arc<dyn Clock>,
    jobs: Arc<JobManager>,
    worktrees: Arc<Worktrees>,
    events: BroadcastBus,
    index: Arc<RwLock<HashMap<CardId, BoardId>>>,
    /// One lock per board. A single process-wide lock would let a clone or a backend sync on
    /// one board block every request for every other one until the client's timeout.
    gates: Arc<Mutex<HashMap<BoardId, Arc<Mutex<()>>>>>,
    /// The last reported load failure per board. The snapshot refresh rescans every document
    /// roughly every two seconds, and one unreadable file must not fill the log with it.
    unreadable: Arc<std::sync::Mutex<HashMap<BoardId, String>>>,
    /// Each board's summary and the document stamp it was parsed from.
    ///
    /// Every other snapshot field is served from memory; board summaries are the one thing a
    /// snapshot reads from disk, and a snapshot is assembled whenever a session changes — up
    /// to twenty times a second. Reparsing and revalidating every card, comment and activity
    /// entry of every board at that rate is pure waste when no board file has changed.
    summaries: Arc<RwLock<HashMap<BoardId, (DocumentStamp, BoardSummary)>>>,
}

/// The size and modification time a summary was parsed from.
type DocumentStamp = (u64, std::time::SystemTime);

impl Boards {
    /// Constructs board orchestration around shared daemon services and event bus.
    pub fn new(
        store: Arc<BoardStore>,
        state_store: Arc<StateStore>,
        backends: BoardBackends,
        clock: Arc<dyn Clock>,
        jobs: Arc<JobManager>,
        worktrees: Arc<Worktrees>,
        events: BroadcastBus,
    ) -> Self {
        Self {
            store,
            state_store,
            backends,
            clock,
            jobs,
            worktrees,
            events,
            index: Arc::new(RwLock::new(HashMap::new())),
            gates: Arc::new(Mutex::new(HashMap::new())),
            unreadable: Arc::new(std::sync::Mutex::new(HashMap::new())),
            summaries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Serializes the read-modify-write cycles of one board without touching the others.
    async fn gate(&self, id: &BoardId) -> tokio::sync::OwnedMutexGuard<()> {
        let board = {
            let mut gates = self.gates.lock().await;
            Arc::clone(gates.entry(id.clone()).or_default())
        };
        board.lock_owned().await
    }

    /// Loads one document while scanning the store: an unreadable file is skipped rather than
    /// fatal, and is reported once per distinct error instead of on every refresh.
    fn scan_load(&self, id: &BoardId) -> Option<BoardDocument> {
        // `peek`, never `load`: quarantining here would move the damaged file aside and let
        // the `ensure` that scanned for it create an empty board in its place.
        let loaded = self.store.peek(id);
        let mut seen = self
            .unreadable
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match loaded {
            Ok(doc) => {
                seen.remove(id);
                doc
            }
            Err(error) => {
                let error = error.to_string();
                if seen.insert(id.clone(), error.clone()).as_ref() != Some(&error) {
                    tracing::warn!(%id, %error, "skipping unreadable board");
                }
                None
            }
        }
    }

    /// The board this context already owns, if the store holds one.
    ///
    /// A document this build cannot read belongs to some other context until proven otherwise:
    /// propagating its error here would take every context's board down with one bad file.
    /// A board is named after the context that owns it, so `ensure` and `create` still load
    /// their own id directly and refuse to write over a damaged one.
    fn context_board(&self, context: &ContextId) -> DaemonResult<Option<BoardId>> {
        // A board is named after the context that owns it, so the common case is one document
        // read. `ensure` is the app's refresh path after every `BoardChanged`: scanning, parsing
        // and re-validating every board in the home on each card edit is the cost the
        // `summaries` memo was built to avoid, and this path bypasses it.
        if let Ok(id) = BoardId::try_from(context.as_str())
            && self
                .scan_load(&id)
                .is_some_and(|doc| doc.board.context_id == *context)
        {
            return Ok(Some(id));
        }
        for id in self.store.list()? {
            if self
                .scan_load(&id)
                .is_some_and(|doc| doc.board.context_id == *context)
            {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }
    /// Lists healthy boards belonging to existing contexts, optionally restricted to one context.
    pub async fn list(&self, context: Option<&ContextId>) -> DaemonResult<Vec<BoardSummary>> {
        let state = self.state_store.load().await?;
        // A context that does not exist is a mistyped request, not a context without boards.
        if let Some(context) = context
            && !state.contexts.iter().any(|record| record.id == *context)
        {
            return Err(DaemonError::NotFound(format!("context {context}")));
        }
        let mut summaries = Vec::new();
        for id in self.store.list()? {
            let Some(doc) = self.scan_load(&id) else {
                continue;
            };
            if context.is_some_and(|id| *id != doc.board.context_id)
                || !state.contexts.iter().any(|c| c.id == doc.board.context_id)
            {
                continue;
            }
            self.index
                .write()
                .await
                .extend(doc.cards.iter().map(|c| (c.id.clone(), id.clone())));
            summaries.push(summarize(&doc.board, &doc.cards));
        }
        Ok(summaries)
    }
    /// Loads a full board and removes stale worktree links from the returned view only.
    pub async fn get(&self, id: &BoardId) -> DaemonResult<BoardView> {
        let mut doc = self
            .store
            .load(id)?
            .ok_or_else(|| BoardError::BoardNotFound(id.to_string()))?;
        let state = self.state_store.load().await?;
        let context = doc.board.context_id.clone();
        scrub_repo(&state, &context, &mut doc.board.default_repo_id);
        let mut index = self.index.write().await;
        index.retain(|_, board| board != id);
        for card in &mut doc.cards {
            index.insert(card.id.clone(), id.clone());
            if card
                .worktree_id
                .as_ref()
                .is_some_and(|id| !state.worktrees.iter().any(|w| w.id == *id))
            {
                card.worktree_id = None;
            }
            scrub_repo(&state, &context, &mut card.repo_id);
        }
        Ok(BoardView {
            board: doc.board,
            cards: doc.cards,
        })
    }
    /// Gets or creates the context's first local board; unknown contexts are errors.
    pub async fn ensure(&self, context: &ContextId) -> DaemonResult<BoardView> {
        let state = self.state_store.load().await?;
        let context_record = state
            .contexts
            .iter()
            .find(|c| c.id == *context)
            .ok_or_else(|| DaemonError::NotFound(format!("context {context}")))?
            .clone();
        // Reading an existing board is the app's refresh path after every BoardChanged: it
        // takes no lock, so it can never queue behind a worktree clone or a backend sync.
        if let Some(id) = self.context_board(context)? {
            return self.get(&id).await;
        }
        let mut board = new_board(&context_record, &self.clock.now().to_rfc3339());
        let _guard = self.gate(&board.id).await;
        if let Some(id) = self.context_board(context)? {
            return self.get(&id).await;
        }
        self.refuse_over_quarantine(&board.id)?;
        if self.store.load(&board.id)?.is_some() {
            return Err(BoardError::Duplicate(context.to_string()).into());
        }
        let backend = self.backends.get(&board.backend.kind)?;
        backend.validate(&board.backend.settings).await?;
        board.backend.settings = backend.normalize(&board.backend.settings).await?;
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: Vec::new(),
        };
        self.save(&doc, BoardChangeReason::Created).await?;
        Ok(BoardView {
            board: doc.board,
            cards: doc.cards,
        })
    }
    /// Returns available summaries without making a broken board fail the whole snapshot.
    ///
    /// A board whose document has not changed since it was last parsed is served from the
    /// `summaries` memo instead of being read, parsed and validated again.
    pub async fn summaries(&self) -> Vec<BoardSummary> {
        let state = match self.state_store.load().await {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!(%error, "failed to load board summaries");
                return Vec::new();
            }
        };
        let ids = match self.store.list() {
            Ok(ids) => ids,
            Err(error) => {
                tracing::warn!(%error, "failed to load board summaries");
                return Vec::new();
            }
        };
        let mut summaries = Vec::new();
        for id in &ids {
            let stamp = self.store.stamp(id);
            let cached = match stamp {
                Some(stamp) => self
                    .summaries
                    .read()
                    .await
                    .get(id)
                    .filter(|(parsed, _)| *parsed == stamp)
                    .map(|(_, summary)| summary.clone()),
                None => None,
            };
            let summary = match cached {
                Some(summary) => summary,
                None => {
                    let Some(doc) = self.scan_load(id) else {
                        continue;
                    };
                    self.index
                        .write()
                        .await
                        .extend(doc.cards.iter().map(|c| (c.id.clone(), id.clone())));
                    let summary = summarize(&doc.board, &doc.cards);
                    if let Some(stamp) = stamp {
                        self.summaries
                            .write()
                            .await
                            .insert(id.clone(), (stamp, summary.clone()));
                    }
                    summary
                }
            };
            if state
                .contexts
                .iter()
                .any(|context| context.id == summary.context_id)
            {
                summaries.push(summary);
            }
        }
        // Only when a board went away: the hot path must not take the write lock.
        if self.summaries.read().await.len() > ids.len() {
            self.summaries
                .write()
                .await
                .retain(|id, _| ids.contains(id));
        }
        summaries
    }
    /// Creates the context's only board after validating its backend configuration.
    pub async fn create(
        &self,
        context: &ContextId,
        name: Option<String>,
        prefix: Option<String>,
        backend: Option<BackendRef>,
    ) -> DaemonResult<BoardView> {
        let state = self.state_store.load().await?;
        let context_record = state
            .contexts
            .iter()
            .find(|c| c.id == *context)
            .ok_or_else(|| DaemonError::NotFound(format!("context {context}")))?
            .clone();
        let now = self.now();
        let mut board = new_board(&context_record, &now);
        let _guard = self.gate(&board.id).await;
        // Unlike snapshot reads, creation must not write over a damaged document: the direct
        // load below reports it, even though the scan above steps around it.
        if self.context_board(context)?.is_some() {
            return Err(BoardError::Duplicate(context.to_string()).into());
        }
        self.refuse_over_quarantine(&board.id)?;
        if self.store.load(&board.id)?.is_some() {
            return Err(BoardError::Duplicate(context.to_string()).into());
        }
        apply_board_patch(
            &mut board,
            BoardPatch {
                name,
                prefix,
                backend,
                ..Default::default()
            },
            &now,
        )?;
        let backend = self.backends.get(&board.backend.kind)?;
        backend.validate(&board.backend.settings).await?;
        board.backend.settings = backend.normalize(&board.backend.settings).await?;
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: Vec::new(),
        };
        self.save(&doc, BoardChangeReason::Created).await?;
        self.get(&doc.board.id).await
    }

    /// Updates board configuration, rejecting removal of referenced statuses or labels.
    ///
    /// A patch that changes the backend **kind** takes its settings from the patch alone: the
    /// previous kind's settings name a project on another system, and everything the old
    /// backend left behind — cursor, status map, read-only fields — describes a remote this
    /// board no longer talks to. Both are dropped before the patch is applied, so a client
    /// that echoes back the settings it was showing cannot smuggle them into the new kind.
    pub async fn update(&self, id: &BoardId, patch: BoardPatch) -> DaemonResult<BoardView> {
        let _guard = self.gate(id).await;
        let mut doc = self.load(id)?;
        let backend_before = doc.board.backend.clone();
        self.validate_repo(
            &doc.board,
            patch.default_repo_id.as_ref().and_then(Option::as_ref),
        )
        .await?;
        if let Some(backend) = patch.backend.as_ref()
            && *backend != doc.board.backend
        {
            if backend.kind != doc.board.backend.kind {
                if doc
                    .cards
                    .iter()
                    .any(|card| card.remote.is_some() || card.conflict.is_some())
                {
                    return Err(DaemonError::Conflict(
                        "cannot change the backend kind of a linked board".into(),
                    ));
                }
                // The patch is the entire description of the new kind. Dropping the stored
                // settings before the patch is applied keeps that true however
                // `apply_board_patch` composes the two: no key shaped for the old kind can
                // reach the new backend's `validate`, and `local` keeps no settings at all.
                doc.board.backend.settings = serde_json::Value::Null;
                // Cursor, status map and read-only fields all describe the remote this board
                // is leaving; keeping them would map its columns onto a system that never had
                // them.
                doc.board.sync = SyncState::default();
            } else {
                // A required setting is the backend's identity — the Jira project a key belongs
                // to. Changing it under linked cards points every one of them at a system that
                // never issued its key, and the next full pull archives the whole board and
                // calls the sync a success. Same refusal as a kind change, same reason.
                let identity = self
                    .backends
                    .get(&backend.kind)?
                    .settings_schema()
                    .into_iter()
                    .filter(fleet_core::board::is_required)
                    .map(|row| row.key)
                    .find(|key| backend.settings.get(key) != doc.board.backend.settings.get(key));
                if let Some(key) = identity
                    && doc.cards.iter().any(|card| card.remote.is_some())
                {
                    return Err(DaemonError::Conflict(format!(
                        "cannot change `{key}` on a board whose cards are already linked"
                    )));
                }
                // Same kind, new settings (a filter, a story-points field, a column list): the
                // status map and the read-only list still describe this remote and survive —
                // dropping them would disarm the read-only guard until the next sync. The
                // cursor cannot: it is a window into a query this board no longer runs.
                doc.board.sync.cursor = None;
            }
        }
        if !apply_board_patch(&mut doc.board, patch, &self.now())? {
            // A patch that changes nothing writes nothing and announces nothing, exactly as an
            // empty card patch and a no-op move do.
            return self.get(id).await;
        }
        for card in &doc.cards {
            if !doc
                .board
                .statuses
                .iter()
                .any(|status| status.id == card.status_id)
            {
                return Err(BoardError::Invalid {
                    field: "statuses".into(),
                    reason: format!(
                        "status {} is used by card {}",
                        card.status_id,
                        card.display_key(&doc.board)
                    ),
                }
                .into());
            }
            if let Some(label) = card
                .labels
                .iter()
                .find(|id| !doc.board.labels.iter().any(|label| label.id == **id))
            {
                return Err(BoardError::Invalid {
                    field: "labels".into(),
                    reason: format!(
                        "label {label} is used by card {}",
                        card.display_key(&doc.board)
                    ),
                }
                .into());
            }
            if let Some(key) = card.properties.keys().find(|key| {
                !doc.board
                    .properties
                    .iter()
                    .any(|schema| schema.key == **key)
            }) {
                return Err(BoardError::Invalid {
                    field: "properties".into(),
                    reason: format!(
                        "property {key} is used by card {}",
                        card.display_key(&doc.board)
                    ),
                }
                .into());
            }
        }
        // Only a backend the patch actually touched is asked about itself. `validate` is two
        // `acli` subprocesses for Jira, and running them for `--add-label`, a rename or a
        // conflict policy made a purely local edit wait on the network — and fail outright
        // whenever the CLI happened to be signed out of a backend the edit never mentioned.
        if doc.board.backend != backend_before {
            let adapter = self.backends.get(&doc.board.backend.kind)?;
            adapter.validate(&doc.board.backend.settings).await?;
            doc.board.backend.settings = adapter.normalize(&doc.board.backend.settings).await?;
        }
        if doc.board.backend.is_local() {
            for card in &mut doc.cards {
                card.dirty = false;
            }
        }
        self.save(&doc, BoardChangeReason::Updated).await?;
        self.get(id).await
    }

    /// Moves a board document to trash and removes its card lookup entries.
    pub async fn delete(&self, id: &BoardId) -> DaemonResult<()> {
        let _guard = self.gate(id).await;
        self.load(id)?;
        self.store.delete(id)?;
        self.summaries.write().await.remove(id);
        self.index.write().await.retain(|_, board| board != id);
        self.changed(id, BoardChangeReason::Deleted);
        Ok(())
    }

    /// Deletes every board belonging to `context`, so deleting a context cannot strand a
    /// document that a later context with the same derived id would silently adopt.
    pub async fn delete_for_context(&self, context: &ContextId) -> DaemonResult<()> {
        for id in self.store.list()? {
            let owned = match self.store.peek(&id) {
                Ok(Some(doc)) => doc.board.context_id == *context,
                Ok(None) => false,
                // An unreadable document still belongs to the context by its file name.
                Err(error) => {
                    tracing::warn!(%id, %error, "deleting unreadable board with its context");
                    id.as_str() == context.as_str()
                }
            };
            if owned {
                // Not `delete`: that one loads the document first, which is exactly what fails
                // here. The context's repos are already gone by now, so refusing would leave
                // a context that can never be deleted and can never get its repos back.
                let _guard = self.gate(&id).await;
                self.store.delete(&id)?;
                self.summaries.write().await.remove(&id);
                self.index.write().await.retain(|_, board| *board != id);
                self.changed(&id, BoardChangeReason::Deleted);
            }
        }
        // A board whose only remains are quarantined is in no listing, and leaving them behind
        // would refuse a board to every later context that derives the same id.
        if let Ok(id) = BoardId::try_from(context.as_str())
            && !self.store.quarantined(&id)?.is_empty()
        {
            let _guard = self.gate(&id).await;
            self.store.delete(&id)?;
        }
        Ok(())
    }

    /// Allocates a UUID and local number, then persists a validated card.
    pub async fn create_card(&self, board: &BoardId, draft: CardDraft) -> DaemonResult<Card> {
        let _guard = self.gate(board).await;
        let mut doc = self.load(board)?;
        self.validate_repo(&doc.board, draft.repo_id.as_ref())
            .await?;
        validate_parent(&doc.cards, None, draft.parent_id.as_ref())?;
        let now = self.now();
        ops::check_draft_writable(&doc.board, &draft)?;
        let card = ops::create_card(&mut doc.board, &doc.cards, new_card_id()?, draft, &now)?;
        doc.cards.push(card.clone());
        self.save(&doc, BoardChangeReason::CardChanged).await?;
        Ok(card)
    }

    /// Applies a partial card edit through the pure domain operations.
    pub async fn update_card(&self, card: &CardId, patch: CardPatch) -> DaemonResult<Card> {
        let (_guard, mut doc, index) = self.card_document(card).await?;
        let now = self.now();
        self.validate_repo(&doc.board, patch.repo_id.as_ref().and_then(Option::as_ref))
            .await?;
        // A card that owns a worktree keeps the repository that worktree lives in. Clearing it
        // would leave `Repo: —` next to a live worktree, and pointing it at a different
        // repository is the same wound with a name on it: `create_worktree_from_card` then
        // refuses to adopt or recreate the worktree, and this path refuses to clear it, so the
        // card can never reach any repository again. Only the worktree's own repository passes.
        if let Some(next) = patch.repo_id.as_ref()
            && let Some(worktree) = doc.cards[index].worktree_id.clone()
            && let Some(live) = self
                .state_store
                .load()
                .await?
                .worktrees
                .into_iter()
                .find(|w| w.id == worktree)
            && next.as_ref() != Some(&live.repo_id)
        {
            return Err(DaemonError::Conflict(match next {
                None => {
                    format!("cannot clear the repository of a card linked to worktree {worktree}")
                }
                Some(repo) => {
                    format!("cannot move a card linked to worktree {worktree} to repository {repo}")
                }
            }));
        }
        validate_parent(
            &doc.cards,
            Some(card),
            patch.parent_id.as_ref().and_then(Option::as_ref),
        )?;
        let changed = apply_card_patch(&doc.board, &mut doc.cards[index], patch, &now)?;
        if changed.is_empty() {
            // A patch that changes nothing writes nothing and announces nothing.
            return self.card_view(&doc.board, &doc.cards[index]).await;
        }
        // Only `move_card` renumbers a column, so a status changed by a patch would keep the
        // position it held in the column it left and interleave with cards it never met.
        if changed.iter().any(|field| field == "status_id") {
            // The patch path moves a card exactly as `move_card` does, so it refuses the same
            // card: an archived one belongs to no column and cannot be positioned in one.
            if doc.cards[index].archived {
                return Err(BoardError::Invalid {
                    field: "archived".into(),
                    reason: "cannot move an archived card".into(),
                }
                .into());
            }
            let status = doc.cards[index].status_id.clone();
            doc.cards[index].position = doc
                .cards
                .iter()
                .enumerate()
                .filter(|(other, card)| {
                    *other != index
                        && card.board_id == doc.board.id
                        && !card.archived
                        && card.status_id == status
                })
                .map(|(_, card)| card.position)
                .max()
                .map_or(0, |position| position.saturating_add(10));
        }
        self.save_card(doc, index, &now).await
    }

    /// Moves a card and persists all affected column positions atomically.
    pub async fn move_card(
        &self,
        card: &CardId,
        status: &StatusId,
        index: Option<usize>,
    ) -> DaemonResult<Card> {
        let (_guard, mut doc, card_index) = self.card_document(card).await?;
        let now = self.now();
        // A move that lands where the card already was writes nothing and announces nothing,
        // exactly as an empty patch does.
        if !ops::move_card(&doc.board, &mut doc.cards, card, status, index, &now)? {
            return self.card_view(&doc.board, &doc.cards[card_index]).await;
        }
        self.save_card(doc, card_index, &now).await
    }

    /// Deletes a card and clears references to it from its children.
    pub async fn delete_card(&self, card: &CardId) -> DaemonResult<()> {
        let (_guard, mut doc, index) = self.locked_card_document(card).await?;
        // A mirrored card cannot be deleted from here: nothing carries the deletion to the
        // backend, so the next pull files the issue again as a brand-new card — with a new
        // number and none of the comments, activity, worktree link or column position this
        // row held. A confirmation that promises a deletion the sync undoes, and drops local
        // data on the way, is worse than a refusal that names where the issue actually lives.
        if let Some(link) = &doc.cards[index].remote {
            return Err(DaemonError::Validation(format!(
                "{} mirrors {} on the `{}` backend: deleting it here would drop its comments \
                 and activity and the next sync would bring the issue back. Delete or close it \
                 in the backend, or archive the card.",
                doc.cards[index].display_key(&doc.board),
                link.key,
                doc.board.backend.kind
            )));
        }
        doc.cards.remove(index);
        let now = self.now();
        for child in &mut doc.cards {
            if child.parent_id.as_ref() != Some(card) {
                continue;
            }
            let patch = CardPatch {
                parent_id: Some(None),
                ..Default::default()
            };
            match apply_card_patch(&doc.board, child, patch, &now) {
                Ok(_) => {}
                // A backend that cannot write parents back (Jira: `acli edit` has no parent
                // flag) still must not leave a child pointing at a card this document no
                // longer holds — `validate_card` refuses that, and the board would be unable
                // to save anything again. The link is dropped locally and never marked dirty:
                // there is no push that could carry it, and the next pull decides the truth.
                Err(BoardError::ReadOnlyField(_)) => {
                    child.parent_id = None;
                    child.updated_at.clone_from(&now);
                    push_activity(
                        child,
                        ActivityKind::Updated,
                        None,
                        "Updated parent_id",
                        &now,
                    );
                }
                Err(error) => return Err(error.into()),
            }
        }
        doc.board.updated_at = now;
        self.save(&doc, BoardChangeReason::CardChanged).await
    }

    /// Appends a locally authored comment with a UUID and clock timestamp.
    pub async fn add_comment(&self, card: &CardId, body: String) -> DaemonResult<Card> {
        let (_guard, mut doc, index) = self.card_document(card).await?;
        let now = self.now();
        ops::add_comment(
            &mut doc.cards[index],
            uuid::Uuid::new_v4().to_string(),
            None,
            body,
            &now,
        )?;
        doc.cards[index].dirty = !doc.board.backend.is_local();
        self.save_card(doc, index, &now).await
    }

    /// Creates or reuses the card's repository worktree and optionally starts the card.
    pub async fn create_worktree_from_card(
        &self,
        card: &CardId,
        repo: Option<RepoId>,
        base: Option<String>,
        host: Option<HostId>,
    ) -> DaemonResult<(Card, Worktree, bool)> {
        // The same refusal `CreateWorktree` gives: nothing below this line can reach a remote
        // host, and silently creating the worktree locally would link the card to the wrong one.
        if host.is_some() {
            return Err(DaemonError::Unsupported(
                "remote hosts are not supported yet".to_owned(),
            ));
        }
        // Own the entire create-and-link transaction independently of the socket request.
        let service = self.clone();
        let card = card.clone();
        tokio::spawn(async move { service.create_and_link_worktree(&card, repo, base).await })
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    async fn create_and_link_worktree(
        &self,
        card: &CardId,
        repo: Option<RepoId>,
        base: Option<String>,
    ) -> DaemonResult<(Card, Worktree, bool)> {
        let (mut guard, mut doc, mut index) = self.card_document(card).await?;
        if doc.cards[index].archived {
            return Err(DaemonError::Conflict(
                "cannot create a worktree for an archived card".into(),
            ));
        }
        let state = self.state_store.load().await?;
        // The card and the board are only offered the repositories their views show, and a
        // view drops one the state no longer backs: falling back to a deleted repository here
        // would refuse the card by a name nothing on the board still displays.
        let mut card_repo = doc.cards[index].repo_id.clone();
        let mut board_repo = doc.board.default_repo_id.clone();
        scrub_repo(&state, &doc.board.context_id, &mut card_repo);
        scrub_repo(&state, &doc.board.context_id, &mut board_repo);
        let repo_id = repo
            .or(card_repo)
            .or(board_repo)
            .ok_or_else(|| BoardError::Invalid {
                field: "repo_id".into(),
                reason: "select a repository for this card or board".into(),
            })?;
        self.validate_repo(&doc.board, Some(&repo_id)).await?;
        let repo = state
            .repos
            .iter()
            .find(|repo| repo.id == repo_id)
            .ok_or_else(|| DaemonError::NotFound(format!("repo {repo_id}")))?
            .clone();
        // Relinking a card to a worktree in another repository would strand the live one it
        // already owns: nothing on the board could reach that worktree again.
        if let Some(linked) = doc.cards[index].worktree_id.clone()
            && state
                .worktrees
                .iter()
                .any(|w| w.id == linked && w.repo_id != repo_id)
        {
            return Err(DaemonError::Conflict(format!(
                "card is already linked to worktree {linked} in another repository"
            )));
        }
        let slug = worktree_slug(&doc.board, &doc.cards[index]);
        let by_slug = state
            .worktrees
            .iter()
            .find(|w| w.repo_id == repo_id && w.slug == slug);
        // Adopting a worktree by slug is how `fleet create` and a card meet, but never one
        // another card already owns: two cards on one worktree open each other's session.
        if let Some(existing) = by_slug
            && doc
                .cards
                .iter()
                .any(|other| other.id != *card && other.worktree_id.as_ref() == Some(&existing.id))
        {
            return Err(DaemonError::Conflict(format!(
                "worktree {} already belongs to another card",
                existing.id
            )));
        }
        let linked = state
            .worktrees
            .iter()
            .find(|w| w.repo_id == repo_id && doc.cards[index].worktree_id.as_ref() == Some(&w.id))
            .or(by_slug)
            .cloned();
        let (worktree, created) = match linked {
            Some(existing) => (existing, false),
            None => {
                // Creating a worktree clones a repository and runs its hooks: minutes of work
                // that must not hold the board, or every other request for it times out.
                drop(guard);
                let made = match self
                    .worktrees
                    .create(
                        repo_id.clone(),
                        slug.clone(),
                        Some(slug.clone()),
                        base,
                        repo.hooks.clone(),
                    )
                    .await
                {
                    Ok(made) => made,
                    // Two `w` presses on one card both pass the adoption check above and both
                    // reach here, because the guard is dropped for the clone. The loser must
                    // adopt what the winner just made — reporting `already exists` for the
                    // card's own worktree is a toast for work that succeeded.
                    Err(error @ DaemonError::Conflict(_)) => {
                        let adopted = self
                            .state_store
                            .load()
                            .await?
                            .worktrees
                            .into_iter()
                            .find(|w| w.repo_id == repo_id && w.slug == slug);
                        match adopted {
                            Some(worktree) => (false, worktree, None),
                            None => return Err(error),
                        }
                    }
                    Err(error) => return Err(error),
                };
                let reloaded = match self.card_document(card).await {
                    Ok(reloaded) => reloaded,
                    // The guard was dropped for the clone, so the card can be deleted while its
                    // worktree is being made. Nothing on the board references the worktree now:
                    // the refusal names it, or `fleet list` is the only trace it ever existed.
                    Err(error) => {
                        return Err(DaemonError::Conflict(format!(
                            "worktree {} was created, but its card is gone: {error}",
                            made.1.id
                        )));
                    }
                };
                guard = reloaded.0;
                doc = reloaded.1;
                index = reloaded.2;
                if doc.cards[index].archived {
                    return Err(DaemonError::Conflict(
                        "cannot create a worktree for an archived card".into(),
                    ));
                }
                // The ownership check above ran under the guard this clone dropped. Two cards
                // that render the same slug would otherwise both adopt one worktree here.
                if doc.cards.iter().any(|other| {
                    other.id != *card && other.worktree_id.as_ref() == Some(&made.1.id)
                }) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree {} already belongs to another card",
                        made.1.id
                    )));
                }
                // `Worktrees::create` is itself idempotent: it reports whether it made one.
                (made.1, made.0)
            }
        };
        let _guard = guard;
        let now = self.now();
        let target = &mut doc.cards[index];
        target.worktree_id = Some(worktree.id.clone());
        target.repo_id = Some(repo_id);
        push_activity(
            target,
            ActivityKind::WorktreeCreated,
            None,
            format!("Linked worktree {}", worktree.id),
            &now,
        );
        let should_start = doc.board.settings.start_on_worktree
            && doc.board.statuses.iter().any(|status| {
                status.id == target.status_id
                    && matches!(
                        status.category,
                        StatusCategory::Backlog | StatusCategory::Unstarted
                    )
            });
        if should_start && let Some(status) = first_status_in(&doc.board, StatusCategory::Started) {
            ops::move_card(&doc.board, &mut doc.cards, card, &status.id, None, &now)?;
        }
        Ok((self.save_card(doc, index, &now).await?, worktree, created))
    }

    /// Describes every backend kind this build registers.
    pub fn list_backends(&self) -> Vec<BackendDescriptor> {
        self.backends.descriptors()
    }

    /// Submits a detached schema/pull/push job; local boards do not submit jobs.
    ///
    /// `full` clears the stored cursor **before** the job starts, so the pull that runs is a
    /// full one even though `sync_document` reads the cursor minutes later, and a full sync
    /// that fails halfway still leaves the board asking for a full one next time.
    pub async fn sync(&self, id: &BoardId, full: bool) -> DaemonResult<JobId> {
        let doc = self.load(id)?;
        if doc.board.backend.is_local() {
            return Err(BoardError::Unsupported("pull").into());
        }
        // A backend this build does not register is a bad request, not a job that fails
        // minutes later: `create` and `update` already refuse it up front.
        self.backends.get(&doc.board.backend.kind)?;
        if full {
            // Under the same gate `run_sync` takes, and re-read inside it: a sync already
            // running would otherwise write the cursor back over the clearing.
            let _guard = self.gate(id).await;
            let mut doc = self.load(id)?;
            if doc.board.sync.cursor.take().is_some() {
                doc.board.updated_at = self.now();
                self.persist(&doc).await?;
            }
        }
        let service = self.clone();
        let id = id.clone();
        Ok(self.jobs.submit(
            JobKind::Custom("board.sync".into()),
            id.to_string(),
            format!("Sync {}", doc.board.name),
            false,
            true,
            move |context| async move { service.run_sync(&id, &context).await },
        ))
    }

    /// Resolves a conflict, materializing remote labels and resolving remote parent keys.
    pub async fn resolve_conflict(
        &self,
        card: &CardId,
        resolution: ConflictResolution,
    ) -> DaemonResult<Card> {
        let (_guard, mut doc, index) = self.card_document(card).await?;
        let now = self.now();
        let mut parent = None;
        if resolution == ConflictResolution::TakeRemote
            && let Some(conflict) = doc.cards[index].conflict.clone()
        {
            let mut scratch = doc.cards[index].clone();
            apply_remote(&mut doc.board, &mut scratch, &conflict.remote, &now);
            parent = conflict
                .remote
                .parent_key
                .as_ref()
                .and_then(|key| {
                    doc.cards.iter().find(|candidate| {
                        candidate.id != *card
                            && candidate
                                .remote
                                .as_ref()
                                .is_some_and(|link| link.key == *key)
                    })
                })
                .map(|candidate| candidate.id.clone());
            // A remote parent that would close a cycle is dropped exactly like a key that
            // names no local card: `UpdateCard` refuses one, and the contract forbids it.
            if validate_parent(&doc.cards, Some(card), parent.as_ref()).is_err() {
                parent = None;
            }
        }
        sync::resolve_conflict(&doc.board, &mut doc.cards[index], resolution, &now)?;
        if resolution == ConflictResolution::TakeRemote {
            doc.cards[index].parent_id = parent;
        }
        self.save_card(doc, index, &now).await
    }

    /// Fetches backend metadata without mutating the persisted board.
    pub async fn describe_backend(&self, id: &BoardId) -> DaemonResult<BackendSchema> {
        let doc = self.load(id)?;
        Ok(self
            .backends
            .get(&doc.board.backend.kind)?
            .describe(&doc.board)
            .await?)
    }

    /// Refuses to create a board over a document this build had to quarantine.
    ///
    /// The board a context owns is named after it, so a fresh one would take the place of the
    /// damaged document and report success. The file is still there, under its `.broken-` name:
    /// naming it is the only way a user learns the board was not empty.
    fn refuse_over_quarantine(&self, id: &BoardId) -> DaemonResult<()> {
        if let Some(path) = self.store.quarantined(id)?.first() {
            return Err(DaemonError::Conflict(format!(
                "board {id} has a quarantined document at {}; restore or remove it before creating a board",
                path.display()
            )));
        }
        Ok(())
    }

    fn now(&self) -> String {
        self.clock.now().to_rfc3339()
    }

    fn load(&self, id: &BoardId) -> DaemonResult<BoardDocument> {
        self.store
            .load(id)?
            .ok_or_else(|| BoardError::BoardNotFound(id.to_string()).into())
    }

    /// A repository a board may point at: it must exist and share the board's context, or the
    /// app — which only ever offers this context's repos — could never show what was stored.
    async fn validate_repo(&self, board: &Board, id: Option<&RepoId>) -> DaemonResult<()> {
        let Some(id) = id else {
            return Ok(());
        };
        let state = self.state_store.load().await?;
        let repo = state
            .repos
            .iter()
            .find(|repo| repo.id == *id)
            .ok_or_else(|| DaemonError::NotFound(format!("repo {id}")))?;
        if repo.context_id != board.context_id {
            return Err(BoardError::Invalid {
                field: "repo_id".into(),
                reason: format!("repo {id} belongs to context {}", repo.context_id),
            }
            .into());
        }
        Ok(())
    }

    /// Locks the card's board, then reloads the document under that lock.
    async fn locked_card_document(
        &self,
        id: &CardId,
    ) -> DaemonResult<(tokio::sync::OwnedMutexGuard<()>, BoardDocument, usize)> {
        let (doc, _) = self.find_card_document(id).await?;
        let guard = self.gate(&doc.board.id).await;
        let (doc, index) = self.find_card_document(id).await?;
        Ok((guard, doc, index))
    }

    async fn card_document(
        &self,
        id: &CardId,
    ) -> DaemonResult<(tokio::sync::OwnedMutexGuard<()>, BoardDocument, usize)> {
        let (guard, doc, index) = self.locked_card_document(id).await?;
        require_push_baseline(&doc.cards[index])?;
        Ok((guard, doc, index))
    }

    async fn find_card_document(&self, id: &CardId) -> DaemonResult<(BoardDocument, usize)> {
        let cached = self.index.read().await.get(id).cloned();
        if let Some(board) = cached
            && let Some(doc) = self.scan_load(&board)
            && let Some(index) = doc.cards.iter().position(|card| card.id == *id)
        {
            return Ok((doc, index));
        }
        self.index.write().await.remove(id);
        // A document this build cannot read hides its own cards, never every other board's.
        for board in self.store.list()? {
            let Some(doc) = self.scan_load(&board) else {
                continue;
            };
            self.index.write().await.extend(
                doc.cards
                    .iter()
                    .map(|card| (card.id.clone(), board.clone())),
            );
            if let Some(index) = doc.cards.iter().position(|card| card.id == *id) {
                return Ok((doc, index));
            }
        }
        Err(BoardError::CardNotFound(id.to_string()).into())
    }

    async fn save_card(
        &self,
        mut doc: BoardDocument,
        index: usize,
        now: &str,
    ) -> DaemonResult<Card> {
        doc.board.updated_at = now.into();
        self.save(&doc, BoardChangeReason::CardChanged).await?;
        self.card_view(&doc.board, &doc.cards[index]).await
    }

    /// One card as the protocol reports it: a worktree or repository link the state no longer
    /// backs is dropped, exactly as `get` drops it from a whole board.
    async fn card_view(&self, board: &Board, card: &Card) -> DaemonResult<Card> {
        let mut card = card.clone();
        let state = self.state_store.load().await?;
        if card
            .worktree_id
            .as_ref()
            .is_some_and(|id| !state.worktrees.iter().any(|w| w.id == *id))
        {
            card.worktree_id = None;
        }
        scrub_repo(&state, &board.context_id, &mut card.repo_id);
        Ok(card)
    }

    async fn persist(&self, doc: &BoardDocument) -> DaemonResult<()> {
        for card in &doc.cards {
            validate_card(&doc.board, card)?;
        }
        self.store.save(doc)?;
        // A file this daemon just rewrote may land inside the modification-time resolution of
        // the one it replaced, so the write drops the memo rather than trusting the stamp.
        self.summaries.write().await.remove(&doc.board.id);
        let mut index = self.index.write().await;
        index.retain(|_, board| *board != doc.board.id);
        index.extend(
            doc.cards
                .iter()
                .map(|card| (card.id.clone(), doc.board.id.clone())),
        );
        Ok(())
    }

    async fn save(&self, doc: &BoardDocument, reason: BoardChangeReason) -> DaemonResult<()> {
        self.persist(doc).await?;
        self.changed(&doc.board.id, reason);
        Ok(())
    }

    fn changed(&self, id: &BoardId, reason: BoardChangeReason) {
        self.events.publish(Event::BoardChanged {
            board_id: id.clone(),
            reason,
        });
        self.events.request_snapshot_current();
    }

    async fn run_sync(&self, id: &BoardId, context: &JobCtx) -> DaemonResult<()> {
        let _guard = self.gate(id).await;
        let result = self.sync_document(id, context).await;
        if let Err(error) = &result {
            // Reload the last valid checkpoint, never persist a partially invalid pull.
            if let Ok(mut doc) = self.load(id) {
                doc.board.sync.last_error = Some(error.to_string());
                doc.board.updated_at = self.now();
                if let Err(save_error) = self.persist(&doc).await {
                    tracing::warn!(%id, %save_error, "failed to persist board sync error");
                }
            }
            self.changed(id, BoardChangeReason::SyncFailed);
        }
        result
    }

    async fn sync_document(&self, id: &BoardId, context: &JobCtx) -> DaemonResult<()> {
        let mut doc = self.load(id)?;
        let backend = self.backends.get(&doc.board.backend.kind)?;
        let caps = backend.capabilities();
        if !caps.pull {
            return Err(BoardError::Unsupported("pull").into());
        }
        context.progress("describing board backend")?;
        let schema = backend.describe(&doc.board).await?;
        context.progress("adopting backend schema")?;
        let now = self.now();
        if doc.board.sync.last_synced_at.is_none() {
            doc.board.sync.last_synced_at = doc
                .cards
                .iter()
                .filter_map(|card| card.remote.as_ref())
                .map(|link| link.synced_at.clone())
                .max();
        }
        let old_properties = doc.board.properties.clone();
        let old_statuses = doc.board.statuses.clone();
        let mut unmapped = adopt_schema(&mut doc.board, &schema, &now);
        for card in &mut doc.cards {
            card.properties.retain(|key, value| {
                if !old_properties
                    .iter()
                    .any(|schema| schema.key == *key && schema.source == PropertySource::Backend)
                {
                    return true;
                }
                doc.board.properties.iter().any(|schema| {
                    // A schema with no options at all states nothing about the values it
                    // allows: Jira's issue-type options come from a `project view` whose
                    // failure is a warning, and dropping every value against an empty list
                    // would delete the property from every card over one failed metadata call.
                    schema.key == *key
                        && value.matches_kind(schema.kind)
                        && (schema.options.is_empty()
                            || match value {
                                PropertyValue::Select(value) => {
                                    schema.options.iter().any(|option| option.value == *value)
                                }
                                PropertyValue::MultiSelect(values) => values.iter().all(|value| {
                                    schema.options.iter().any(|option| option.value == *value)
                                }),
                                _ => true,
                            })
                })
            });
            if !doc
                .board
                .statuses
                .iter()
                .any(|status| status.id == card.status_id)
            {
                let old = old_statuses
                    .iter()
                    .find(|status| status.id == card.status_id);
                let status = old
                    .and_then(|old| {
                        doc.board
                            .statuses
                            .iter()
                            .find(|status| status.name.eq_ignore_ascii_case(&old.name))
                    })
                    .or_else(|| old.and_then(|old| first_status_in(&doc.board, old.category)))
                    .or_else(|| doc.board.statuses.first())
                    .ok_or_else(|| BoardError::Invalid {
                        field: "statuses".into(),
                        reason: "backend provided no usable status".into(),
                    })?;
                card.status_id = status.id.clone();
                card.updated_at = now.clone();
            }
        }
        context.progress("pulling remote cards")?;
        let cursor = if caps.incremental {
            doc.board.sync.cursor.as_deref()
        } else {
            None
        };
        let mut pull = backend.pull(&doc.board, cursor).await?;
        if !pull.full
            && doc.cards.iter().any(|card| {
                (card.dirty || awaiting_push_baseline(card))
                    && card.remote.as_ref().is_some_and(|link| {
                        !pull.cards.iter().any(|remote| remote.key == link.key)
                            && !pull.deleted_keys.contains(&link.key)
                    })
            })
        {
            // A cursor omission cannot tell us which fields (especially status) still need pushing.
            pull = backend.pull(&doc.board, None).await?;
            if doc.cards.iter().any(|card| {
                (card.dirty || awaiting_push_baseline(card))
                    && card.remote.as_ref().is_some_and(|link| {
                        !pull.full
                            && !pull.cards.iter().any(|remote| remote.key == link.key)
                            && !pull.deleted_keys.contains(&link.key)
                    })
            }) {
                return Err(
                    BoardError::Backend("backend omitted a dirty card's baseline".into()).into(),
                );
            }
        }
        for card in doc
            .cards
            .iter_mut()
            .filter(|card| awaiting_push_baseline(card))
        {
            let key = &card.remote.as_ref().expect("recovery requires a link").key;
            if pull.deleted_keys.contains(key)
                || (pull.full && !pull.cards.iter().any(|remote| remote.key == *key))
            {
                card.archived = true;
                card.dirty = false;
                card.conflict = None;
                push_activity(
                    card,
                    ActivityKind::Synced,
                    None,
                    fleet_core::board::ARCHIVED_BY_SYNC,
                    &self.now(),
                );
                continue;
            }
            let remote = pull
                .cards
                .iter()
                .find(|remote| {
                    card.remote
                        .as_ref()
                        .is_some_and(|link| link.key == remote.key)
                })
                .ok_or_else(|| BoardError::Backend("missing post-push baseline".into()))?;
            if let Some(link) = &mut card.remote {
                link.version = remote.version.clone();
                link.remote_updated_at = remote.updated_at.clone();
            }
            push_activity(
                card,
                ActivityKind::Synced,
                None,
                "Refreshed post-push baseline",
                &self.now(),
            );
        }
        // A sampled `describe` names only the statuses its sample happened to use, and a
        // project gains statuses between two syncs. Adopting the ones this pull brought back —
        // before anything reconciles against them — is what turns them into columns instead of
        // "unmapped status" noise on the activity of every card that holds one.
        if let Some(readopted) =
            readopt_pulled_statuses(&mut doc.board, &schema, &pull, &self.now())
        {
            context.progress("adopting newly seen remote statuses")?;
            // The re-adoption covers everything `describe` named plus what the pull added, so
            // its answer replaces the first one rather than joining it.
            unmapped = readopted;
        }
        context.progress("reconciling remote cards")?;
        let now = self.now();
        let mut reconciled = reconcile(&doc.board, &doc.cards, &pull, caps, &now);
        // The pure engine allocates deterministic placeholders; persistence owns UUIDs.
        let mut ids = HashMap::new();
        for card in &mut reconciled.cards {
            if !doc.cards.iter().any(|existing| existing.id == card.id) {
                let id = new_card_id()?;
                ids.insert(card.id.clone(), id.clone());
                card.id = id;
            }
        }
        for card in &mut reconciled.cards {
            if let Some(id) = card.parent_id.as_ref().and_then(|id| ids.get(id)) {
                card.parent_id = Some(id.clone());
            }
        }
        for op in &mut reconciled.to_push {
            let card_id = match op {
                PushOp::Create { card_id }
                | PushOp::Update { card_id, .. }
                | PushOp::Transition { card_id, .. }
                | PushOp::AddComment { card_id, .. } => card_id,
            };
            if let Some(id) = ids.get(card_id) {
                *card_id = id.clone();
            }
        }
        reconciled.summary.unmapped_statuses.extend(unmapped);
        reconciled.summary.unmapped_statuses.sort();
        reconciled.summary.unmapped_statuses.dedup();
        let checkpoint_cursor = doc.board.sync.cursor.clone();
        doc.board = reconciled.board;
        doc.cards = reconciled.cards;
        if let Some(error) = &doc.board.sync.last_error {
            return Err(BoardError::Backend(error.clone()).into());
        }
        let unimportable = std::mem::take(&mut reconciled.summary.skipped);
        let mut skipped = unimportable.clone();
        // A key the backend listed but could not read is reported exactly like one this board
        // could not import: named, without failing the cards that did reconcile.
        skipped.extend(pull.failed_keys.iter().cloned());
        let skipped_warning = (!skipped.is_empty()).then(|| {
            format!(
                "skipped {} remote cards: {}",
                skipped.len(),
                skipped.join("; ")
            )
        });
        if !unimportable.is_empty() {
            // One unimportable remote issue must not throw away the cards that did reconcile,
            // and must not advance the cursor past itself: the next pull offers it again.
            //
            // Only *this* board's failure to import rewinds the cursor. A key the backend
            // could not read is the backend's own business and it has already answered with
            // the cursor it wants kept — holding its watermark while still advancing whatever
            // counts pulls towards the next full one. Rewinding the whole opaque string on top
            // of that undid the second half: one permanently unreadable key froze the counter,
            // no full pull ever came due again, and remote deletions stopped being noticed
            // while the incremental window grew without bound.
            doc.board.sync.cursor = checkpoint_cursor;
        }
        // A remote issue this board cannot import is a warning about that issue, never a
        // reason to strand every local edit: the push below is the only way a dirty card ever
        // reaches the backend, and failing here left it queued forever behind one bad key.
        if let Some(warning) = &skipped_warning
            && reconciled.summary.created == 0
            && reconciled.summary.updated == 0
            && reconciled.summary.deleted == 0
            && reconciled.to_push.is_empty()
        {
            self.persist(&doc).await?;
            return Err(BoardError::Backend(warning.clone()).into());
        }
        self.persist(&doc).await?;
        if !reconciled.to_push.is_empty() {
            context.progress(format!("pushing {} operations", reconciled.to_push.len()))?;
            let result = backend
                .push(&doc.board, &doc.cards, &reconciled.to_push)
                .await?;
            context.progress("applying push acknowledgements")?;
            apply_push_result(
                &mut doc.cards,
                &result,
                &doc.board.backend.kind,
                &reconciled.unpushed,
                &self.now(),
            );
            reconciled.summary.pushed = result
                .acks
                .iter()
                .filter(|ack| {
                    doc.cards.iter().any(|card| card.id == ack.card_id)
                        && !result
                            .failures
                            .iter()
                            .any(|failure| failure.card_id == ack.card_id)
                })
                .count();
            self.persist(&doc).await?;
            if result.acks.iter().any(|ack| ack.version.is_none()) {
                context.progress("refreshing post-push baselines")?;
                let baseline = backend.pull(&doc.board, None).await?;
                for ack in result.acks.iter().filter(|ack| ack.version.is_none()) {
                    let remote = baseline
                        .cards
                        .iter()
                        .find(|remote| remote.key == ack.key)
                        .ok_or_else(|| {
                            BoardError::Backend(format!(
                                "missing post-push baseline for {}",
                                ack.key
                            ))
                        })?;
                    if let Some(card) = doc.cards.iter_mut().find(|card| card.id == ack.card_id) {
                        if let Some(link) = &mut card.remote {
                            link.version = remote.version.clone();
                            link.remote_updated_at = remote.updated_at.clone();
                        }
                        push_activity(
                            card,
                            ActivityKind::Synced,
                            None,
                            "Refreshed post-push baseline",
                            &self.now(),
                        );
                    }
                }
                self.persist(&doc).await?;
            }
            if !result.failures.is_empty() {
                // The card's display key, never its UUID: this sentence is what `board sync`
                // and the board list print, and a user cannot look a UUID up anywhere.
                return Err(BoardError::Backend(
                    result
                        .failures
                        .iter()
                        .map(|failure| {
                            let name = doc
                                .cards
                                .iter()
                                .find(|card| card.id == failure.card_id)
                                .map_or_else(
                                    || failure.card_id.to_string(),
                                    |card| card.display_key(&doc.board),
                                );
                            format!("{name}: {}", failure.error)
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                )
                .into());
            }
        }
        context.progress("saving board sync result")?;
        doc.board.sync.last_synced_at = Some(self.now());
        // The skipped keys stay visible where every surface already looks for a board's
        // trouble, instead of being lost with the job that reported them. `reconcile` clears
        // it on the next sync, so the warning lasts exactly as long as its cause.
        doc.board.sync.last_error = skipped_warning.clone();
        doc.board.updated_at = self.now();
        self.save(&doc, BoardChangeReason::Synced).await?;
        // `deleted` is counted here and nowhere else: a card this sync archived because its
        // issue is gone drops out of every column, out of `summarize` and out of `board show`,
        // and without this number nothing at all says it happened.
        // Both clauses are conditional: an unconditional `unmapped statuses: ` left every
        // successful sync ending in a colon with nothing after it.
        let unmapped_clause = if reconciled.summary.unmapped_statuses.is_empty() {
            String::new()
        } else {
            format!(
                "; unmapped statuses: {}",
                reconciled.summary.unmapped_statuses.join(", ")
            )
        };
        // `pushNewCards` defaults to off, so a card made on a linked board files no issue and
        // stays dirty forever. The sync used to report `0 pushed` and leave the user to guess;
        // this names the setting that is holding them, once, with the count.
        let kept_local_clause = if reconciled.summary.kept_local == 0 {
            String::new()
        } else {
            format!(
                "; {} kept local (pushNewCards is off)",
                reconciled.summary.kept_local
            )
        };
        context.progress(format!("synced: {} pulled, {} created, {} updated, {} archived, {} conflicts, {} pushed{unmapped_clause}{kept_local_clause}{}",
            reconciled.summary.pulled, reconciled.summary.created, reconciled.summary.updated,
            reconciled.summary.deleted, reconciled.summary.conflicts, reconciled.summary.pushed,
            skipped_warning.map(|warning| format!("; {warning}")).unwrap_or_default()))?;
        Ok(())
    }
}

/// Adopts the statuses a pull brought back that the board's status map does not name yet.
///
/// Returns the unmapped statuses of the second adoption, or `None` when there was nothing to
/// adopt. The scratch schema is the described one with those statuses added: `adopt_schema`
/// rebuilds the whole status map, and replaces the board's backend properties, labels and
/// read-only fields, from the schema it is handed — a schema carrying only the new statuses
/// would erase the mapping of every status `describe` did name.
fn readopt_pulled_statuses(
    board: &mut Board,
    schema: &BackendSchema,
    pull: &PullResult,
    now: &str,
) -> Option<Vec<String>> {
    let unseen = unseen_statuses(board, pull);
    if unseen.is_empty() {
        return None;
    }
    let mut used: std::collections::BTreeSet<String> = board
        .statuses
        .iter()
        .map(|status| status.id.to_string())
        .collect();
    for remote in &unseen {
        let name = if remote.name.trim().is_empty() {
            remote_status_key(remote)
        } else {
            remote.name.clone()
        };
        // A column already carrying this name is the column this status belongs to:
        // `adopt_schema` maps the two together, and a second one would split the work in half.
        if board
            .statuses
            .iter()
            .any(|status| status.name.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        let Ok(id) = StatusId::try_from(unique_status_slug(&name, &mut used)) else {
            continue;
        };
        let declared = remote_category(remote);
        let status = Status {
            id,
            name,
            category: declared.unwrap_or(StatusCategory::Unstarted),
            color: None,
        };
        // A board reads left to right in workflow order, and appending put a *started* column
        // to the right of the completed one the moment a project gained a status between two
        // syncs. A status whose category the remote declared goes after the last column of
        // that category or earlier — where `describe` would have placed it had its sample
        // named it the first time. One the remote left unnamed is a guess either way, and the
        // end is the least surprising place for a column nobody can order.
        match declared {
            Some(category) => {
                let at = board
                    .statuses
                    .iter()
                    .rposition(|existing| existing.category <= category)
                    .map_or(0, |index| index + 1);
                board.statuses.insert(at, status);
            }
            None => board.statuses.push(status),
        }
    }
    // `adopt_schema` adopts a schema wholesale on a board that still carries the default
    // columns with nothing mapped to them. Reaching here with those columns untouched means
    // every pulled status matched one by name, so a second adoption would trade the whole
    // default set for the handful this pull happened to mention — including the columns the
    // cards this sync just repaired now sit in.
    if board.statuses == default_statuses()
        && board.sync.status_map.remote_to_local.is_empty()
        && board.sync.last_synced_at.is_none()
    {
        return None;
    }
    let keys: std::collections::BTreeSet<String> = unseen.iter().map(remote_status_key).collect();
    let mut scratch = schema.clone();
    // A status `describe` named but never mapped can appear in both lists; it is the pull's
    // copy that carries the category the issues actually use.
    scratch
        .statuses
        .retain(|status| !keys.contains(&remote_status_key(status)));
    scratch.statuses.extend(unseen);
    Some(adopt_schema(board, &scratch, now))
}

/// The distinct statuses of pulled cards that no live column is mapped to.
fn unseen_statuses(board: &Board, pull: &PullResult) -> Vec<RemoteStatus> {
    let mut seen = std::collections::BTreeSet::new();
    let mut unseen = Vec::new();
    for status in pull.cards.iter().map(|card| &card.status) {
        let key = remote_status_key(status);
        // A status with neither an id nor a name is no status at all: adopting it would add a
        // nameless column, which `validate_board` refuses.
        if key.is_empty() {
            continue;
        }
        let mapped = [&status.id, &status.name]
            .into_iter()
            .filter(|key| !key.is_empty())
            .any(|key| {
                board
                    .sync
                    .status_map
                    .remote_to_local
                    .get(key)
                    // A map entry pointing at a column that no longer exists names nothing.
                    .is_some_and(|id| board.statuses.iter().any(|status| status.id == *id))
            });
        if mapped || !seen.insert(key) {
            continue;
        }
        unseen.push(status.clone());
    }
    unseen
}

/// A status id no column on this board already holds.
fn unique_status_slug(name: &str, used: &mut std::collections::BTreeSet<String>) -> String {
    let mut base = fleet_core::slug::normalize_context_id(name);
    if base.is_empty() {
        base = "status".into();
    }
    let mut slug = base.clone();
    let mut suffix = 2;
    while !used.insert(slug.clone()) {
        slug = format!("{base}-{suffix}");
        suffix += 1;
    }
    slug
}

/// Drops a repository link the state no longer backs.
///
/// A deleted repository — or one moved to another context — stays written on the card, so
/// every view would keep offering it as the card's repository while `create_worktree_from_card`
/// refused it with an error that never says the repository is gone. Only the view is scrubbed:
/// the stored id survives a repository that comes back under the same name.
fn scrub_repo(state: &fleet_core::state::State, context: &ContextId, id: &mut Option<RepoId>) {
    if id.as_ref().is_some_and(|id| {
        !state
            .repos
            .iter()
            .any(|repo| repo.id == *id && repo.context_id == *context)
    }) {
        *id = None;
    }
}

/// Rejects a parent that is missing, on another board, the card itself, or its own descendant.
fn validate_parent(
    cards: &[Card],
    child: Option<&CardId>,
    parent: Option<&CardId>,
) -> DaemonResult<()> {
    let Some(parent) = parent else {
        return Ok(());
    };
    if child == Some(parent) {
        return Err(BoardError::Invalid {
            field: "parent_id".into(),
            reason: "a card cannot be its own parent".into(),
        }
        .into());
    }
    if !cards.iter().any(|card| card.id == *parent) {
        return Err(BoardError::CardNotFound(parent.to_string()).into());
    }
    // Walk up from the proposed parent: reaching the child would close a cycle.
    let mut seen = std::collections::HashSet::new();
    let mut current = Some(parent.clone());
    while let Some(id) = current {
        if Some(&id) == child {
            return Err(BoardError::Invalid {
                field: "parent_id".into(),
                reason: "a card cannot be its own ancestor".into(),
            }
            .into());
        }
        if !seen.insert(id.clone()) {
            break;
        }
        current = cards
            .iter()
            .find(|card| card.id == id)
            .and_then(|card| card.parent_id.clone());
    }
    Ok(())
}

fn awaiting_push_baseline(card: &Card) -> bool {
    !card.archived
        // The version is the signal: an ack carries the remote's version and nothing else, so a
        // link without one after a push is a card whose baseline was never read back. The
        // stamp beside it is whatever the last pull saw, which says nothing about this push.
        && card
            .remote
            .as_ref()
            .is_some_and(|link| link.version.is_none())
        && card
            .activity
            .iter()
            .rev()
            .find(|activity| {
                activity.kind == ActivityKind::Synced
                    && (activity.message == "Pushed local changes"
                        || activity.message == "Refreshed post-push baseline"
                        || activity.message.starts_with("Synced "))
            })
            .is_some_and(|activity| activity.message == "Pushed local changes")
}

fn require_push_baseline(card: &Card) -> DaemonResult<()> {
    if awaiting_push_baseline(card) {
        return Err(DaemonError::Conflict(
            "sync this board to recover the post-push baseline before editing".into(),
        ));
    }
    Ok(())
}

fn new_card_id() -> DaemonResult<CardId> {
    CardId::try_from(uuid::Uuid::new_v4().to_string())
        .map_err(|error| DaemonError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapters::{Adapters, clock::SystemClock, files::RealFiles},
        services::Services,
        stores::config::ConfigStore,
    };
    use fleet_core::{model::Context, paths::FleetHome, state::default_state};

    async fn fixture() -> (
        tempfile::TempDir,
        Services,
        tokio::sync::broadcast::Receiver<Event>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = FleetHome::new(temp.path());
        let files = Arc::new(RealFiles::new(
            home.trash_dir(),
            [home.boards_dir(), home.repos_dir(), home.worktrees_dir()],
        ));
        let state = Arc::new(StateStore::new(
            temp.path(),
            files.clone(),
            Arc::new(SystemClock),
        ));
        let mut initial = default_state();
        initial.contexts.push(Context {
            id: "work".parse().unwrap(),
            name: "Work".into(),
            owners: vec![],
            created_at: "now".into(),
        });
        state.save(initial).await.unwrap();
        let events = BroadcastBus::default();
        let receiver = events.subscribe();
        let services = Services::build(
            temp.path().to_path_buf(),
            Arc::new(ConfigStore::new(temp.path(), files.clone())),
            state,
            Arc::new(JobManager::new(temp.path())),
            Adapters::system(files),
            events,
        );
        (temp, services, receiver)
    }
    #[tokio::test]
    async fn ensure_is_idempotent_persists_and_emits_once() {
        let (_temp, services, mut receiver) = fixture().await;
        let id = "work".parse().unwrap();
        let (a, b) = tokio::join!(services.boards.ensure(&id), services.boards.ensure(&id));
        let a = a.unwrap();
        assert_eq!(a, b.unwrap());
        assert_eq!(
            receiver.recv().await.unwrap(),
            Event::BoardChanged {
                board_id: a.board.id.clone(),
                reason: BoardChangeReason::Created
            }
        );
        assert!(receiver.try_recv().is_err());
        assert_eq!(services.boards.get(&a.board.id).await.unwrap(), a);
        assert_eq!(services.snapshot().await.unwrap().boards.len(), 1);
        assert!(
            services
                .boards
                .ensure(&"missing".parse().unwrap())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn stale_worktree_links_are_only_cleared_in_views_and_orphans_are_hidden() {
        let (_temp, services, _receiver) = fixture().await;
        let view = services
            .boards
            .ensure(&"work".parse().unwrap())
            .await
            .unwrap();
        let card: Card = serde_json::from_value(serde_json::json!({ "id":"card-1", "boardId":view.board.id, "number":1, "title":"Task", "statusId":"todo", "worktreeId":"acme/api#missing", "createdAt":"now", "updatedAt":"now" })).unwrap();
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board: view.board,
            cards: vec![card],
        };
        services.boards.store.save(&doc).unwrap();
        assert!(
            services.boards.get(&doc.board.id).await.unwrap().cards[0]
                .worktree_id
                .is_none()
        );
        assert!(
            services
                .boards
                .store
                .load(&doc.board.id)
                .unwrap()
                .unwrap()
                .cards[0]
                .worktree_id
                .is_some()
        );
        services
            .state
            .transaction(|state| {
                state.contexts.retain(|c| c.id.as_str() != "work");
                Ok(())
            })
            .await
            .unwrap();
        assert!(services.boards.list(None).await.unwrap().is_empty());
        assert!(services.boards.summaries().await.is_empty());
    }
    #[tokio::test]
    async fn a_renamed_card_reuses_the_worktree_it_is_already_linked_to() {
        let (_temp, services, _receiver) = fixture().await;
        let view = services
            .boards
            .ensure(&"work".parse().unwrap())
            .await
            .unwrap();
        let repo_id: RepoId = "acme/api".parse().unwrap();
        let card = services
            .boards
            .create_card(
                &view.board.id,
                CardDraft {
                    title: "Third".into(),
                    ..CardDraft::default()
                },
            )
            .await
            .unwrap();
        let slug = worktree_slug(&view.board, &card);
        let worktree_id = format!("{repo_id}#{slug}");
        let seed = (worktree_id.clone(), slug.clone());
        services
            .state
            .transaction(move |state| {
                let (worktree_id, slug) = seed.clone();
                state.repos.push(
                    serde_json::from_value(serde_json::json!({
                        "id": "acme/api", "owner": "acme", "name": "api",
                        "url": "https://example.invalid/acme/api.git", "contextId": "work",
                        "defaultBranch": "main", "path": "/tmp/acme-api", "clonedAt": "now"
                    }))
                    .unwrap(),
                );
                state.worktrees.push(
                    serde_json::from_value(serde_json::json!({
                        "id": worktree_id, "repoId": "acme/api", "slug": slug,
                        "branch": slug, "baseRef": "main", "path": "/tmp/acme-api-wt",
                        "session": "s", "createdAt": "now"
                    }))
                    .unwrap(),
                );
                Ok(())
            })
            .await
            .unwrap();
        let (card, worktree, created) = services
            .boards
            .create_worktree_from_card(&card.id, Some(repo_id), None, None)
            .await
            .unwrap();
        // The worktree already existed, so this request adopted it rather than creating one.
        assert!(!created);
        assert_eq!(worktree.id.as_str(), worktree_id);
        assert_eq!(card.worktree_id.as_ref().unwrap().as_str(), worktree_id);
        services
            .boards
            .update_card(
                &card.id,
                CardPatch {
                    title: Some("Third renamed".into()),
                    ..CardPatch::default()
                },
            )
            .await
            .unwrap();
        // The title-derived slug moved, but the stored link is what identifies the worktree.
        let (card, worktree, created) = services
            .boards
            .create_worktree_from_card(&card.id, None, None, None)
            .await
            .unwrap();
        assert!(!created);
        assert_eq!(worktree.id.as_str(), worktree_id);
        assert_eq!(card.worktree_id.as_ref().unwrap().as_str(), worktree_id);
        assert_eq!(services.state.load().await.unwrap().worktrees.len(), 1);
    }

    #[tokio::test]
    async fn deleting_a_context_deletes_the_board_it_owned() {
        let (_temp, services, _receiver) = fixture().await;
        let context: ContextId = "work".parse().unwrap();
        let view = services.boards.ensure(&context).await.unwrap();
        services
            .boards
            .create_card(
                &view.board.id,
                CardDraft {
                    title: "Secret".into(),
                    ..CardDraft::default()
                },
            )
            .await
            .unwrap();
        services
            .dispatch(fleet_proto::request::RequestBody::DeleteContext {
                id: context.clone(),
            })
            .await
            .unwrap();
        // The document is gone, so a later context deriving the same id cannot adopt its cards.
        assert!(
            services
                .boards
                .store
                .load(&view.board.id)
                .unwrap()
                .is_none()
        );
        services
            .dispatch(fleet_proto::request::RequestBody::CreateContext {
                name: "Work".into(),
                owners: vec![],
            })
            .await
            .unwrap();
        assert!(
            services
                .boards
                .ensure(&context)
                .await
                .unwrap()
                .cards
                .is_empty()
        );
    }

    #[tokio::test]
    async fn parents_must_exist_on_the_same_board_and_never_close_a_cycle() {
        let (_temp, services, _receiver) = fixture().await;
        let view = services
            .boards
            .ensure(&"work".parse().unwrap())
            .await
            .unwrap();
        let parent = services
            .boards
            .create_card(
                &view.board.id,
                CardDraft {
                    title: "Parent".into(),
                    ..CardDraft::default()
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            services
                .boards
                .create_card(
                    &view.board.id,
                    CardDraft {
                        title: "Orphan".into(),
                        parent_id: Some("11111111-1111-4111-8111-111111111111".parse().unwrap()),
                        ..CardDraft::default()
                    },
                )
                .await,
            Err(DaemonError::NotFound(_))
        ));
        let child = services
            .boards
            .create_card(
                &view.board.id,
                CardDraft {
                    title: "Child".into(),
                    parent_id: Some(parent.id.clone()),
                    ..CardDraft::default()
                },
            )
            .await
            .unwrap();
        for (card, proposed) in [(&parent.id, &parent.id), (&parent.id, &child.id)] {
            assert!(matches!(
                services
                    .boards
                    .update_card(
                        card,
                        CardPatch {
                            parent_id: Some(Some(proposed.clone())),
                            ..CardPatch::default()
                        },
                    )
                    .await,
                Err(DaemonError::Validation(_))
            ));
        }
    }

    #[tokio::test]
    async fn unregistered_repositories_never_reach_a_board_or_a_card() {
        let (_temp, services, _receiver) = fixture().await;
        let view = services
            .boards
            .ensure(&"work".parse().unwrap())
            .await
            .unwrap();
        let missing: RepoId = "missing/repo".parse().unwrap();
        assert!(
            services
                .boards
                .update(
                    &view.board.id,
                    BoardPatch {
                        default_repo_id: Some(Some(missing.clone())),
                        ..BoardPatch::default()
                    },
                )
                .await
                .is_err()
        );
        assert!(
            services
                .boards
                .create_card(
                    &view.board.id,
                    CardDraft {
                        title: "Task".into(),
                        repo_id: Some(missing.clone()),
                        ..CardDraft::default()
                    },
                )
                .await
                .is_err()
        );
        let card = services
            .boards
            .create_card(
                &view.board.id,
                CardDraft {
                    title: "Task".into(),
                    ..CardDraft::default()
                },
            )
            .await
            .unwrap();
        assert!(
            services
                .boards
                .update_card(
                    &card.id,
                    CardPatch {
                        repo_id: Some(Some(missing)),
                        ..CardPatch::default()
                    },
                )
                .await
                .is_err()
        );
        // A refused reference is never persisted, so `card worktree` cannot inherit it.
        let after = services.boards.get(&view.board.id).await.unwrap();
        assert_eq!(after.board.default_repo_id, None);
        assert_eq!(after.cards.len(), 1);
        assert_eq!(after.cards[0].repo_id, None);
    }

    #[tokio::test]
    async fn damaged_board_does_not_break_snapshot() {
        let (temp, services, _receiver) = fixture().await;
        let view = services
            .boards
            .ensure(&"work".parse().unwrap())
            .await
            .unwrap();
        std::fs::write(
            FleetHome::new(temp.path()).board_path(&view.board.id),
            "broken",
        )
        .unwrap();
        assert!(services.snapshot().await.unwrap().boards.is_empty());
    }
}
