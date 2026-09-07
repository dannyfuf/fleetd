use super::*;

/// Claims the active context's board whenever the tab is showing and its slot went stale.
///
/// The screen observes [`AppState`] rather than asking from `render`: every transition that
/// makes the board stale — `g b`, `r`, a reconnect, an `Ack` from a card mutation — ends in a
/// notify, so the observation sees exactly what the repaint used to and the paint itself stays
/// free of daemon round trips.
pub(super) fn synchronize(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if !matches!(state.read(cx).screen, Screen::Hub { tab: HubTab::Board }) {
        return;
    }
    ensure_current(state, bridge, cx);
}

/// Ensures the active context's board through the ordinary asynchronous reply channel.
pub(super) fn ensure_current(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    ensure_backends(state, bridge, cx);
    let Some((context_id, generation)) = state.update(cx, |state, _| state.begin_board_load())
    else {
        return;
    };
    let reply = bridge.request(RequestBody::EnsureBoard {
        context_id: context_id.clone(),
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let result = match reply.recv().await {
            Ok(Ok(ResponseBody::Board(view))) => Ok(view),
            Ok(Ok(_)) => Err("EnsureBoard returned an unexpected response".to_owned()),
            Ok(Err(error)) => Err(error.message),
            Err(error) => Err(format!("Board request channel closed: {error}")),
        };
        state.update(cx, |state, cx| {
            state.finish_board_load(&context_id, generation, result);
            cx.notify();
        });
    })
    .detach();
}

/// Fetches the daemon's backend registry once per connection.
///
/// Nothing in `fleet-app` knows a backend by name: the header's chip, the settings dialog's
/// kind cycler and every settings row it draws are built from these descriptors. A failure is
/// silent on purpose — the header falls back to the raw kind and the settings dialog to the
/// board's own kind, which is exactly what an older daemon would leave it with.
fn ensure_backends(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if !state.update(cx, |app, _| app.begin_backends_load()) {
        return;
    }
    let reply = bridge.request(RequestBody::ListBoardBackends {});
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::BoardBackends(backends))) = reply.recv().await else {
            // The ask is made once per connection and the flag was set before it went out, so
            // a failure has to release it or this connection never asks again.
            state.update(cx, |app, _| app.backends_load_failed());
            return;
        };
        state.update(cx, |app, cx| {
            app.apply_backends(backends);
            cx.notify();
        });
    })
    .detach();
}

/// Whether a `board.sync` job is queued or running for the shown board.
#[must_use]
pub(super) fn syncing(state: &AppState) -> bool {
    let Some(board) = state.board().map(|view| view.board.id.clone()) else {
        return false;
    };
    state.snapshot.as_ref().is_some_and(|snapshot| {
        snapshot.jobs.iter().any(|job| {
            matches!(&job.kind, JobKind::Custom(kind) if kind == "board.sync")
                && job.target == board.as_str()
                && matches!(job.status, JobStatus::Queued | JobStatus::Running)
        })
    })
}

/// Where a refused board request writes its message.
///
/// A dialog paints a scrim over the whole window, so a refusal written to the status bar's
/// sticky slot while one is open is a refusal nobody can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The status bar's sticky slot, for keys pressed on the board itself.
    Sticky,
    /// The error line of the card detail, while it is still showing this card.
    CardDetail(CardId),
}

impl Refusal {
    /// The sink a surface returning to the card detail must use.
    #[must_use]
    pub(crate) fn for_detail(detail: bool, card: &CardId) -> Self {
        if detail {
            Self::CardDetail(card.clone())
        } else {
            Self::Sticky
        }
    }

    pub(crate) fn report(self, state: &Entity<AppState>, message: String, cx: &mut App) {
        let showing = |card: &CardId, cx: &mut App| {
            matches!(
                state.read(cx).overlay,
                Some(Overlay::Dialog(Dialogs::CardDetail))
            ) && dialogs::with_host(state, cx, |host| {
                host.card_detail.card_id.as_ref() == Some(card)
            })
        };
        match self {
            Self::Sticky => fail(state, message, cx),
            // The detail can be closed and reopened on another card before a slow refusal
            // lands: card A's message on card B's dialog names a card the user did not touch.
            Self::CardDetail(card) if showing(&card, cx) => {
                dialogs::with_host(state, cx, |host| host.card_detail.error = Some(message));
                state.update(cx, |_, cx| cx.notify());
            }
            // The reply can arrive after the detail closed, and the next open reseeds the
            // slot: written there, the refusal would never be shown anywhere at all.
            Self::CardDetail(_) => fail(state, message, cx),
        }
    }
}

/// Sends a card mutation and applies the daemon's answer; failures take the sticky slot.
pub(super) fn send_card(
    state: &Entity<AppState>,
    bridge: &Bridge,
    body: RequestBody,
    cx: &mut App,
) {
    send_card_reporting(state, bridge, body, Refusal::Sticky, cx);
}

/// Sends a card mutation and writes a refusal to `refusal`.
pub(crate) fn send_card_reporting(
    state: &Entity<AppState>,
    bridge: &Bridge,
    body: RequestBody,
    refusal: Refusal,
    cx: &mut App,
) {
    let reply = bridge.request(body);
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = match reply.recv().await {
            Ok(answer) => answer,
            Err(error) => {
                let message = format!("Board request channel closed: {error}");
                cx.update(|cx| refusal.report(&state, message, cx));
                return;
            }
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::Card(card)) => state.update(cx, |app, cx| {
                let id = card.id.clone();
                app.apply_card(card);
                focus_card(app, &id);
                cx.notify();
            }),
            Ok(ResponseBody::Ack) => state.update(cx, |app, cx| {
                app.board_stale = true;
                cx.notify();
            }),
            Ok(_) => {}
            Err(error) => refusal.report(&state, error.message, cx),
        });
    })
    .detach();
}

/// Records a refusal in the sticky slot (§1.8: an error is never a toast).
pub(super) fn fail(state: &Entity<AppState>, message: String, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.sticky_error = Some(StickyError {
            text: message,
            job: None,
            retryable: false,
        });
        cx.notify();
    });
}

/// Opens a worktree's session and routes the app to it.
///
/// `refusal` is where a failure is written: the card detail dialog paints a scrim over the
/// status bar, so a refusal sent to the sticky slot from there is one nobody can read.
pub(crate) fn open_session(
    worktree: WorktreeId,
    refusal: Refusal,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    let reply = bridge.request(RequestBody::EnsureSession {
        worktree: Some(worktree),
        agent: None,
        sleep_previous: true,
    });
    let state = state.clone();
    // Starting a session takes as long as it takes, and the user keeps typing meanwhile.
    let requested_from = state.read(cx).overlay.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::Session(session)) => state.update(cx, |app, cx| {
                // Anything opened since the request is the user's current work: routing to the
                // session now would close it and throw its draft away.
                if app.overlay != requested_from {
                    return;
                }
                app.touch_session(session.id.clone());
                app.screen = Screen::Workspace {
                    session: session.id.clone(),
                };
                app.close_overlay();
                cx.notify();
            }),
            Ok(_) => {}
            Err(error) => refusal.report(&state, error.message, cx),
        });
    })
    .detach();
}

/// Creates the worktree a card asks for, then links and opens nothing: §8 keeps `w` and `o`
/// separate so a create never steals the screen while its hooks are still running.
pub(super) fn request_worktree(
    card: CardId,
    repo: Option<RepoId>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    request_worktree_reporting(card, repo, state, bridge, Refusal::Sticky, cx);
}

/// Creates a card's worktree and writes a refusal to `refusal`.
pub(crate) fn request_worktree_reporting(
    card: CardId,
    repo: Option<RepoId>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    refusal: Refusal,
    cx: &mut App,
) {
    let reply = bridge.request(RequestBody::CreateWorktreeFromCard {
        card_id: card,
        repo_id: repo,
        base: None,
        host: None,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::CardWorktree { card, worktree, .. }) => state.update(cx, |app, cx| {
                let id = card.id.clone();
                app.apply_card(card);
                focus_card(app, &id);
                app.toast_short(
                    format!("\u{2713} {}", worktree.id.as_str()),
                    Icon::GitBranchPlus,
                    Instant::now(),
                );
                cx.notify();
            }),
            Ok(_) => {}
            Err(error) => refusal.report(&state, error.message, cx),
        });
    })
    .detach();
}
