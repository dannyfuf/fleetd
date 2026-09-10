use super::*;
use fleet_core::model::Repo;
use std::future::Future;

/// One wording for the archived-card refusal, which is raised again after the board guard is
/// dropped for the worktree clone and the card is reloaded.
pub(super) const ARCHIVED_CARD: &str = "cannot create a worktree for an archived card";

fn archived_card() -> DaemonError {
    DaemonError::Conflict(ARCHIVED_CARD.to_owned())
}

/// One wording for the ownership refusal, raised both before and after the clone, because the
/// guard that made the first check authoritative is dropped while the worktree is made.
fn worktree_owned_by_another_card(worktree: impl std::fmt::Display) -> DaemonError {
    DaemonError::Conflict(format!(
        "worktree {worktree} already belongs to another card"
    ))
}

impl Boards {
    /// Creates or reuses the card's repository worktree and optionally starts the card.
    pub async fn create_worktree_from_card(
        &self,
        card: &CardId,
        repo: Option<RepoId>,
        base: Option<String>,
        host: Option<HostId>,
    ) -> DaemonResult<(Card, Worktree, bool)> {
        if host.is_some() {
            return Err(DaemonError::Protocol(
                "hosted card worktree creation must pass through router orchestration".to_owned(),
            ));
        }
        let worktrees = Arc::clone(&self.worktrees);
        self.create_worktree_from_card_routed(
            card,
            repo,
            base,
            host,
            move |repo, slug, branch, base, _host| async move {
                let (created, worktree, _post_create_job) = worktrees
                    .create(repo.id, slug, branch, base, repo.hooks)
                    .await?;
                Ok((created, worktree))
            },
        )
        .await
    }

    /// Creates and links a card worktree through a placement-aware creation callback.
    ///
    /// Local dispatch uses `create_worktree_from_card`; router dispatch supplies a callback that
    /// performs repository ensure and worktree creation on the selected host.
    pub async fn create_worktree_from_card_routed<Create, Created>(
        &self,
        card: &CardId,
        repo: Option<RepoId>,
        base: Option<String>,
        host: Option<HostId>,
        create: Create,
    ) -> DaemonResult<(Card, Worktree, bool)>
    where
        Create: FnOnce(Repo, String, Option<String>, Option<String>, Option<HostId>) -> Created
            + Send
            + 'static,
        Created: Future<Output = DaemonResult<(bool, Worktree)>> + Send + 'static,
    {
        // Own the entire create-and-link transaction independently of the socket request.
        let service = self.clone();
        let card = card.clone();
        tokio::spawn(async move {
            service
                .create_and_link_worktree(&card, repo, base, host, create)
                .await
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    async fn create_and_link_worktree<Create, Created>(
        &self,
        card: &CardId,
        repo: Option<RepoId>,
        base: Option<String>,
        host: Option<HostId>,
        create: Create,
    ) -> DaemonResult<(Card, Worktree, bool)>
    where
        Create: FnOnce(Repo, String, Option<String>, Option<String>, Option<HostId>) -> Created,
        Created: Future<Output = DaemonResult<(bool, Worktree)>>,
    {
        let (mut guard, mut doc, mut index) = self.card_document(card).await?;
        if doc.cards[index].archived {
            return Err(archived_card());
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
            return Err(worktree_owned_by_another_card(&existing.id));
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
                let made = match create(repo, slug.clone(), Some(slug.clone()), base, host.clone())
                    .await
                {
                    Ok((created, worktree)) => (created, worktree),
                    // Two `w` presses on one card both pass the adoption check above and both
                    // reach here, because the guard is dropped for the clone. The loser must
                    // adopt what the winner just made — reporting `already exists` for the
                    // card's own worktree is a toast for work that succeeded.
                    Err(error @ DaemonError::Conflict(_)) if host.is_none() => {
                        let adopted = self
                            .state_store
                            .load()
                            .await?
                            .worktrees
                            .into_iter()
                            .find(|w| w.repo_id == repo_id && w.slug == slug);
                        match adopted {
                            Some(worktree) => (false, worktree),
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
                        // `Worktrees::create` is idempotent, so an adopted worktree must not
                        // be reported as one this call made.
                        return Err(DaemonError::Conflict(if made.0 {
                            format!(
                                "worktree {} was created, but its card is gone: {error}",
                                made.1.id,
                            )
                        } else {
                            format!("worktree {} is linked to no card: {error}", made.1.id)
                        }));
                    }
                };
                guard = reloaded.0;
                doc = reloaded.1;
                index = reloaded.2;
                if doc.cards[index].archived {
                    // Archiving is the other way the card can move while the guard is dropped,
                    // and it leaves the same orphan: a worktree this call made is referenced by
                    // nothing on the board, so the refusal names it rather than reading as the
                    // pre-clone refusal, which promises nothing was created.
                    return Err(if made.0 {
                        DaemonError::Conflict(format!(
                            "worktree {} was created, but its card was archived",
                            made.1.id,
                        ))
                    } else {
                        archived_card()
                    });
                }
                // The ownership check above ran under the guard this clone dropped. Two cards
                // that render the same slug would otherwise both adopt one worktree here.
                if doc.cards.iter().any(|other| {
                    other.id != *card && other.worktree_id.as_ref() == Some(&made.1.id)
                }) {
                    return Err(worktree_owned_by_another_card(&made.1.id));
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

    /// A repository a board may point at: it must exist and share the board's context, or the
    /// app — which only ever offers this context's repos — could never show what was stored.
    pub(super) async fn validate_repo(
        &self,
        board: &Board,
        id: Option<&RepoId>,
    ) -> DaemonResult<()> {
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
}

/// Drops a repository link the state no longer backs.
///
/// A deleted repository — or one moved to another context — stays written on the card, so
/// every view would keep offering it as the card's repository while `create_worktree_from_card`
/// refused it with an error that never says the repository is gone. Only the view is scrubbed:
/// the stored id survives a repository that comes back under the same name.
pub(super) fn scrub_repo(
    state: &fleet_core::state::State,
    context: &ContextId,
    id: &mut Option<RepoId>,
) {
    if id.as_ref().is_some_and(|id| {
        !state
            .repos
            .iter()
            .any(|repo| repo.id == *id && repo.context_id == *context)
    }) {
        *id = None;
    }
}
