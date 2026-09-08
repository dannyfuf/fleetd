//! §3.8.3 Confirm — *show me exactly what I will lose, in facts, with their age*.

use std::time::Instant;

use fleet_core::{
    github::InspectionPrState,
    ids::{CardId, ContextId, RepoId, SessionId, TerminalId, WorktreeId},
    inspection::WorktreeInspection,
    sessions::SessionState,
};
use fleet_proto::{request::RequestBody, response::PruneResult, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::confirm as confirm_actions,
    bridge::Bridge,
    dialogs::{DialogHost, notify, root, with_host},
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
pub(crate) use lifecycle::seed;
use lifecycle::*;
pub use policy::ConfirmRequest;
use policy::*;
pub(crate) use view::render;

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
