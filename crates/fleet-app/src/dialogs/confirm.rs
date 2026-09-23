//! §3.8.3 Confirm — *show me exactly what I will lose, in facts, with their age*.

use std::{collections::HashMap, time::Instant};

use fleet_core::{
    agents::{AgentKind, DelegationId, ThreadId},
    github::InspectionPrState,
    ids::{CardId, ContextId, RepoId, SessionId, StatusId, TerminalId, WorktreeId},
    inspection::WorktreeInspection,
    sessions::SessionState,
};
use fleet_proto::{request::RequestBody, response::PruneResult, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, EntityId, FocusHandle, Global, Window, div};

use crate::{
    actions::confirm as confirm_actions,
    bridge::Bridge,
    dialogs::{DialogHost, SessionTransport, notify, root, with_host},
    presentation::{age_secs, now_unix},
    state::AppState,
};

mod facts;
mod lifecycle;
mod policy;
#[cfg(test)]
mod tests;
mod view;

use facts::*;
use lifecycle::{commit, recheck};
// The policy helpers are pinned by `mod tests`, which reaches them through `use super::*`.
#[cfg(test)]
use lifecycle::{
    delete_outcome, prune_requires_review, reviewed_prune_ids, reviewed_prune_request,
};
pub use policy::ConfirmRequest;
use policy::*;

/// The child a staged `x` is asking to cancel, carried from the transcript to the dialog.
#[derive(Debug, Clone)]
pub(crate) struct DelegationCancelDraft {
    id: DelegationId,
    child: ThreadId,
    provider: AgentKind,
    title: String,
}

#[derive(Default)]
struct PendingDelegationCancels(HashMap<EntityId, DelegationCancelDraft>);

impl Global for PendingDelegationCancels {}

/// The run the board's `X` is asking to cancel, carried from the key to the dialog.
///
/// The sentence is built where the card is — the board knows whether a child is out there or a
/// slot is merely owed — so the dialog states a fact it was handed rather than deriving one
/// from a card it would have to look up again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardRunCancelDraft {
    card: CardId,
    key: String,
    fact: String,
}

/// What a board key staged for the confirm it is about to open, per window.
///
/// Neither of these can travel inside [`ConfirmRequest`]: the request is the sentence the user
/// reads back, and a status id is not something a sentence can hold (contracts §5.5 fixes the
/// `MoveCancelsRun` fields). Staging keeps the enum exactly as specified and still leaves the
/// accepted dialog with the one fact it needs to act.
#[derive(Default)]
struct PendingBoardConfirms {
    /// The run `X` staged, keyed by the window whose dialog will adopt it.
    cancels: HashMap<EntityId, CardRunCancelDraft>,
    /// Where `[` / `]` or a dropped card would move the card to.
    moves: HashMap<EntityId, MoveTarget>,
}

/// Where a confirmed [`ConfirmRequest::MoveCancelsRun`] puts the card: the column, and for a
/// dropped card the place in it (`None` appends, which is what `[` / `]` ask for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MoveTarget {
    pub(crate) status: StatusId,
    pub(crate) index: Option<usize>,
}

impl Global for PendingBoardConfirms {}

/// The Confirm dialog's draft.
#[derive(Debug, Default)]
pub struct ConfirmState {
    /// What is being confirmed.
    pub(crate) request: Option<ConfirmRequest>,
    /// The inspection behind a delete confirm's facts.
    pub(crate) inspection: Option<WorktreeInspection>,
    /// The dry run behind a prune confirm's body.
    pub(crate) prune: Option<PruneResult>,
    /// Whether facts are still being gathered.
    pub(crate) loading: bool,
    /// The exact warning from a failed inspection.
    pub(crate) error: Option<String>,
    /// Whether the prune body shows its KEEP list (`s`).
    pub(crate) show_keep: bool,
    /// When the facts last arrived, for the mandatory freshness stamp.
    pub(crate) checked_at: Option<Instant>,
    /// Bumps on every seed and re-check.
    pub(crate) seq: u64,
    pub(crate) list: Option<gpui::ListState>,
    /// The subtitle under the title, when the request's own target is not enough: a worktree
    /// delete names its repository and where the copy is on disk. Read from the snapshot when
    /// the dialog opens, so render only reads it.
    pub(crate) subtitle: Option<String>,
    /// The child this window is being asked to cancel, staged by the transcript's `x`.
    pub(crate) delegation_cancel: Option<DelegationCancelDraft>,
    /// The card run this window is being asked to cancel, staged by the board's `X`.
    pub(crate) card_run_cancel: Option<CardRunCancelDraft>,
    /// Where a confirmed `MoveCancelsRun` moves the card, staged by `[` / `]` or a drop.
    pub(crate) move_target: Option<MoveTarget>,
}

impl ConfirmRequest {
    /// Stages the delegation-specific confirm for the window that opened it.
    pub(crate) fn stage_delegation_cancel(
        state: &Entity<AppState>,
        id: DelegationId,
        child: ThreadId,
        provider: AgentKind,
        title: String,
        cx: &mut App,
    ) {
        cx.default_global::<PendingDelegationCancels>().0.insert(
            state.entity_id(),
            DelegationCancelDraft {
                id,
                child,
                provider,
                title,
            },
        );
    }

    /// Stages the run the board's `X` is asking to cancel, for the window that pressed it.
    ///
    /// `fact` is the one risk line the dialog shows: the board is where a live child and an
    /// owed slot are told apart, so the sentence is written there and only read here.
    pub(crate) fn stage_card_run_cancel(
        state: &Entity<AppState>,
        card: CardId,
        key: String,
        fact: String,
        cx: &mut App,
    ) {
        cx.default_global::<PendingBoardConfirms>()
            .cancels
            .insert(state.entity_id(), CardRunCancelDraft { card, key, fact });
    }

    /// Stages where a confirmed [`ConfirmRequest::MoveCancelsRun`] moves the card.
    pub(crate) fn stage_move_target(state: &Entity<AppState>, target: MoveTarget, cx: &mut App) {
        cx.default_global::<PendingBoardConfirms>()
            .moves
            .insert(state.entity_id(), target);
    }
}

pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    lifecycle::seed(state, bridge, cx);
    adopt_staged_delegation_cancel(state, cx);
    adopt_staged_board_confirm(state, cx);
}

/// Moves the draft `x` staged for this window onto the host the dialog renders from.
///
/// Separate from [`seed`] because it is the half that needs no daemon: the transcript stages,
/// the dialog adopts, and only [`commit_delegation_cancel`] ever reaches the wire.
fn adopt_staged_delegation_cancel(state: &Entity<AppState>, cx: &mut App) {
    let pending = cx
        .default_global::<PendingDelegationCancels>()
        .0
        .remove(&state.entity_id());
    if pending.is_some() {
        with_host(state, cx, |host| host.confirm.delegation_cancel = pending);
    }
}

/// Moves what a board key staged for this window onto the host the dialog renders from.
///
/// Both halves are taken in one pass so a `[` that was answered with `Esc` leaves nothing
/// behind for the next confirm to adopt.
fn adopt_staged_board_confirm(state: &Entity<AppState>, cx: &mut App) {
    let id = state.entity_id();
    let staged = {
        let pending = cx.default_global::<PendingBoardConfirms>();
        (pending.cancels.remove(&id), pending.moves.remove(&id))
    };
    if staged.0.is_none() && staged.1.is_none() {
        return;
    }
    with_host(state, cx, |host| {
        host.confirm.card_run_cancel = staged.0;
        host.confirm.move_target = staged.1;
    });
}

/// The consequence the board's `X` states, in one place for the card and the harness.
const CARD_RUN_CANCEL_CONSEQUENCE: &str =
    "Stops the run. What it already changed in the worktree is kept.";
/// The consequence a transcript's `x` states.
const DELEGATION_CANCEL_CONSEQUENCE: &str =
    "Stops the delegated child. Work it already changed is kept.";

/// The one sentence the open confirm asks the user to accept (§3.8.3).
///
/// Ordered exactly as [`render`] chooses its card, so the harness reports the sentence that is
/// on screen rather than the one a second reading of the draft would produce
/// (`docs/TESTING-HARNESS.md` §3, `dialog.message`). `None` for a confirm opened with no
/// target, which is the case the "Nothing to confirm" card draws.
pub(crate) fn consequence(draft: &ConfirmState) -> Option<String> {
    if draft.delegation_cancel.is_some() {
        return Some(DELEGATION_CANCEL_CONSEQUENCE.to_owned());
    }
    if draft.card_run_cancel.is_some() {
        return Some(CARD_RUN_CANCEL_CONSEQUENCE.to_owned());
    }
    let request = draft.request.as_ref()?;
    Some(request.consequence(&view::facts_for(request, draft, now_unix())))
}

pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if let Some(pending) = host.read(cx).confirm.delegation_cancel.clone() {
        return delegation_cancel_card(state, bridge, focus, pending);
    }
    if let Some(pending) = host.read(cx).confirm.card_run_cancel.clone() {
        return card_run_cancel_card(state, bridge, focus, pending);
    }
    view::render(state, bridge, focus, host, window, cx)
}

/// The confirm the board's `X` raises: one card, one run, one sentence about what stops.
fn card_run_cancel_card(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    pending: CardRunCancelDraft,
) -> AnyElement {
    let accept_state = state.clone();
    let accept_bridge = bridge.clone();
    let strong_state = state.clone();
    let strong_bridge = bridge.clone();
    let lower = pending.card.clone();
    let upper = pending.card.clone();
    root(focus)
        .on_action(move |_: &confirm_actions::Accept, _window, cx| {
            commit_card_run_cancel(&accept_state, &accept_bridge, lower.clone(), cx);
        })
        .on_action(move |_: &confirm_actions::AcceptStrong, _window, cx| {
            commit_card_run_cancel(&strong_state, &strong_bridge, upper.clone(), cx);
        })
        .child(
            ConfirmDialog::new(
                format!("Cancel {}'s run?", pending.key),
                FactList::new().fact(Fact::risk(pending.fact)),
            )
            .dismiss_action(crate::dialogs::Dialogs::Confirm.dismiss_action())
            .accept_actions(
                Box::new(confirm_actions::Accept),
                Box::new(confirm_actions::AcceptStrong),
            )
            .consequence(CARD_RUN_CANCEL_CONSEQUENCE)
            .icon(Icon::CircleX)
            .action_label("Cancel run")
            .force_confirm_key(ConfirmKey::Lower),
        )
        .into_any_element()
}

/// The confirm the agent transcript's `x` raises over one delegated child.
fn delegation_cancel_card(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    pending: DelegationCancelDraft,
) -> AnyElement {
    let accept_state = state.clone();
    let accept_bridge = bridge.clone();
    let strong_state = state.clone();
    let strong_bridge = bridge.clone();
    let lower = pending.clone();
    let upper = pending.clone();
    let provider = pending.provider.executable();
    let title = if pending.title.trim().is_empty() {
        format!("Cancel {provider} subagent?")
    } else {
        format!("Cancel {provider} subagent “{}”?", pending.title)
    };
    root(focus)
        .on_action(move |_: &confirm_actions::Accept, _window, cx| {
            commit_delegation_cancel(&accept_state, &accept_bridge, lower.id, cx);
        })
        .on_action(move |_: &confirm_actions::AcceptStrong, _window, cx| {
            commit_delegation_cancel(&strong_state, &strong_bridge, upper.id, cx);
        })
        .child(
            ConfirmDialog::new(
                title,
                FactList::new().fact(Fact::risk(format!(
                    "child {} stops and reports no further work",
                    pending.child
                ))),
            )
            .dismiss_action(crate::dialogs::Dialogs::Confirm.dismiss_action())
            .accept_actions(
                Box::new(confirm_actions::Accept),
                Box::new(confirm_actions::AcceptStrong),
            )
            .target(pending.id.to_string())
            .consequence(DELEGATION_CANCEL_CONSEQUENCE)
            .icon(Icon::CircleX)
            .action_label("Cancel delegation")
            .force_confirm_key(ConfirmKey::Lower),
        )
        .into_any_element()
}

/// Confirms the board's `X`: cancel the card's run, then close.
///
/// Through `send_card_reporting`, not a bare `send`: the daemon answers with the card, so the
/// tile's mark drops as soon as the run is gone, and a refusal — a run that ended between the
/// key and the `y` — reaches the sticky slot instead of vanishing (contracts §5.5).
fn commit_card_run_cancel<T: SessionTransport>(
    state: &Entity<AppState>,
    bridge: &T,
    card: CardId,
    cx: &mut App,
) {
    crate::screens::board::send_card_reporting(
        state,
        bridge,
        RequestBody::CardRunCancel { card_id: card },
        crate::screens::board::Refusal::Sticky,
        cx,
    );
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}

fn commit_delegation_cancel<T: SessionTransport>(
    state: &Entity<AppState>,
    bridge: &T,
    delegation: DelegationId,
    cx: &mut App,
) {
    bridge.send(RequestBody::DelegationCancel { delegation });
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}

impl ConfirmState {
    fn list_len(&self) -> usize {
        self.prune.as_ref().map_or(0, |result| {
            1 + result.deleted.len()
                + if self.show_keep {
                    1 + result.skipped.len()
                } else {
                    0
                }
        })
    }

    fn update_list(&mut self) {
        let len = self.list_len();
        match &self.list {
            Some(list) => list.reset(len),
            None => {
                self.list = Some(gpui::ListState::new(
                    len,
                    gpui::ListAlignment::Top,
                    gpui::px(0.0),
                ))
            }
        }
    }

    fn begin_recheck(&mut self) {
        self.seq = self.seq.wrapping_add(1);
        self.loading = true;
        self.error = None;
        self.checked_at = None;
        if matches!(self.request, Some(ConfirmRequest::Prune { .. })) {
            self.prune = None;
            self.update_list();
        }
    }
}
