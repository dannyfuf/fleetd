use super::*;

impl Boards {
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
}

/// Rejects a parent that is missing, on another board, the card itself, or its own descendant.
pub(super) fn validate_parent(
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

pub(super) fn awaiting_push_baseline(card: &Card) -> bool {
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

pub(super) fn require_push_baseline(card: &Card) -> DaemonResult<()> {
    if awaiting_push_baseline(card) {
        return Err(DaemonError::Conflict(
            "sync this board to recover the post-push baseline before editing".into(),
        ));
    }
    Ok(())
}

pub(super) fn new_card_id() -> DaemonResult<CardId> {
    CardId::try_from(uuid::Uuid::new_v4().to_string())
        .map_err(|error| DaemonError::Validation(error.to_string()))
}
