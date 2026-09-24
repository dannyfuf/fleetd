use super::*;

#[cfg(test)]
mod tests;

impl Boards {
    /// Allocates a UUID and local number, then persists a validated card.
    pub async fn create_card(&self, board: &BoardId, draft: CardDraft) -> DaemonResult<Card> {
        let guard = self.gate(board).await;
        let mut doc = self.load(board)?;
        self.validate_repo(&doc.board, draft.repo_id.as_ref())
            .await?;
        validate_parent(&doc.cards, None, draft.parent_id.as_ref())?;
        let now = self.now();
        ops::check_draft_writable(&doc.board, &draft)?;
        let card = ops::create_card(&mut doc.board, &doc.cards, new_card_id()?, draft, &now)?;
        // Links are checked against the set the card is about to join: `validate_card`, which
        // every save runs, sees one card and so cannot tell whether a blocker exists at all.
        validate_links(&doc.board, &doc.cards, &card)?;
        // A card created straight into a column that runs something starts it, exactly as a move
        // into that column would: the column is what runs, not the gesture that put the card there.
        let seeds = [card.id.clone()];
        let index = doc.cards.len();
        doc.cards.push(card);
        self.commit(guard, &mut doc, &seeds, &now).await?;
        Ok(doc.cards[index].clone())
    }

    /// Creates a pull request card, or answers the card already tracking that pull request.
    ///
    /// The whole read-decide-write runs under the board gate, so two callers upserting the same
    /// pull request at once are serialised: the second sees the first one's card and answers
    /// `Existing`. An `Existing` answer changed nothing, so it writes nothing and announces
    /// nothing. A created or reopened card is seeded exactly as `create_card` seeds one: the
    /// column it lands in decides what runs, with no special case for any column.
    ///
    /// On a Reviews board the pull request's repository may belong to any context, so the
    /// draft's repository is not validated there. It is set to the pull request's repository
    /// when that is a Fleet repository and left empty otherwise: the card is still created and
    /// visible, and its run refuses later with a sentence that names the repository.
    ///
    /// # Errors
    ///
    /// A draft with no pull request, an unknown board, a refused draft field, a link that
    /// would close a cycle, or the failure of the save or of the first run it started.
    pub async fn upsert_pull_request_card(
        &self,
        board: &BoardId,
        mut draft: CardDraft,
        requested_at: Option<String>,
    ) -> DaemonResult<(Card, UpsertOutcome)> {
        let guard = self.gate(board).await;
        let mut doc = self.load(board)?;
        // A card whose run is still going — or still starting, which on a card-worktree board
        // can take the length of a pull-request fetch — is not reopened under it: the run would
        // report into a column the card has left, exactly the wound `move_card` refuses. The
        // request stands as it is, and the next upsert after the run ends compares again.
        if let Some(pull_request) = &draft.pull_request
            && let Some(held) = doc.cards.iter().find(|card| {
                card.board_id == doc.board.id
                    && card
                        .pull_request
                        .as_ref()
                        .is_some_and(|held| held.same_pull_request(pull_request))
            })
            && (is_working(held) || self.start_in_flight(&doc.board.id, &held.id).await)
        {
            return Ok((held.clone(), UpsertOutcome::Existing));
        }
        if doc.board.kind.is_tasks() {
            self.validate_repo(&doc.board, draft.repo_id.as_ref())
                .await?;
        } else if let Some(pull_request) = &draft.pull_request {
            // Matched without case, as the pull request itself is, and linked under the id Fleet
            // registered it with.
            draft.repo_id = self
                .state_store
                .load()
                .await?
                .repos
                .iter()
                .find(|fleet| {
                    fleet
                        .id
                        .as_str()
                        .eq_ignore_ascii_case(pull_request.repo.as_str())
                })
                .map(|fleet| fleet.id.clone());
        }
        validate_parent(&doc.cards, None, draft.parent_id.as_ref())?;
        let now = self.now();
        ops::check_draft_writable(&doc.board, &draft)?;
        let (outcome, index) = ops::upsert_pull_request_card(
            &mut doc.board,
            &mut doc.cards,
            new_card_id()?,
            draft,
            requested_at.as_deref(),
            &now,
        )?;
        if outcome == UpsertOutcome::Existing {
            return Ok((doc.cards[index].clone(), outcome));
        }
        // Checked against the set the card now stands in, for the reason `create_card` gives.
        validate_links(&doc.board, &doc.cards, &doc.cards[index])?;
        if outcome == UpsertOutcome::Reopened {
            // A run owed to the column the card has left is not owed any more; the reopen's own
            // activity entry already says what happened.
            doc.cards[index].pending_run = None;
            doc.board.updated_at.clone_from(&now);
        }
        let seeds = [doc.cards[index].id.clone()];
        self.commit(guard, &mut doc, &seeds, &now).await?;
        Ok((doc.cards[index].clone(), outcome))
    }

    /// Applies a partial card edit through the pure domain operations.
    pub async fn update_card(&self, card: &CardId, patch: CardPatch) -> DaemonResult<Card> {
        let (guard, mut doc, index) = self.card_document(card).await?;
        let now = self.now();
        // A working card cannot be archived out from under its run: the run would carry on with
        // no column left to report into, and the board would show neither. Cancel it first.
        if patch.archived == Some(true) && is_working(&doc.cards[index]) {
            return Err(DaemonError::Conflict(format!(
                "{} is working; cancel the run first",
                doc.cards[index].display_key(&doc.board)
            )));
        }
        self.validate_repo(&doc.board, patch.repo_id.as_ref().and_then(Option::as_ref))
            .await?;
        // A card that owns a worktree keeps the repository that worktree lives in. Clearing it
        // would leave `Repo: —` next to a live worktree, and pointing it at a different
        // repository is the same wound with a name on it: `create_worktree_from_card` then
        // refuses to adopt or recreate the worktree, and this path refuses to clear it, so the
        // card can never reach any repository again. Only the worktree's own repository passes.
        // A worktree another host owns counts as live here exactly as a local one does: the card
        // links it through the same field, and clearing its repository strands it the same way.
        if let Some(next) = patch.repo_id.as_ref()
            && let Some(worktree) = doc.cards[index].worktree_id.clone()
            && let Some(live) = self.known_worktree(&self.state_store.load().await?, &worktree)
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
        validate_links(&doc.board, &doc.cards, &doc.cards[index])?;
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
        // The two edits that can change what the board owes: the card may have entered a column
        // that runs something, and either way its dependants may now be free to advance.
        let seeded = changed
            .iter()
            .any(|field| field == "status_id" || field == "archived");
        if seeded {
            // A run owed to the column the card has left is not owed any more. Nothing is
            // announced: the `Updated` entry the patch just wrote already says what happened.
            doc.cards[index].pending_run = None;
        }
        let seeds = if seeded {
            vec![card.clone()]
        } else {
            Vec::new()
        };
        doc.board.updated_at.clone_from(&now);
        self.commit(guard, &mut doc, &seeds, &now).await?;
        self.card_view(&doc.board, &doc.cards[index]).await
    }

    /// Moves a card and persists all affected column positions atomically.
    pub async fn move_card(
        &self,
        card: &CardId,
        status: &StatusId,
        index: Option<usize>,
        cancel_run: bool,
    ) -> DaemonResult<Card> {
        let (mut guard, mut doc, mut card_index) = self.card_document(card).await?;
        // A live run is settled before the move, and never under this gate: `cancel_run` takes
        // the same board gate itself, and the gates are not reentrant.
        if is_working(&doc.cards[card_index]) {
            let key = doc.cards[card_index].display_key(&doc.board);
            let cancelled = latest_run(&doc.cards[card_index]).map(|run| run.id);
            drop(guard);
            if !cancel_run {
                return Err(DaemonError::Conflict(format!(
                    "{key} is working; pass --cancel-run to move it"
                )));
            }
            // With no automation nothing can be running: the row is what a previous daemon left
            // behind, and the flag is the user saying to move the card regardless.
            if self.automation().is_some() {
                self.cancel_run(card).await?;
            }
            (guard, doc, card_index) = self.card_document(card).await?;
            // The gate was down for the length of the cancel, and the card can have gained a
            // *different* live run in that window — a concurrent `card run`, or a cascade that
            // reached it once the cancel landed. The cancelled run is still live here as often
            // as not (its `Cancelled` arrives with the delivery), so the id is what tells the
            // two apart: moving on a run nobody asked to cancel would leave a live child
            // reporting into a column the card has left.
            if super::automation::live_run(&doc.cards[card_index])
                .is_some_and(|live| Some(live) != cancelled)
            {
                return Err(DaemonError::Conflict(format!(
                    "{key} is working; cancel the run first"
                )));
            }
        }
        let now = self.now();
        // A move that lands where the card already was writes nothing and announces nothing,
        // exactly as an empty patch does.
        if !ops::move_card(&doc.board, &mut doc.cards, card, status, index, &now)? {
            return self.card_view(&doc.board, &doc.cards[card_index]).await;
        }
        // A run owed to the column the card has left is not owed any more, and it is cleared
        // silently: the `Moved` entry already says what the user did.
        doc.cards[card_index].pending_run = None;
        doc.board.updated_at.clone_from(&now);
        self.commit(guard, &mut doc, std::slice::from_ref(card), &now)
            .await?;
        self.card_view(&doc.board, &doc.cards[card_index]).await
    }

    /// Deletes a card and clears references to it from its children.
    pub async fn delete_card(&self, card: &CardId) -> DaemonResult<()> {
        let (guard, mut doc, index) = self.locked_card_document(card).await?;
        // The same refusal archiving makes, for the same reason: a run reporting into a card
        // this document no longer holds has nowhere to land its outcome.
        if is_working(&doc.cards[index]) {
            return Err(DaemonError::Conflict(format!(
                "{} is working; cancel the run first",
                doc.cards[index].display_key(&doc.board)
            )));
        }
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
        let key = doc.cards[index].display_key(&doc.board);
        doc.cards.remove(index);
        let now = self.now();
        // A card that no longer exists blocks nobody. The link is dropped in the same write as
        // the deletion — a dangling one would fail `validate_links` on every later edit — and
        // every card it freed is seeded, so a column that advances the unblocked can act on it.
        let mut seeds = Vec::new();
        for dependant in &mut doc.cards {
            if !dependant.blocked_by.iter().any(|blocker| blocker == card) {
                continue;
            }
            dependant.blocked_by.retain(|blocker| blocker != card);
            dependant.updated_at.clone_from(&now);
            push_activity(
                dependant,
                ActivityKind::Updated,
                None,
                format!("Unblocked: {key} was deleted"),
                &now,
            );
            seeds.push(dependant.id.clone());
        }
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
        doc.board.updated_at.clone_from(&now);
        self.commit(guard, &mut doc, &seeds, &now).await
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

    /// The tail every card-writing trigger site shares.
    ///
    /// Evaluates, saves **once**, records whether the board still parks a card, drops the gate,
    /// and only then starts what the evaluation decided — `start_for_card` re-acquires the same
    /// gate to record its run, and the gates are not reentrant
    /// (`docs/BOARD.md` §4, [`super::automation`]).
    ///
    /// On a board that runs each card in its own worktree the starts are handed off rather than
    /// awaited ([`Boards::apply_starts_after_answer`]): creating the card's worktree can take
    /// minutes, and the request must answer once the save has landed.
    ///
    /// # Errors
    ///
    /// The failure of the evaluation, of the save, or of the first run the plan asked for.
    async fn commit(
        &self,
        guard: tokio::sync::OwnedMutexGuard<()>,
        doc: &mut BoardDocument,
        seeds: &[CardId],
        now: &str,
    ) -> DaemonResult<()> {
        let plan = self.evaluate_with_reservation(doc, seeds, now).await?;
        self.save(doc, BoardChangeReason::CardChanged).await?;
        self.note_pending(
            &doc.board.id,
            doc.cards.iter().any(|card| card.pending_run.is_some()),
        )
        .await;
        drop(guard);
        self.apply_starts_after_answer(&doc.board, plan).await
    }
}

/// Whether the card's newest run is still going.
///
/// The card's own rows are the answer: a run is written when the delegation exists and closed
/// when its delivery lands, so a live row is a run this daemon believes is still going. One left
/// live by a crash is what [`Boards::resume_automation`] adopts or closes at boot, and until it
/// does, the refusals here are right to treat it as working.
pub(super) fn is_working(card: &Card) -> bool {
    latest_run(card).is_some_and(CardRun::is_live)
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
