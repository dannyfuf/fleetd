use super::*;

impl Boards {
    /// Loads one document while scanning the store: an unreadable file is skipped rather than
    /// fatal, and is reported once per distinct error instead of on every refresh.
    pub(super) fn scan_load(&self, id: &BoardId) -> Option<BoardDocument> {
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
    pub(super) fn context_board(&self, context: &ContextId) -> DaemonResult<Option<BoardId>> {
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

    pub(super) fn load(&self, id: &BoardId) -> DaemonResult<BoardDocument> {
        self.store
            .load(id)?
            .ok_or_else(|| BoardError::BoardNotFound(id.to_string()).into())
    }

    pub(super) async fn persist(&self, doc: &BoardDocument) -> DaemonResult<()> {
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

    pub(super) async fn save(
        &self,
        doc: &BoardDocument,
        reason: BoardChangeReason,
    ) -> DaemonResult<()> {
        self.persist(doc).await?;
        self.changed(&doc.board.id, reason);
        Ok(())
    }

    pub(super) async fn save_card(
        &self,
        mut doc: BoardDocument,
        index: usize,
        now: &str,
    ) -> DaemonResult<Card> {
        doc.board.updated_at = now.into();
        self.save(&doc, BoardChangeReason::CardChanged).await?;
        self.card_view(&doc.board, &doc.cards[index]).await
    }

    pub(super) async fn card_document(
        &self,
        id: &CardId,
    ) -> DaemonResult<(tokio::sync::OwnedMutexGuard<()>, BoardDocument, usize)> {
        let (guard, doc, index) = self.locked_card_document(id).await?;
        require_push_baseline(&doc.cards[index])?;
        Ok((guard, doc, index))
    }

    /// Locks the card's board, then reloads the document under that lock.
    pub(super) async fn locked_card_document(
        &self,
        id: &CardId,
    ) -> DaemonResult<(tokio::sync::OwnedMutexGuard<()>, BoardDocument, usize)> {
        let (doc, _) = self.find_card_document(id).await?;
        let guard = self.gate(&doc.board.id).await;
        let (doc, index) = self.find_card_document(id).await?;
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

    /// One card as the protocol reports it: a worktree or repository link the state no longer
    /// backs is dropped, exactly as `get` drops it from a whole board.
    pub(super) async fn card_view(&self, board: &Board, card: &Card) -> DaemonResult<Card> {
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

    /// Refuses to create a board over a document this build had to quarantine.
    ///
    /// The board a context owns is named after it, so a fresh one would take the place of the
    /// damaged document and report success. The file is still there, under its `.broken-` name:
    /// naming it is the only way a user learns the board was not empty.
    pub(super) fn refuse_over_quarantine(&self, id: &BoardId) -> DaemonResult<()> {
        if let Some(path) = self.store.quarantined(id)?.first() {
            return Err(DaemonError::Conflict(format!(
                "board {id} has a quarantined document at {}; restore or remove it before creating a board",
                path.display()
            )));
        }
        Ok(())
    }
}
