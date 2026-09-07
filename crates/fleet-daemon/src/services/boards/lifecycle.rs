use super::*;

impl Boards {
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
}
