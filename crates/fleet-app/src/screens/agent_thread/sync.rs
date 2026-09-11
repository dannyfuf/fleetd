//! Adopting the daemon's projection, and preparing everything `render` will compose.
//!
//! The split this module exists for: a **structural** change — a new item, a settled turn, an
//! opened gate — rebuilds the grouping, the folds and the footers, while pure text growth on an
//! item that already has a row rewrites that row and nothing else. That is what keeps a fast
//! model from re-running grouping, folding and summarization per token, and it is why
//! [`streaming_only_change`] compares every row-bearing part of the projection rather than the
//! three that happen to be lists of items: a part missing from the comparison is a row that
//! silently never appears.

use std::{collections::HashMap, rc::Rc, time::Instant};

use fleet_core::agents::{ItemId, ItemKind, ThreadProjection, TurnState};
use fleet_lazygit::diff_view::DiffView;
use fleet_ui_kit::{TranscriptRow, TranscriptRowKind};
use gpui::{Context, SharedString, prelude::*};

use super::{
    AgentThreadEvent, AgentThreadView, RowsKey,
    composer::ComposerMode,
    decisions::QuestionWizard,
    presentation::{self, composer_placeholder, empty_invitation, unreachable_placeholder},
    rows::{self, ResolvedGate, RowInputs, build_rows},
};

impl AgentThreadView {
    /// Replaces the daemon projection wholesale, which a thread switch does.
    pub fn set_projection(&mut self, projection: ThreadProjection, cx: &mut Context<Self>) {
        self.thread = projection.thread;
        self.projection = projection;
        self.rows_key = None;
        self.prepare(cx);
        let rows = self.rows.to_vec();
        // A thread switch is the **only** moment every measured height is worthless, so it is
        // the only caller of `set_thread` and therefore of `ListState::reset`.
        self.transcript
            .update(cx, |list, cx| list.set_thread(rows, cx));
        cx.notify();
    }

    /// Adopts the mirror's projection, patching only the rows a stream moved (§5).
    ///
    /// Structural change — a new item, a settled turn, an opened gate — rebuilds the grouping;
    /// pure text growth on already-known streaming items rewrites those rows and nothing else,
    /// so a fast model does not re-run grouping, folding or summarization per token.
    pub(crate) fn sync(&mut self, projection: &ThreadProjection, cx: &mut Context<Self>) {
        if projection.last_seq == self.projection.last_seq {
            return;
        }
        let streaming_only = streaming_only_change(&self.projection, projection, &self.row_of_item);
        let was_working = self.is_working();
        self.record_resolved_gates(projection);
        self.projection = projection.clone();
        // A new settled turn is a new checkpoint, and `[u]` is drawn only where one exists — so
        // the listing is re-read exactly then, never per frame and never per streamed token.
        let settled = self
            .projection
            .turns
            .iter()
            .filter(|turn| turn.ended.is_some())
            .count();
        if settled != self.listed_turns {
            self.listed_turns = settled;
            cx.emit(AgentThreadEvent::RefreshCheckpoints);
        }
        self.reconcile_pending();
        self.seed_controls();
        self.sync_clock();
        if streaming_only {
            let items: Vec<ItemId> = self.row_of_item.keys().copied().collect();
            let mut rows = self.rows.to_vec();
            let mut moved = false;
            for item in items {
                moved |= self.patch_streaming_row(&mut rows, item);
            }
            if moved {
                self.rows = Rc::from(rows);
                self.rows_key = self.rows_key.take().map(|key| RowsKey {
                    last_seq: self.projection.last_seq,
                    ..key
                });
                let rows = self.rows.to_vec();
                self.transcript
                    .update(cx, |list, cx| list.set_rows(rows, cx));
            }
            cx.notify();
            return;
        }
        self.prepare(cx);
        self.install_rows(cx);
        // B7.2: real tool activity in the running turn releases the first-turn anchor, and
        // releasing must drop the reserved space rather than merely re-arm follow.
        if self.has_live_work() {
            self.transcript
                .update(cx, |list, cx| list.release_anchor(cx));
        }
        // §B5.5 rule 4: `stopping…` is held until the daemon reports liveness cleared, not
        // until the interrupt request returns.
        if self.stopping && was_working && !self.is_working() {
            self.stopping = false;
        }
        cx.notify();
    }

    /// Rebuilds every prepared value behind its revision key.
    pub(crate) fn prepare(&mut self, cx: &mut Context<Self>) {
        self.sync_wizard();
        self.sync_diffs(cx);
        self.refresh_rows();
        self.refresh_decisions();
        self.refresh_metadata();
        self.sync_composer(cx);
    }

    /// Rebuilds the row model, or reuses it when nothing it depends on moved.
    fn refresh_rows(&mut self) {
        let key = RowsKey {
            thread: self.thread,
            last_seq: self.projection.last_seq,
            expanded_rev: self.expanded_rev,
            unfolded_rev: self.unfolded_rev,
            pending_rev: self.pending_rev,
            checkpoints_rev: self.checkpoints_rev,
            mode: self.composer_mode(),
        };
        if self.rows_key.as_ref() == Some(&key) {
            return;
        }
        let worktree = self.projection.worktree.slug().to_owned();
        let built = build_rows(&RowInputs {
            projection: &self.projection,
            expanded: &self.expanded,
            unfolded: &self.unfolded,
            expanded_gates: &self.expanded_gates,
            resolved: &self.resolved,
            pending: &self.pending,
            checkpoints: &self.checkpoints,
            started_at: self.started_at,
            parked: self.parked_detail(),
            empty: SharedString::from(empty_invitation(self.projection.provider, &worktree)),
        });
        self.rows = Rc::from(built.rows);
        self.targets = built.targets;
        self.row_of_item = built.streaming;
        self.rows_key = Some(key);
    }

    /// Hands the prepared rows to the list, which splices only what changed.
    fn install_rows(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows.to_vec();
        self.transcript
            .update(cx, |list, cx| list.set_rows(rows, cx));
    }

    /// Sizes the question wizard for the request that is actually open.
    ///
    /// The wizard holds one selection slot and one free-text slot per question, so it has to be
    /// sized before either can be written; a differently shaped request resets it, which is
    /// what `spec-B` §B4.3 means by "a new request resets the wizard position".
    pub(crate) fn sync_wizard(&mut self) {
        match self.open_gate().map(|gate| &gate.kind) {
            Some(fleet_core::agents::GateKind::Question { questions }) => {
                self.wizard.resize(questions.len());
            }
            _ => {
                if self.wizard != QuestionWizard::default() {
                    self.wizard = QuestionWizard::default();
                }
            }
        }
    }

    /// Rebuilds the docked decisions.
    fn refresh_decisions(&mut self) {
        self.decisions =
            super::decisions::decisions(&self.projection, &self.wizard, self.answering);
    }

    /// Rebuilds the metadata strip and bumps the fit memo's revision.
    fn refresh_metadata(&mut self) {
        let metadata = presentation::metadata_segments(&self.projection, self.interaction_mode());
        let trailing = presentation::trailing_segments(&self.projection);
        if metadata == self.metadata && trailing == self.trailing {
            return;
        }
        self.metadata = metadata;
        self.trailing = trailing;
        // The fit is memoised per `(width, revision)`; the segments changing is exactly the
        // revision bump that invalidates it, and the only one.
        self.metadata_rev = self.metadata_rev.wrapping_add(1);
    }

    /// Keeps the composer's placeholder, editability and focus ring in step with its mode.
    ///
    /// Written here, at every transition, rather than from the render body: `set_focus_visible`
    /// notifies, and a render that calls it schedules a second frame for a state the first one
    /// had already drawn (`docs/APP-CONTRACTS.md`, "Render prepares nothing").
    pub(crate) fn sync_composer(&self, cx: &mut Context<Self>) {
        let mode = self.composer_mode();
        let unreachable = self.is_unreachable();
        let choice_only = self.question_is_choice_only();
        let payload = self.approval_payload();
        let placeholder = match self.host.as_ref().filter(|_| unreachable) {
            Some(host) => unreachable_placeholder(&host.name),
            None => composer_placeholder(
                mode,
                self.projection.provider,
                payload.as_deref(),
                choice_only,
            ),
        };
        let enabled = mode.editor_enabled(choice_only) && !unreachable;
        // A question's free-text draft is per question and keyed by it, so stepping the wizard
        // brings back whatever was typed for the question it steps onto: pressing `[p]` must
        // never lose a typed answer.
        let answer =
            matches!(mode, ComposerMode::Question(_)).then(|| self.wizard.custom().to_owned());
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
            input.set_read_only(!enabled, cx);
            input.set_focus_visible(enabled, cx);
            if let Some(answer) = answer
                && input.text() != answer
            {
                input.set_text(answer, cx);
            }
        });
    }

    /// The invocation an open approval is showing, which is what `[e]` seeds the composer with.
    fn approval_payload(&self) -> Option<String> {
        match self.open_gate().map(|gate| &gate.kind) {
            Some(fleet_core::agents::GateKind::Permission { payload, .. }) => Some(payload.clone()),
            _ => None,
        }
    }

    /// Whether the question under the cursor forbids a free-text answer.
    pub(crate) fn question_is_choice_only(&self) -> bool {
        match self.open_gate().map(|gate| &gate.kind) {
            Some(fleet_core::agents::GateKind::Question { questions }) => questions
                .get(self.wizard.cursor())
                .is_some_and(|question| !question.allows_other),
            _ => false,
        }
    }

    /// The parked detail line, when a usage window parked the turn.
    fn parked_detail(&self) -> Option<SharedString> {
        match &self.projection.session {
            fleet_core::agents::SessionState::Waiting(
                fleet_core::agents::WaitingReason::UsageLimit { window, resets_at },
            ) => Some(SharedString::from(format!(
                "{window} limit resets {}",
                resets_at.format("%H:%M")
            ))),
            _ => None,
        }
    }

    /// §3.3's `Working`, over the same inputs the row build uses.
    pub(crate) fn row_inputs_working(&self) -> bool {
        matches!(self.projection.turn, TurnState::Running(_))
            || self.projection.session == fleet_core::agents::SessionState::Running
            || self.projection.session == fleet_core::agents::SessionState::Starting
            || !self.projection.background_tasks.is_empty()
            || self.projection.retrying.is_some()
            || !self.pending.is_empty()
    }

    /// Whether the running turn has a live work row, which is what releases the anchor.
    fn has_live_work(&self) -> bool {
        self.rows.iter().any(|row| {
            matches!(
                row.kind,
                TranscriptRowKind::WorkLive(_) | TranscriptRowKind::Work(_)
            )
        })
    }

    /// Starts, keeps or drops the live row's clock.
    ///
    /// The row carries `started_at` and ticks itself; no elapsed figure ever enters the model,
    /// so one element repaints per second instead of the whole tree.
    pub(crate) fn sync_clock(&mut self) {
        match self.projection.turn {
            TurnState::Running(turn) => {
                if self.clock_turn != Some(turn) {
                    self.clock_turn = Some(turn);
                    self.started_at = Some(Instant::now());
                }
            }
            _ if self.is_working() => {
                if self.started_at.is_none() {
                    self.started_at = Some(Instant::now());
                }
            }
            _ => {
                self.clock_turn = None;
                self.started_at = None;
            }
        }
    }

    /// Seeds the control draft from what the harness reports, without overwriting a human pick.
    ///
    /// Tier two of §B6.4 in miniature: `model_explicit` is why this is safe to call on every
    /// sync — a seeded selection may be replaced by a later seed, a human's pick never is.
    fn seed_controls(&mut self) {
        let Some(model) = self.projection.model.clone() else {
            return;
        };
        let instance = SharedString::new_static(self.projection.provider.executable());
        if self.controls.instance() == Some(&instance) && self.controls.is_explicit() {
            return;
        }
        self.controls.seed_model(instance, model);
    }

    /// Records every gate that closed between the projection in hand and the next one.
    ///
    /// A gate this window answered carries its own answer; one that closed with no answer from
    /// here was withdrawn, resolved by another client, or declared stale — all three read as
    /// `withdrawn · the agent stopped waiting`, which is what actually happened from this
    /// window's point of view.
    fn record_resolved_gates(&mut self, next: &ThreadProjection) {
        for gate in &self.projection.gates {
            if next.gates.iter().any(|open| open.id == gate.id) {
                continue;
            }
            let answer = self.answered.remove(&gate.id);
            let outcome = outcome_of(answer.as_ref());
            self.resolved.push(ResolvedGate {
                gate: gate.clone(),
                outcome,
                answer,
            });
            if self.answering == Some(gate.id) {
                self.answering = None;
            }
            self.expanded_rev = self.expanded_rev.wrapping_add(1);
        }
    }

    /// Drops the optimistic bubbles the projection has now echoed back.
    ///
    /// The comparison is against the echo count recorded at dispatch, not "any user item with
    /// this text", so sending the same message twice does not clear both bubbles on the first
    /// echo. The daemon cannot yet adopt the client's item id — `UserInput` carries no id field
    /// — so text is the join; see this module's integration notes.
    fn reconcile_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let before = self.pending.len();
        let projection = &self.projection;
        self.pending.retain(|pending| {
            let echoes = count_user_items(projection, &pending.text);
            echoes <= pending.echoes
        });
        if self.pending.len() != before {
            self.pending_rev = self.pending_rev.wrapping_add(1);
        }
    }

    /// Rewrites the text of the row that carries `item`, returning whether it moved.
    fn patch_streaming_row(&self, rows: &mut [TranscriptRow], item: ItemId) -> bool {
        let Some(index) = self.row_of_item.get(&item).copied() else {
            return false;
        };
        let Some(source) = self
            .projection
            .items
            .iter()
            .find(|candidate| candidate.id == item)
        else {
            return false;
        };
        let text = rows::item_text(source);
        let Some(row) = rows.get_mut(index) else {
            return false;
        };
        match &mut row.kind {
            TranscriptRowKind::Assistant(assistant) => {
                let markdown = fleet_ui_kit::parse_markdown_document(&text);
                if assistant.markdown == markdown {
                    return false;
                }
                assistant.markdown = markdown;
                assistant.empty = text.trim().is_empty();
            }
            TranscriptRowKind::Reasoning(reasoning) => {
                let text = SharedString::from(text);
                if reasoning.text == text {
                    return false;
                }
                reasoning.text = text;
            }
            _ => return false,
        }
        true
    }

    /// Builds, updates and drops the inline diff surface of every item that carries a patch.
    fn sync_diffs(&mut self, cx: &mut Context<Self>) {
        let patches: Vec<(SharedString, Option<String>, String)> = self
            .projection
            .items
            .iter()
            .filter_map(|item| {
                let ItemKind::Tool(call) = &item.kind else {
                    return None;
                };
                let diff = call.diff.as_ref()?;
                Some((
                    SharedString::from(item.id.to_string()),
                    Some(diff.path.display().to_string()),
                    diff.unified.clone(),
                ))
            })
            .collect();
        let mut diffs = self.diffs.borrow_mut();
        diffs.retain(|id, _| patches.iter().any(|(known, _, _)| known == id));
        for (id, path, unified) in patches {
            match diffs.get(&id) {
                Some(view) => view.update(cx, |view, cx| view.set_unified(unified, cx)),
                None => {
                    let view = cx.new(|cx| DiffView::for_path(path, unified, cx));
                    diffs.insert(id, view);
                }
            }
        }
    }
}

/// What a settled gate turned out to be, from the answer this window sent.
fn outcome_of(answer: Option<&fleet_core::agents::GateAnswer>) -> fleet_ui_kit::GateOutcome {
    use fleet_core::agents::{GateAnswer, PermissionChoice};
    use fleet_ui_kit::GateOutcome;

    match answer {
        Some(GateAnswer::Permission { choice, .. }) => match choice {
            PermissionChoice::AllowOnce
            | PermissionChoice::AllowSession
            | PermissionChoice::AllowDirectory
            | PermissionChoice::Edit => GateOutcome::Allowed,
            PermissionChoice::Deny | PermissionChoice::DenyAndStop => GateOutcome::Declined,
        },
        Some(GateAnswer::Question { .. }) => GateOutcome::Answered,
        Some(GateAnswer::Plan(fleet_core::agents::PlanAnswer::Approve)) => GateOutcome::Allowed,
        Some(GateAnswer::Plan(_)) => GateOutcome::Answered,
        None => GateOutcome::Withdrawn,
    }
}

/// How many user messages in a projection carry exactly this text.
pub(crate) fn count_user_items(projection: &ThreadProjection, text: &str) -> usize {
    projection
        .items
        .iter()
        .filter(|item| match &item.kind {
            ItemKind::UserMessage { text: sent, .. } => sent.trim() == text.trim(),
            _ => false,
        })
        .count()
}

/// Whether one projection differs from the next only by text on items that already have rows.
///
/// A free function so the rule can be asserted without a window: it decides whether `sync` takes
/// the cheap streaming path, and a part of the projection missing from the comparison is a row
/// that silently never appears.
fn streaming_only_change(
    current: &ThreadProjection,
    next: &ThreadProjection,
    row_of_item: &HashMap<ItemId, usize>,
) -> bool {
    let structure_held = next.items.len() == current.items.len()
        && next.turns.len() == current.turns.len()
        && next.gates.len() == current.gates.len()
        // §5 gives notices and checkpoints rows of their own, and a `GateResolved` immediately
        // followed by a `GateOpened` keeps the count at one while replacing the decision the
        // drawer draws: every row-bearing part of the projection has to be compared.
        && next.notices.len() == current.notices.len()
        && next.checkpoints.len() == current.checkpoints.len()
        && next.background_tasks == current.background_tasks
        && next
            .gates
            .iter()
            .zip(&current.gates)
            .all(|(next, current)| next.id == current.id && next.kind == current.kind)
        && next.turn == current.turn
        && next.session == current.session
        && next.retrying == current.retrying
        && next.mode == current.mode
        && next.exit_code == current.exit_code
        && next
            .turns
            .iter()
            .zip(&current.turns)
            .all(|(next, current)| next.ended.is_some() == current.ended.is_some());
    if !structure_held {
        return false;
    }
    next.items
        .iter()
        .zip(&current.items)
        .all(|(next, current)| {
            let same_row = next.id == current.id
                && next.status == current.status
                && next.parent == current.parent
                && next.children == current.children;
            // Text may only have grown on an item this view already has a row for; a text
            // change anywhere else is structural as far as the row model is concerned.
            same_row
                && (next.kind == current.kind
                    || (row_of_item.contains_key(&next.id)
                        && matches!(
                            (&next.kind, &current.kind),
                            (
                                ItemKind::AssistantText { .. },
                                ItemKind::AssistantText { .. }
                            ) | (ItemKind::Reasoning { .. }, ItemKind::Reasoning { .. })
                        )))
        })
}
