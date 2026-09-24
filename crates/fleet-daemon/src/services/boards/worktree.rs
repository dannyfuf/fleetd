use super::*;
use fleet_core::model::Repo;
use std::future::Future;

use crate::services::worktrees::PullRequestHead;

/// One wording for the archived-card refusal, which is raised again after the board guard is
/// dropped for the worktree clone and the card is reloaded.
pub(super) const ARCHIVED_CARD: &str = "cannot create a worktree for an archived card";

/// A run start the card's own facts refuse, in the shape `require_automatable` raises.
fn refused_run(reason: &str) -> DaemonError {
    BoardError::Invalid {
        field: "automation".into(),
        reason: reason.into(),
    }
    .into()
}

fn archived_card() -> DaemonError {
    DaemonError::Conflict(ARCHIVED_CARD.to_owned())
}

/// What a card-worktree start has to do about the card's pull request before it runs.
enum PullRequestStart {
    /// Nothing: a board-worktree board, a card that will not run, or a linked card with no
    /// pull request.
    Nothing,
    /// The card's worktree exists; bring it to the pull request's current head.
    Follow {
        worktree: Box<Worktree>,
        number: u64,
        /// `owner/name#number`, as the refusal names it.
        pr_key: String,
    },
    /// The card has no worktree yet; create one from the pull request, or adopt the one that
    /// already exists for it and, when `follow` says so, bring that to the head.
    Create {
        repo: RepoId,
        number: u64,
        /// `owner/name#number`, as the refusal names it.
        pr_key: String,
        /// Whether this start is one that follows the head (see [`follows_head`]).
        follow: bool,
    },
}

/// Whether a run in `status` brings the card's worktree to its pull request's current head: only
/// a run in the board's first running column — the first, in board order, whose automation runs
/// an action — does. That run is the review; a later column (Review published) works from what
/// the review saw, so moving its checkout would put the report's file:line findings on commits
/// the review never read.
fn follows_head(board: &Board, status: &StatusId) -> bool {
    board
        .statuses
        .iter()
        .find(|column| {
            column
                .automation
                .as_ref()
                .is_some_and(|automation| automation.on_enter.is_some())
        })
        .is_some_and(|column| column.id == *status)
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

    /// Creates or adopts the worktree of a card's pull request and links it to the card, on a
    /// card-worktree board, or brings an already linked one to the pull request's current head;
    /// a no-op on a board-worktree board and for a linked card with no pull request.
    ///
    /// Called by a run's start, outside every gate, before the run is prepared. Creating a
    /// worktree fetches the pull request and runs the repository's hooks — minutes of work — so
    /// the gate is held only to read the card and, afterwards, to write the link. Following the
    /// head fetches too, and holds no gate at all.
    ///
    /// # Errors
    ///
    /// The C5 refusals — a card with neither a worktree nor a pull request, a pull request in a
    /// repository Fleet does not hold, a linked worktree another host owns, a linked worktree
    /// with local changes the head cannot fast-forward over — as `BoardError::Invalid` on the
    /// `automation` field; the fetch's or the creation's own failure; the ownership `Conflict`
    /// when a card on another board already links the worktree; and the `Conflict`s
    /// `create_and_link_worktree` raises when the card moved while the gate was dropped.
    pub(crate) async fn ensure_pull_request_worktree(
        &self,
        board: &BoardId,
        card: &CardId,
    ) -> DaemonResult<bool> {
        let worktrees = Arc::clone(&self.worktrees);
        self.ensure_pull_request_worktree_with(board, card, move |repo, number| async move {
            // The post-create job is the hooks, which run on their own: the run only needs the
            // checkout, and a hook failure marks the worktree degraded rather than refusing it.
            let (created, worktree, _post_create_job) =
                worktrees.create_from_pr(repo, number).await?;
            Ok((created, worktree))
        })
        .await
    }

    /// [`Self::ensure_pull_request_worktree`] around an injected creation, so a test can move the
    /// card while the gate is dropped. Answers whether it created a worktree.
    pub(super) async fn ensure_pull_request_worktree_with<Create, Created>(
        &self,
        board: &BoardId,
        card: &CardId,
        create: Create,
    ) -> DaemonResult<bool>
    where
        Create: FnOnce(RepoId, u64) -> Created,
        Created: Future<Output = DaemonResult<(bool, Worktree)>>,
    {
        let (repo_id, number, pr_key, follow) = match self.pull_request_start(board, card).await? {
            PullRequestStart::Nothing => return Ok(false),
            PullRequestStart::Follow {
                worktree,
                number,
                pr_key,
            } => {
                self.follow_pull_request(&worktree, number, &pr_key).await?;
                return Ok(false);
            }
            PullRequestStart::Create {
                repo,
                number,
                pr_key,
                follow,
            } => (repo, number, pr_key, follow),
        };
        // Fetching the pull request and running the hooks must not hold the board, or every
        // other request for it times out behind a clone.
        let (created, worktree) = create(repo_id.clone(), number).await?;
        // An adopted worktree — one the old Review tab or `fleet create` already made for this
        // branch — holds whatever it was left at: the same head rule as a linked one applies.
        // Another board's card is refused first, so its worktree is never moved on this card's
        // behalf; the check is repeated under `pull_request_links` below.
        if !created && follow {
            if self.linked_on_another_board(board, &worktree.id) {
                return Err(worktree_owned_by_another_card(&worktree.id));
            }
            self.follow_pull_request(&worktree, number, &pr_key).await?;
        }
        // Across boards: the same pull request can sit on two contexts' Reviews boards, and
        // `create_from_pr` answers both with one worktree.
        let _links = self.pull_request_links.lock().await;
        if self.linked_on_another_board(board, &worktree.id) {
            return Err(worktree_owned_by_another_card(&worktree.id));
        }
        let _guard = self.gate(board).await;
        let mut doc = self.load(board)?;
        // The same two ways the card can move while the gate was dropped, refused in the words
        // `create_and_link_worktree` uses, so the worktree this made is named rather than lost.
        let Some(index) = doc.cards.iter().position(|other| other.id == *card) else {
            let error = DaemonError::from(BoardError::CardNotFound(card.to_string()));
            return Err(DaemonError::Conflict(if created {
                format!(
                    "worktree {} was created, but its card is gone: {error}",
                    worktree.id
                )
            } else {
                format!("worktree {} is linked to no card: {error}", worktree.id)
            }));
        };
        if doc.cards[index].archived {
            return Err(if created {
                DaemonError::Conflict(format!(
                    "worktree {} was created, but its card was archived",
                    worktree.id
                ))
            } else {
                archived_card()
            });
        }
        if doc
            .cards
            .iter()
            .any(|other| other.id != *card && other.worktree_id.as_ref() == Some(&worktree.id))
        {
            return Err(worktree_owned_by_another_card(&worktree.id));
        }
        let now = self.now();
        let target = &mut doc.cards[index];
        target.worktree_id = Some(worktree.id.clone());
        target.repo_id = Some(repo_id);
        target.updated_at.clone_from(&now);
        push_activity(
            target,
            ActivityKind::WorktreeCreated,
            None,
            format!("Linked worktree {}", worktree.id),
            &now,
        );
        doc.board.updated_at = now;
        self.save(&doc, BoardChangeReason::CardChanged).await?;
        Ok(created)
    }

    /// What a start owes the card's pull request, read under the board gate.
    async fn pull_request_start(
        &self,
        board: &BoardId,
        card: &CardId,
    ) -> DaemonResult<PullRequestStart> {
        let _guard = self.gate(board).await;
        let doc = self.load(board)?;
        if doc.board.settings.run_location.is_board_worktree() {
            return Ok(PullRequestStart::Nothing);
        }
        // A card that is gone or archived is not going to run: the start's own preparation
        // answers that, and creating a worktree for it would only leave an orphan.
        let Some(target) = doc
            .cards
            .iter()
            .find(|other| other.id == *card && !other.archived)
        else {
            return Ok(PullRequestStart::Nothing);
        };
        let state = self.state_store.load().await?;
        if let Some(linked) = target.worktree_id.as_ref()
            && let Some(worktree) = self.known_worktree(&state, linked)
        {
            // Each card's worktree can live somewhere else, so the host rule the board
            // worktree gets from `require_automatable` is applied here, per card.
            if let Some(host) = worktree.host {
                return Err(refused_run(&format!(
                    "automation is unavailable on a worktree owned by host {host}"
                )));
            }
            // A review runs at the pull request's current head, not the one it was cloned at;
            // a later column's run stays on the head the review saw.
            return Ok(match target.pull_request.as_ref() {
                Some(pull_request) if follows_head(&doc.board, &target.status_id) => {
                    PullRequestStart::Follow {
                        number: pull_request.number,
                        pr_key: format!("{}#{}", pull_request.repo, pull_request.number),
                        worktree: Box::new(worktree),
                    }
                }
                _ => PullRequestStart::Nothing,
            });
        }
        let Some(pull_request) = target.pull_request.as_ref() else {
            return Err(refused_run(&format!(
                "{} has no worktree to run in; link a pull request or create its worktree first",
                target.display_key(&doc.board)
            )));
        };
        // GitHub names are case-insensitive, so a reference spelt `Acme/API` runs in the
        // repository Fleet registered as `acme/api`, under that id.
        let Some(repo) = state.repos.iter().find(|repo| {
            repo.id
                .as_str()
                .eq_ignore_ascii_case(pull_request.repo.as_str())
        }) else {
            return Err(refused_run(&format!(
                "{} is not a Fleet repository; clone it into this context first",
                pull_request.repo
            )));
        };
        Ok(PullRequestStart::Create {
            repo: repo.id.clone(),
            number: pull_request.number,
            pr_key: format!("{}#{}", pull_request.repo, pull_request.number),
            follow: follows_head(&doc.board, &target.status_id),
        })
    }

    /// Brings the card's existing worktree to its pull request's head, outside every gate, or
    /// refuses the run when the worktree holds work of the user's that moving it would touch.
    async fn follow_pull_request(
        &self,
        worktree: &Worktree,
        number: u64,
        pr_key: &str,
    ) -> DaemonResult<()> {
        match self
            .worktrees
            .follow_pull_request_head(worktree, number)
            .await?
        {
            PullRequestHead::Current | PullRequestHead::Moved => Ok(()),
            PullRequestHead::LocalChanges => Err(refused_run(&format!(
                "The worktree for {pr_key} has local changes; commit or discard them before the review runs."
            ))),
        }
    }

    /// Whether a card on any other board already links this worktree.
    ///
    /// Read without those boards' gates: the link this guards is taken under
    /// `pull_request_links`, the only writer of a pull-request link, so a racing claim cannot
    /// land between this read and the caller's save.
    fn linked_on_another_board(&self, board: &BoardId, worktree: &WorktreeId) -> bool {
        let ids = match self.store.list() {
            Ok(ids) => ids,
            Err(error) => {
                tracing::warn!(%error, "the board store could not be listed for a worktree's owner");
                return false;
            }
        };
        ids.iter().filter(|id| *id != board).any(|id| {
            self.scan_load(id).is_some_and(|doc| {
                doc.cards
                    .iter()
                    .any(|card| card.worktree_id.as_ref() == Some(worktree))
            })
        })
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
        // A pull request's card on a card-worktree board runs in that pull request's checkout,
        // which a slug branch off the base is not: the verb makes the same worktree a run would,
        // under the same single-owner rule. A link to a worktree that no longer exists is no
        // link, exactly as a run's start reads it.
        if doc.cards[index].pull_request.is_some()
            && doc.cards[index]
                .worktree_id
                .as_ref()
                .is_none_or(|linked| self.known_worktree(&state, linked).is_none())
            && !doc.board.settings.run_location.is_board_worktree()
        {
            if let Some(host) = host {
                return Err(DaemonError::Conflict(format!(
                    "a pull request's worktree is made on this daemon, not on host {host}"
                )));
            }
            let board = doc.board.id.clone();
            drop(guard);
            let created = self.ensure_pull_request_worktree(&board, card).await?;
            let (guard, doc, index) = self.card_document(card).await?;
            let state = self.state_store.load().await?;
            let worktree = doc.cards[index]
                .worktree_id
                .as_ref()
                .and_then(|linked| state.worktrees.iter().find(|w| w.id == *linked))
                .cloned()
                .ok_or_else(|| {
                    DaemonError::Conflict(format!(
                        "{} lost its pull request's worktree while it was made",
                        doc.cards[index].display_key(&doc.board)
                    ))
                })?;
            let _guard = guard;
            let now = self.now();
            let mut doc = doc;
            start_on_worktree(&mut doc, index, card, &now)?;
            return Ok((self.save_card(doc, index, &now).await?, worktree, created));
        }
        // The card and the board are only offered the repositories their views show, and a
        // view drops one the state no longer backs: falling back to a deleted repository here
        // would refuse the card by a name nothing on the board still displays.
        let mut card_repo = doc.cards[index].repo_id.clone();
        let mut board_repo = doc.board.default_repo_id.clone();
        scrub_repo(&state, Some(&doc.board.context_id), &mut card_repo);
        scrub_repo(&state, Some(&doc.board.context_id), &mut board_repo);
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
        start_on_worktree(&mut doc, index, card, &now)?;
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

/// Moves a card that just got its worktree into the board's first started column, when the board
/// asks for that and the card has not started yet.
fn start_on_worktree(
    doc: &mut BoardDocument,
    index: usize,
    card: &CardId,
    now: &str,
) -> DaemonResult<()> {
    let target = &doc.cards[index];
    let should_start = doc.board.settings.start_on_worktree
        && doc.board.statuses.iter().any(|status| {
            status.id == target.status_id
                && matches!(
                    status.category,
                    StatusCategory::Backlog | StatusCategory::Unstarted
                )
        });
    if should_start && let Some(status) = first_status_in(&doc.board, StatusCategory::Started) {
        ops::move_card(&doc.board, &mut doc.cards, card, &status.id, None, now)?;
    }
    Ok(())
}

/// Drops a repository link the state no longer backs.
///
/// A deleted repository — or one moved to another context — stays written on the card, so
/// every view would keep offering it as the card's repository while `create_worktree_from_card`
/// refused it with an error that never says the repository is gone. Only the view is scrubbed:
/// the stored id survives a repository that comes back under the same name. `context` is the
/// context the repository must belong to, or `None` when any context will do.
pub(super) fn scrub_repo(
    state: &fleet_core::state::State,
    context: Option<&ContextId>,
    id: &mut Option<RepoId>,
) {
    if id.as_ref().is_some_and(|id| {
        !state
            .repos
            .iter()
            .any(|repo| repo.id == *id && context.is_none_or(|context| repo.context_id == *context))
    }) {
        *id = None;
    }
}

/// The context a card's repository must belong to on `board`: its own, except on a Reviews
/// board, where a pull request's repository may belong to any context and the card names it.
pub(super) fn card_repo_context(board: &Board) -> Option<&ContextId> {
    (board.kind != BoardKind::Reviews).then_some(&board.context_id)
}
