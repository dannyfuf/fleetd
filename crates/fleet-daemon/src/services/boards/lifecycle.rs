use super::*;

const MAX_BOARD_ID_SUFFIX: u32 = 99;

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
            // Local state only, and the mirror is never consulted: a worktree board lives on
            // the daemon that owns the worktree, so a document scoped to a worktree this
            // daemon never published is either an orphan or the stale copy an older build
            // made before the request was routed to its owner (`docs/BOARD.md` §4).
            if context.is_some_and(|id| *id != doc.board.context_id)
                || !state.contexts.iter().any(|c| c.id == doc.board.context_id)
                || doc.board.worktree_id.as_ref().is_some_and(|worktree| {
                    !state.worktrees.iter().any(|item| item.id == *worktree)
                })
            {
                continue;
            }
            self.index
                .write()
                .await
                .extend(doc.cards.iter().map(|c| (c.id.clone(), id.clone())));
            summaries.push(summarize(&doc.board, &doc.cards, &[], &self.now()));
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
        // A *card* may link a worktree another host owns, so the scrub below reads the mirror
        // too; the board's own scope is a local question and is not checked here.
        let mirrored = self.mirrored_worktrees();
        let context = doc.board.context_id.clone();
        scrub_repo(&state, Some(&context), &mut doc.board.default_repo_id);
        let card_context = card_repo_context(&doc.board).cloned();
        let mut index = self.index.write().await;
        index.retain(|_, board| board != id);
        for card in &mut doc.cards {
            index.insert(card.id.clone(), id.clone());
            if card
                .worktree_id
                .as_ref()
                .is_some_and(|id| !worktree_exists(&state, &mirrored, id))
            {
                card.worktree_id = None;
            }
            scrub_repo(&state, card_context.as_ref(), &mut card.repo_id);
        }
        // The card index is not held across the delegation read: the join asks another service.
        drop(index);
        let live_runs = self.live_runs(id).await;
        Ok(BoardView {
            board: doc.board,
            cards: doc.cards,
            live_runs,
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
        let backend = self.backends.get(&board.backend.kind)?;
        backend.validate(&board.backend.settings).await?;
        board.backend.settings = backend.normalize(&board.backend.settings).await?;
        let _allocation = self.allocation.lock().await;
        board.id = self.available_board_id(&board.id)?;
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: Vec::new(),
        };
        self.save(&doc, BoardChangeReason::Created).await?;
        Ok(BoardView {
            board: doc.board,
            cards: doc.cards,
            // A board created by this call has no cards, so nothing can have called a run.
            // Every *existing*-board path above returns through `get`, which joins them.
            live_runs: Vec::new(),
        })
    }

    /// Gets or creates the context's Reviews board; unknown contexts are errors.
    ///
    /// `ensure` line for line, over the Reviews scope: the context's task board is neither read
    /// nor touched, and deleting the context deletes this board with it (`delete_for_context`).
    pub async fn ensure_reviews(&self, context: &ContextId) -> DaemonResult<BoardView> {
        let state = self.state_store.load().await?;
        let context_record = state
            .contexts
            .iter()
            .find(|c| c.id == *context)
            .ok_or_else(|| DaemonError::NotFound(format!("context {context}")))?
            .clone();
        // The refresh path after every BoardChanged, as for `ensure`: an existing board is read
        // without a lock.
        if let Some(id) = self.reviews_board(context)? {
            return self.get(&id).await;
        }
        let mut board = new_reviews_board(&context_record, &self.clock.now().to_rfc3339());
        let _guard = self.gate(&board.id).await;
        if let Some(id) = self.reviews_board(context)? {
            return self.get(&id).await;
        }
        let backend = self.backends.get(&board.backend.kind)?;
        backend.validate(&board.backend.settings).await?;
        board.backend.settings = backend.normalize(&board.backend.settings).await?;
        let _allocation = self.allocation.lock().await;
        board.id = self.available_board_id(&board.id)?;
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: Vec::new(),
        };
        self.save(&doc, BoardChangeReason::Created).await?;
        Ok(BoardView {
            board: doc.board,
            cards: doc.cards,
            // A board created by this call has no cards, so nothing can have called a run.
            live_runs: Vec::new(),
        })
    }

    /// Gets or creates the board scoped to one worktree this daemon published.
    pub async fn ensure_for_worktree(&self, worktree: &WorktreeId) -> DaemonResult<BoardView> {
        let state = self.state_store.load().await?;
        worktree_context(&state, worktree)?;
        // Existing-board refreshes stay lock-free, as context-board refreshes do.
        if let Some(id) = self.worktree_board(worktree)? {
            return self.get(&id).await;
        }
        // Materialization shares the lifecycle claim used by delete and restore. Otherwise a
        // missing-board read can race past a completed cascade and save an orphan, or create an
        // empty document before restore has returned the archived board and its cards.
        let _lifecycle = self.worktrees.claim_lifecycle(worktree.clone()).await;
        let state = self.state_store.load().await?;
        let (worktree_record, context) = worktree_context(&state, worktree)?;
        if let Some(id) = self.worktree_board(worktree)? {
            return self.get(&id).await;
        }
        let base = worktree_board_id(worktree);
        let _guard = self.gate(&base).await;
        if let Some(id) = self.worktree_board(worktree)? {
            return self.get(&id).await;
        }
        let mut board = new_worktree_board(&context, &worktree_record, &self.now());
        let backend = self.backends.get(&board.backend.kind)?;
        backend.validate(&board.backend.settings).await?;
        board.backend.settings = backend.normalize(&board.backend.settings).await?;
        let _allocation = self.allocation.lock().await;
        board.id = self.available_board_id(&base)?;
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: Vec::new(),
        };
        self.save(&doc, BoardChangeReason::Created).await?;
        Ok(BoardView {
            board: doc.board,
            cards: doc.cards,
            // A board created by this call has no cards, so nothing can have called a run.
            // Every *existing*-board path above returns through `get`, which joins them.
            live_runs: Vec::new(),
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
                    let summary = summarize(&doc.board, &doc.cards, &[], &self.now());
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
                // Local state only, as in `list`: a board scoped elsewhere is not this
                // daemon's to publish.
                && summary
                    .worktree_id
                    .as_ref()
                    .is_none_or(|worktree| state.worktrees.iter().any(|item| item.id == *worktree))
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
        let _allocation = self.allocation.lock().await;
        board.id = self.available_board_id(&board.id)?;
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: Vec::new(),
        };
        self.save(&doc, BoardChangeReason::Created).await?;
        self.get(&doc.board.id).await
    }

    /// Creates the worktree's only board after validating its backend configuration.
    pub async fn create_for_worktree(
        &self,
        worktree: &WorktreeId,
        name: Option<String>,
        prefix: Option<String>,
        backend: Option<BackendRef>,
    ) -> DaemonResult<BoardView> {
        let _lifecycle = self.worktrees.claim_lifecycle(worktree.clone()).await;
        let state = self.state_store.load().await?;
        let (worktree_record, context) = worktree_context(&state, worktree)?;
        let now = self.now();
        let mut board = new_worktree_board(&context, &worktree_record, &now);
        let base = board.id.clone();
        let _guard = self.gate(&base).await;
        if self.worktree_board(worktree)?.is_some() {
            return Err(BoardError::Duplicate(worktree.to_string()).into());
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
        let _allocation = self.allocation.lock().await;
        board.id = self.available_board_id(&base)?;
        let doc = BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board,
            cards: Vec::new(),
        };
        self.save(&doc, BoardChangeReason::Created).await?;
        self.get(&doc.board.id).await
    }

    fn available_board_id(&self, base: &BoardId) -> DaemonResult<BoardId> {
        if self.board_id_is_available(base)? {
            return Ok(base.clone());
        }
        for suffix in 2..=MAX_BOARD_ID_SUFFIX {
            let candidate = suffixed_board_id(base, suffix);
            if self.board_id_is_available(&candidate)? {
                return Ok(candidate);
            }
        }
        Err(DaemonError::Conflict(format!(
            "no board id is available for {base}"
        )))
    }

    fn board_id_is_available(&self, id: &BoardId) -> DaemonResult<bool> {
        self.refuse_over_quarantine(id)?;
        Ok(self.store.load(id)?.is_none())
    }

    /// Moves every board scoped through `repo` before publishing its new context in state.
    async fn move_repo_to_context(
        &self,
        repo: RepoId,
        context: ContextId,
    ) -> DaemonResult<fleet_core::model::Repo> {
        let initial = self.state_store.load().await?;
        if !initial.contexts.iter().any(|item| item.id == context) {
            return Err(DaemonError::NotFound(format!("context {context}")));
        }
        initial
            .repos
            .iter()
            .find(|item| item.id == repo)
            .ok_or_else(|| DaemonError::NotFound(format!("repository {repo}")))?;

        // Worktree-board creation uses the same claims. Re-read until every worktree published
        // for this repository is covered, then no scoped board can appear during the move.
        let mut claimed_ids = Vec::<WorktreeId>::new();
        let mut lifecycle_claims = Vec::new();
        loop {
            let state = self.state_store.load().await?;
            let mut unclaimed = state
                .worktrees
                .iter()
                .filter(|worktree| worktree.repo_id == repo && !claimed_ids.contains(&worktree.id))
                .map(|worktree| worktree.id.clone())
                .collect::<Vec<_>>();
            unclaimed.sort();
            if unclaimed.is_empty() {
                break;
            }
            for worktree in unclaimed {
                lifecycle_claims.push(self.worktrees.claim_lifecycle(worktree.clone()).await);
                claimed_ids.push(worktree);
            }
        }

        let ids = self.store.list()?;
        let mut gates = Vec::with_capacity(ids.len());
        for id in &ids {
            gates.push(self.gate(id).await);
        }
        let mut originals = Vec::new();
        for id in ids {
            let Some(mut doc) = self.store.peek(&id)? else {
                continue;
            };
            if doc
                .board
                .worktree_id
                .as_ref()
                .is_some_and(|worktree| claimed_ids.contains(worktree))
            {
                originals.push(doc.clone());
                doc.board.context_id = context.clone();
                doc.board.updated_at = self.now();
                if let Err(error) = self.save(&doc, BoardChangeReason::Updated).await {
                    self.rollback_context_moves(&originals).await?;
                    return Err(error);
                }
            }
        }

        let repo_for_transaction = repo.clone();
        let context_for_transaction = context.clone();
        let moved = self
            .state_store
            .transaction(move |state| {
                if !state
                    .contexts
                    .iter()
                    .any(|item| item.id == context_for_transaction)
                {
                    return Err(DaemonError::NotFound(format!(
                        "context {context_for_transaction}"
                    )));
                }
                let item = state
                    .repos
                    .iter_mut()
                    .find(|item| item.id == repo_for_transaction)
                    .ok_or_else(|| {
                        DaemonError::NotFound(format!("repository {repo_for_transaction}"))
                    })?;
                item.context_id = context_for_transaction;
                Ok(item.clone())
            })
            .await;
        match moved {
            Ok(repo) => Ok(repo),
            Err(error) => {
                self.rollback_context_moves(&originals).await?;
                Err(error)
            }
        }
    }

    async fn rollback_context_moves(&self, originals: &[BoardDocument]) -> DaemonResult<()> {
        for doc in originals.iter().rev() {
            self.save(doc, BoardChangeReason::Updated).await?;
        }
        Ok(())
    }

    /// Updates board configuration, rejecting removal of referenced statuses or labels.
    ///
    /// A patch that changes the backend **kind** takes its settings from the patch alone: the
    /// previous kind's settings name a project on another system, and everything the old
    /// backend left behind — cursor, status map, read-only fields — describes a remote this
    /// board no longer talks to. Both are dropped before the patch is applied, so a client
    /// that echoes back the settings it was showing cannot smuggle them into the new kind.
    pub async fn update(&self, id: &BoardId, patch: BoardPatch) -> DaemonResult<BoardView> {
        let guard = self.gate(id).await;
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
        // The columns as they were, so what the patch did to them can be read off afterwards:
        // which went away, which lost their action, and which changed category.
        let before = doc.board.statuses.clone();
        let settings_before = doc.board.settings.clone();
        let now = self.now();
        if !apply_board_patch(&mut doc.board, patch, &now)? {
            // A patch that changes nothing writes nothing and announces nothing, exactly as an
            // empty card patch and a no-op move do.
            return self.get(id).await;
        }
        // Automation is a worktree, local, host-free affair (contracts §1.7). The three rules
        // are checked on what the patch produced, and only when the patch asked for automation
        // at all: a board whose worktree a host adopted afterwards must stay renamable, and
        // must stay able to give its automation up.
        if asks_for_automation(&before, &settings_before, &doc.board) {
            self.require_automatable(&doc.board).await?;
        }
        // A column a run started in cannot be taken away under it: the run's own row names that
        // column, and nothing left on the card would say where the work is happening. The
        // refusal comes before the in-use check below, which would otherwise answer the same
        // situation with the column's cards instead of its runs.
        if let Some(live) = removed_column_live_runs(&before, &doc.board, &doc.cards) {
            return Err(DaemonError::Conflict(format!(
                "column has {live} live runs; cancel them first"
            )));
        }
        // A card parked for a column that no longer runs anything is waiting for a run that can
        // never come, so the park goes and the activity says why.
        clear_stranded_pending_runs(&before, &doc.board, &mut doc.cards, &now);
        // The seeds this patch owes the engine: a column's category is what makes the cards in
        // it satisfy their dependants, so changing it can release work anywhere on the board.
        let seeds = recategorised_cards(&before, &doc.board, &doc.cards);
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
        // The same walk every card-writing trigger site makes, over the board's own reservation.
        // The tail is spelled out here rather than shared with `cards::commit` because this path
        // saves with `Updated` rather than `CardChanged`: the moves and the parks land in
        // `doc.cards` in memory, and the starts are handed back for after the gate.
        let plan = self
            .evaluate_with_reservation(&mut doc, &seeds, &now)
            .await?;
        self.save(&doc, BoardChangeReason::Updated).await?;
        self.note_pending(
            &doc.board.id,
            doc.cards.iter().any(|card| card.pending_run.is_some()),
        )
        .await;
        // `start_for_card` re-acquires this same gate to record what the delegation service
        // answered, and the gates are not reentrant (`docs/BOARD.md` §4).
        drop(guard);
        // Handed off on a card-worktree board, as a card write's starts are: a pull-request
        // fetch must not hold this request past the client's timeout.
        self.apply_starts_after_answer(&doc.board, plan).await?;
        self.get(id).await
    }

    /// Refuses column automation on a board that cannot run it (contracts §1.7).
    ///
    /// Three rules, in the order a person meets them: a run happens *in* a worktree, it is
    /// started by this daemon's own agents, and it must never be started for a tree another
    /// host owns. The same three run again when a card enters an action column, because a
    /// worktree can be adopted by a host long after its board was configured.
    ///
    /// A board that runs each card in the card's own worktree (`RunLocation::CardWorktree`)
    /// needs no worktree of its own, so the first rule is the board-worktree rule only; and
    /// the host rule is asked here only of a board worktree, because each card's worktree can
    /// live somewhere else and the start path asks it of that one.
    ///
    /// # Errors
    ///
    /// `Validation` carrying whichever of the three sentences applies.
    pub(crate) async fn require_automatable(&self, board: &Board) -> DaemonResult<()> {
        if board.settings.run_location.is_board_worktree() && board.worktree_id.is_none() {
            return Err(refused_automation(
                "automation is available on worktree boards only",
            ));
        }
        if !board.backend.is_local() {
            return Err(refused_automation(
                "automation is available on local boards only",
            ));
        }
        let Some(worktree) = board.worktree_id.as_ref() else {
            return Ok(());
        };
        let state = self.state_store.load().await?;
        // The mirror counts here, unlike board *scope*: a worktree this daemon published and one
        // it merely mirrors are both answers to "who owns the tree this run would touch".
        if let Some(host) = self
            .known_worktree(&state, worktree)
            .and_then(|worktree| worktree.host)
        {
            return Err(refused_automation(&format!(
                "automation is unavailable on a worktree owned by host {host}"
            )));
        }
        Ok(())
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
    ///
    /// Every board scoped to the context goes, whatever its kind: the task board and the
    /// Reviews board alike. Returns the boards it deleted, so the caller can cascade what hangs
    /// off a board without `Boards` depending on it (a board's schedules).
    pub async fn delete_for_context(&self, context: &ContextId) -> DaemonResult<Vec<BoardId>> {
        let mut deleted = Vec::new();
        for id in self.store.list()? {
            let owned = match self.store.peek(&id) {
                Ok(Some(doc)) => doc.board.context_id == *context,
                Ok(None) => false,
                Err(error) => {
                    // Board ids are shared across context and worktree scopes, so a filename
                    // cannot prove ownership when the persisted scope is unreadable.
                    tracing::warn!(%context, %id, %error, "preserving unreadable board whose context ownership cannot be verified");
                    false
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
                deleted.push(id);
            }
        }
        Ok(deleted)
    }

    /// Deletes every board scoped to `worktree` after that worktree is removed.
    ///
    /// Returns the boards it deleted, so the caller can cascade what hangs off a board without
    /// `Boards` depending on it (a board's schedules), exactly as `delete_for_context` does.
    pub async fn delete_for_worktree(
        &self,
        worktree: &WorktreeId,
        trash: &std::path::Path,
    ) -> DaemonResult<Vec<BoardId>> {
        let mut deleted = Vec::new();
        for id in self.store.list()? {
            let owned = match self.store.peek(&id) {
                Ok(Some(doc)) => doc.board.worktree_id.as_ref() == Some(worktree),
                Ok(None) => false,
                Err(error) => {
                    // Derived ids are shared with context boards, so the filename cannot prove
                    // ownership when the persisted worktree scope is unreadable.
                    tracing::warn!(%worktree, %id, %error, "preserving unreadable board whose worktree ownership cannot be verified");
                    false
                }
            };
            if owned {
                // Not `delete`: an unreadable board must not block deletion of a worktree that
                // has already left state and disk.
                let _guard = self.gate(&id).await;
                self.store.delete_with_worktree(&id, trash)?;
                self.summaries.write().await.remove(&id);
                self.index.write().await.retain(|_, board| *board != id);
                self.changed(&id, BoardChangeReason::Deleted);
                deleted.push(id);
            }
        }
        Ok(deleted)
    }

    /// Moves this daemon's own document for a worktree another host owns into the trash.
    ///
    /// A worktree board lives on the daemon that owns the worktree (`docs/BOARD.md` §4), so a
    /// document found here for a hosted worktree is the copy an older build created before board
    /// requests were routed to their owner. Left in place it shadows the real board — the derived
    /// id is the same on both hosts — and it is trashed rather than merged, because nothing here
    /// can tell which side of a divergence is the user's work. No `BoardChanged` is published:
    /// the request this retirement runs under is about to return the *host's* board under that
    /// same id, and a `Deleted` event would tell the app to drop the board it just opened.
    pub async fn retire_hosted_worktree_board(
        &self,
        worktree: &WorktreeId,
        host: &HostId,
    ) -> DaemonResult<Option<RetiredBoard>> {
        let Some(board) = self.worktree_board(worktree)? else {
            return Ok(None);
        };
        let _guard = self.gate(&board).await;
        // Re-read under the gate and by persisted scope: the id above came from a lock-free
        // scan, and only the document itself proves which worktree it belongs to. `peek`, so a
        // damaged file is reported instead of being quarantined and then retired as empty.
        let Some(doc) = self.store.peek(&board)? else {
            return Ok(None);
        };
        if doc.board.worktree_id.as_ref() != Some(worktree) {
            return Ok(None);
        }
        let cards = doc.cards.len();
        let trashed = self.store.delete_reporting(&board)?;
        self.summaries.write().await.remove(&board);
        self.index.write().await.retain(|_, owner| *owner != board);
        let Some(path) = trashed.last().cloned() else {
            // Nothing was left to move: the file went away between the read above and the
            // rename, so there is no trashed document to report or to name in the log.
            return Ok(None);
        };
        if cards == 0 {
            tracing::info!(%worktree, %host, %board, path = %path.display(), "retired this daemon's empty board for a worktree another host owns");
        } else {
            tracing::warn!(%worktree, %host, %board, cards, path = %path.display(), "retired this daemon's board for a worktree another host owns; its cards must be re-created by hand on that host");
        }
        Ok(Some(RetiredBoard { board, path, cards }))
    }

    /// Restores a worktree board bundled into the restored worktree directory.
    pub async fn restore_for_worktree(
        &self,
        worktree: &WorktreeId,
        destination: &std::path::Path,
    ) -> DaemonResult<()> {
        for id in self.store.restore_with_worktree(destination)? {
            let doc = match self.store.peek(&id) {
                Ok(Some(doc)) => doc,
                Ok(None) => continue,
                Err(error) => {
                    // Recovery is a read: preserve an unreadable document in place for a build
                    // that can read it or for manual repair instead of quarantining it again.
                    tracing::warn!(%worktree, %id, %error, "restored unreadable board without validating its scope");
                    continue;
                }
            };
            if doc.board.worktree_id.as_ref() != Some(worktree) {
                return Err(DaemonError::Conflict(format!(
                    "restored board {id} does not belong to worktree {worktree}"
                )));
            }
            self.summaries.write().await.remove(&id);
            self.index
                .write()
                .await
                .extend(doc.cards.iter().map(|card| (card.id.clone(), id.clone())));
            self.changed(&id, BoardChangeReason::Created);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl WorktreeCascade for Boards {
    async fn delete_for_worktree(
        &self,
        worktree: &WorktreeId,
        trash: &std::path::Path,
    ) -> DaemonResult<()> {
        Boards::delete_for_worktree(self, worktree, trash)
            .await
            .map(drop)
    }

    async fn restore_for_worktree(
        &self,
        worktree: &WorktreeId,
        destination: &std::path::Path,
    ) -> DaemonResult<()> {
        Boards::restore_for_worktree(self, worktree, destination).await
    }
}

#[async_trait::async_trait]
impl RepoContextMover for Boards {
    async fn move_repo_to_context(
        &self,
        repo: RepoId,
        context: ContextId,
    ) -> DaemonResult<fleet_core::model::Repo> {
        Boards::move_repo_to_context(self, repo, context).await
    }
}

/// One of the three `automation` refusals of contracts §1.7, in the words every surface prints.
fn refused_automation(reason: &str) -> DaemonError {
    BoardError::Invalid {
        field: "automation".into(),
        reason: reason.into(),
    }
    .into()
}

/// Whether the patch turned automation on, or turned it up, rather than leaving it alone.
///
/// Only a patch that asks for automation is held to the three rules. Holding every patch to
/// them would strand a board whose worktree a host adopted after the fact: it could no longer
/// be renamed, and — worse — its automation could no longer be taken off.
///
/// Moving where an automated board runs is asking for automation too: a context board that
/// automates in its cards' worktrees and is patched back to its own worktree would otherwise
/// keep columns that can never run.
fn asks_for_automation(before: &[Status], settings_before: &BoardSettings, board: &Board) -> bool {
    let settings = &board.settings;
    if settings.max_live_runs.is_some() && settings.max_live_runs != settings_before.max_live_runs {
        return true;
    }
    if settings.run_location != settings_before.run_location
        && board
            .statuses
            .iter()
            .any(|status| status.automation.is_some())
    {
        return true;
    }
    board.statuses.iter().any(|status| {
        status.automation.is_some()
            && before
                .iter()
                .find(|old| old.id == status.id)
                .is_none_or(|old| old.automation != status.automation)
    })
}

/// How many live runs the first column this patch removes still owns, when it owns any.
///
/// A run records the column it started in, so a column with live runs cannot be taken away: the
/// run would outlive the only thing that says what it is doing.
fn removed_column_live_runs(before: &[Status], board: &Board, cards: &[Card]) -> Option<usize> {
    before
        .iter()
        .filter(|old| !board.statuses.iter().any(|status| status.id == old.id))
        .map(|removed| {
            cards
                .iter()
                .filter(|card| {
                    latest_run(card).is_some_and(|run| run.is_live() && run.status_id == removed.id)
                })
                .count()
        })
        .find(|live| *live > 0)
}

/// Clears every park left waiting on a column that no longer runs anything.
///
/// A `pending_run` is a promise that a slot will start this card where it stands. A column that
/// lost its action — or that the patch removed outright — can never keep it, and a card left
/// parked against one waits for a run nothing will ever decide to start.
fn clear_stranded_pending_runs(before: &[Status], board: &Board, cards: &mut [Card], now: &str) {
    for card in cards {
        let Some(pending) = card.pending_run.as_ref() else {
            continue;
        };
        let column = board
            .statuses
            .iter()
            .find(|status| status.id == pending.status_id);
        if column.is_some_and(|status| {
            status
                .automation
                .as_ref()
                .is_some_and(|automation| automation.on_enter.is_some())
        }) {
            continue;
        }
        // The removed column's own name, taken from the columns as they were: the activity is
        // read by someone asking what happened to their card, and a status id would not say.
        let name = column
            .or_else(|| before.iter().find(|status| status.id == pending.status_id))
            .map_or_else(
                || pending.status_id.to_string(),
                |status| status.name.clone(),
            );
        card.pending_run = None;
        push_activity(
            card,
            ActivityKind::Updated,
            None,
            format!("Run canceled: {name} no longer runs an action"),
            now,
        );
    }
}

/// The cards sitting in a column whose category the patch changed.
///
/// Category is what makes a card satisfy its dependants, so a column that becomes (or stops
/// being) `Completed` can release work anywhere on the board: every card in it is a seed.
fn recategorised_cards(before: &[Status], board: &Board, cards: &[Card]) -> Vec<CardId> {
    let changed: Vec<&StatusId> = board
        .statuses
        .iter()
        .filter(|status| {
            before
                .iter()
                .any(|old| old.id == status.id && old.category != status.category)
        })
        .map(|status| &status.id)
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }
    cards
        .iter()
        .filter(|card| !card.archived && changed.iter().any(|id| **id == card.status_id))
        .map(|card| card.id.clone())
        .collect()
}

/// Resolves the worktree a board is scoped to, and the context that board belongs to.
///
/// This daemon's state is the only place it looks: a worktree board lives on the daemon that
/// owns the worktree, so a worktree this daemon never published is `not found: worktree <id>`
/// here and the request belongs to that worktree's owner (`docs/BOARD.md` §4).
fn worktree_context(state: &State, id: &WorktreeId) -> DaemonResult<(Worktree, Context)> {
    let worktree = state
        .worktrees
        .iter()
        .find(|worktree| worktree.id == *id)
        .cloned()
        .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
    let repo = state
        .repos
        .iter()
        .find(|repo| repo.id == worktree.repo_id)
        .ok_or_else(|| DaemonError::NotFound(format!("repository {}", worktree.repo_id)))?;
    let context = state
        .contexts
        .iter()
        .find(|context| context.id == repo.context_id)
        .cloned()
        .ok_or_else(|| DaemonError::NotFound(format!("context {}", repo.context_id)))?;
    Ok((worktree, context))
}

fn suffixed_board_id(base: &BoardId, suffix: u32) -> BoardId {
    let suffix = format!("-{suffix}");
    let keep = BOARD_ID_MAX_LEN
        .saturating_sub(suffix.len())
        .min(base.as_str().len());
    let mut stem = base.as_str()[..keep].trim_end_matches('-').to_owned();
    stem.push_str(&suffix);
    BoardId::try_from(stem).expect("a suffixed worktree board id is always a valid board slug")
}

#[cfg(test)]
pub(super) mod tests;
