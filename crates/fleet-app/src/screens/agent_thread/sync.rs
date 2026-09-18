//! Adopting the daemon's projection, and preparing everything `render` will compose.
//!
//! The split this module exists for: a **structural** change — a new item, a settled turn, an
//! opened gate — rebuilds the grouping, the folds and the footers, while the reducer's
//! [`Applied::Text`] description rewrites one existing row. The view never rediscovers that fact
//! by walking or deep-comparing the projection.

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use fleet_core::agents::{Applied, ItemKind, StreamKind, ThreadProjection, TurnState};
use fleet_lazygit::diff_view::DiffView;
use fleet_ui_kit::{ActiveTheme, TranscriptRowKind};
use gpui::{Context, SharedString, prelude::*};

use super::{
    AgentThreadEvent, AgentThreadView, RowsKey,
    composer::ComposerMode,
    decisions::QuestionWizard,
    presentation::{self, composer_placeholder, empty_invitation, unreachable_placeholder},
    reveal::RevealChunk,
    rows::{self, ResolvedGate, RowInputs, build_rows},
};

impl AgentThreadView {
    /// Replaces the daemon projection wholesale, which a thread switch does.
    pub fn set_projection(&mut self, projection: ThreadProjection, cx: &mut Context<Self>) {
        self.flush_reveal(cx);
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
    pub(crate) fn sync(
        &mut self,
        projection: &ThreadProjection,
        applied: &Applied,
        cx: &mut Context<Self>,
    ) {
        self.sync_batch(projection, std::slice::from_ref(applied), cx);
    }

    /// Adopts one bridge batch. Pure text suffixes stay incremental; any structural member makes
    /// the batch one structural adoption so grouping runs at most once.
    pub(crate) fn sync_batch(
        &mut self,
        projection: &ThreadProjection,
        applied: &[Applied],
        cx: &mut Context<Self>,
    ) {
        if projection.last_seq == self.projection.last_seq {
            return;
        }
        let described_tail = u64::try_from(applied.len()).ok().is_some_and(|count| {
            self.projection.last_seq.0.saturating_add(count) == projection.last_seq.0
        });
        let retrying_changed = self.projection.retrying != projection.retrying;
        let mut every_text_was_queued = true;
        for change in applied {
            if matches!(change, Applied::Text { .. }) && !self.queue_text(projection, change, cx) {
                every_text_was_queued = false;
            }
        }
        if described_tail
            && !applied.is_empty()
            && applied
                .iter()
                .all(|change| matches!(change, Applied::Text { .. }))
            && every_text_was_queued
            && !retrying_changed
        {
            self.projection.last_seq = projection.last_seq;
            self.projection.last_activity = projection.last_activity;
            self.projection.last_nonterminal_seq = projection.last_nonterminal_seq;
            self.projection.retrying.clone_from(&projection.retrying);
            self.rows_key = self.rows_key.take().map(|key| RowsKey {
                last_seq: projection.last_seq,
                ..key
            });
            return;
        }

        // Text descriptions are a filtered subset of a bridge batch: a final delta followed by
        // `ItemCompleted` advances the projection twice but contributes only one entry here. The
        // suffix was queued above; flush it through `patch_row` before rebuilding structural rows
        // so the growing row is remeasured and never enters the structural splice.
        self.flush_reveal(cx);
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

    fn queue_text(
        &mut self,
        projection: &ThreadProjection,
        applied: &Applied,
        cx: &mut Context<Self>,
    ) -> bool {
        let Applied::Text {
            item,
            stream,
            appended,
        } = applied
        else {
            return false;
        };
        if !self.row_of_item.contains_key(item) {
            return false;
        }
        let Ok(text) = projection.stream_text(*item, *stream) else {
            return false;
        };
        let Some(suffix) = text.get(appended.clone()) else {
            return false;
        };
        let tick_ms = cx.theme().motion.reveal_tick_ms;
        let horizon_ms = cx.theme().motion.reveal_horizon_ms;
        self.reveal
            .push(*item, *stream, suffix.to_owned(), tick_ms, horizon_ms);
        if cx.reduce_motion() {
            self.flush_reveal(cx);
        } else {
            self.start_reveal_task(tick_ms, cx);
        }
        true
    }

    fn start_reveal_task(&mut self, tick_ms: u64, cx: &mut Context<Self>) {
        if self.reveal_running || self.reveal.is_empty() {
            return;
        }
        self.reveal_running = true;
        let tick = Duration::from_millis(tick_ms);
        self.reveal_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(tick).await;
                let keep_running = this
                    .update(cx, |this, cx| {
                        let chunks = if cx.reduce_motion() {
                            this.reveal.drain()
                        } else {
                            this.reveal.take_tick()
                        };
                        for chunk in chunks {
                            this.apply_reveal(chunk, cx);
                        }
                        let pending = !this.reveal.is_empty();
                        if !pending {
                            this.reveal_running = false;
                        }
                        pending
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        }));
    }

    fn flush_reveal(&mut self, cx: &mut Context<Self>) {
        self.reveal_running = false;
        self.reveal_task = None;
        for chunk in self.reveal.drain() {
            self.apply_reveal(chunk, cx);
        }
    }

    fn apply_reveal(&mut self, chunk: RevealChunk, cx: &mut Context<Self>) {
        if let Err(error) =
            self.projection
                .append_stream_text(chunk.item, chunk.stream, &chunk.text)
        {
            tracing::warn!(%error, item = %chunk.item, "discarded an invalid reveal suffix");
            return;
        }
        let Some(index) = self.row_of_item.get(&chunk.item).copied() else {
            return;
        };
        let Some(current) = self.rows.get(index) else {
            return;
        };
        let Some(source) = self.projection.item(chunk.item) else {
            return;
        };
        let mut row = current.clone();
        match &mut row.kind {
            TranscriptRowKind::Assistant(assistant) => {
                Rc::make_mut(&mut assistant.markdown).append(&chunk.text);
                assistant.empty = self
                    .projection
                    .stream_text(chunk.item, StreamKind::AssistantText)
                    .is_ok_and(|text| text.trim().is_empty());
            }
            TranscriptRowKind::Reasoning(reasoning) => {
                reasoning.text = SharedString::from(rows::item_text(source));
            }
            TranscriptRowKind::Work(work) => {
                let ItemKind::Tool(call) = &source.kind else {
                    return;
                };
                *work = rows::item::projected_tool_row(source, call, work.expanded);
            }
            // Output hidden by the fixed-height live row changes no painted payload.
            TranscriptRowKind::WorkLive(_) if matches!(&source.kind, ItemKind::Tool(_)) => return,
            // Plan streams are rare, but still stay on the one-row path. Their promoted heading
            // means the whole row payload must be refreshed even though grouping does not.
            TranscriptRowKind::Plan(_) => {
                let items = std::collections::HashMap::from([(source.id, source)]);
                let inputs = RowInputs {
                    projection: &self.projection,
                    expanded: &self.expanded,
                    unfolded: &self.unfolded,
                    expanded_gates: &self.expanded_gates,
                    resolved: &self.resolved,
                    pending: &self.pending,
                    checkpoints: &self.checkpoints,
                    started_at: self.started_at,
                    parked: self.parked_detail(),
                    empty: SharedString::default(),
                };
                let Some((replacement, _)) = rows::item::rows_for(&inputs, &items, source).pop()
                else {
                    return;
                };
                row = replacement;
            }
            _ => return,
        }
        Rc::make_mut(&mut self.rows)[index] = row.clone();
        self.transcript
            .update(cx, |list, cx| list.patch_row(index, row, cx));
        #[cfg(test)]
        {
            self.patched_rows += 1;
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
            || self.pending.iter().any(|pending| !pending.failed)
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
    /// The join is by **id** (§9.2): the client-minted `ItemId` went out with the message, both
    /// adapters adopt it, and the daemon records the user item under it. Text is only the
    /// fallback for a daemon that did not adopt the id, and there it compares against the echo
    /// count recorded at dispatch so sending the same message twice does not clear both bubbles
    /// on the first echo. An id match also clears a failed bubble whose reply was lost with the
    /// socket; the text fallback never does, because a failed send may genuinely be absent.
    fn reconcile_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let before = self.pending.len();
        let projection = &self.projection;
        self.pending.retain(|pending| {
            if projection.items.iter().any(|item| item.id == pending.id) {
                return false;
            }
            if pending.failed {
                return true;
            }
            let echoes = count_user_items(projection, &pending.text);
            echoes <= pending.echoes
        });
        if self.pending.len() != before {
            self.pending_rev = self.pending_rev.wrapping_add(1);
        }
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
