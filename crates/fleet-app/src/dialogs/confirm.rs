//! §3.8.3 Confirm — *show me exactly what I will lose, in facts, with their age*.

use std::{collections::HashMap, time::Instant};

use fleet_core::{
    agents::{AgentKind, DelegationId, ThreadId},
    github::InspectionPrState,
    ids::{CardId, ContextId, RepoId, SessionId, TerminalId, WorktreeId},
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
    /// The child this window is being asked to cancel, staged by the transcript's `x`.
    pub(crate) delegation_cancel: Option<DelegationCancelDraft>,
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
}

pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    lifecycle::seed(state, bridge, cx);
    adopt_staged_delegation_cancel(state, cx);
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

pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(pending) = host.read(cx).confirm.delegation_cancel.clone() else {
        return view::render(state, bridge, focus, host, window, cx);
    };
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
            .target(pending.id.to_string())
            .consequence("Stops the delegated child. Work it already changed is kept.")
            .icon(Icon::CircleX)
            .action_label("Cancel delegation")
            .force_confirm_key(ConfirmKey::Lower),
        )
        .into_any_element()
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
