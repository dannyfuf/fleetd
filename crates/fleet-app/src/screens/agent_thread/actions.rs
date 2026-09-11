//! Every intent the agent tab's keys and clicks route to (§7, §12, `spec-B` §B5/§B8).
//!
//! The keymap resolves a key to an action, the workspace root dispatches it here, and this is
//! where it becomes either local state or one typed [`BridgeCommand`]. Nothing in this file
//! renders, and nothing here reads the view's own elements: an intent is a function of daemon
//! state plus the composer's text.

use fleet_core::agents::{GateAnswer, GateKind, ItemId, ItemKind, ModelSelection, PermissionMode};
use fleet_ui_kit::{
    DecisionAction, MultilineInput, RowAction, TranscriptList, TranscriptRow, TranscriptRowKind,
};
use gpui::{Context, SharedString};

use crate::bridge::BridgeCommand;

use super::{
    AgentThreadEvent, AgentThreadView,
    composer::{
        ComposerMode, InteractionMode, Refusal, RestartInputs, Submit, restart_with_resume,
        strip_send_time_context, submit_gate, submit_intent,
    },
    decisions::{self, Routed},
    picker::{Picker, PickerKind},
    presentation::{mode_label, unreachable_notice},
    rows::{PendingSend, RowTarget},
    user_input,
};

impl AgentThreadView {
    // -- the composer ----------------------------------------------------------------------

    /// `⏎`: accept a completion, answer the open decision, else send.
    pub(crate) fn send(&mut self, cx: &mut Context<Self>) {
        if self.accept_completion(cx) {
            return;
        }
        let text = self.input.read(cx).text().to_owned();
        self.send_text(text, cx);
    }

    /// The same intent over text the composer reported rather than text it still holds.
    ///
    /// [`MultilineInput::submit`] has already emptied the buffer and pushed the entry to history
    /// by the time the event arrives, so re-reading the composer would send nothing.
    pub(crate) fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        let mode = self.composer_mode();

        // A question's `⏎` is an answer, not a turn: the composer *is* the free-text field.
        if matches!(mode, ComposerMode::Question(_)) {
            self.act_on_decision(&DecisionAction::Answer, cx);
            return;
        }
        // An approval correction typed after `[e]` allows the corrected invocation and only the
        // corrected one: an `AllowOnce` with a payload beside it would run the original command
        // while the user believes their correction went out.
        if let Some(gate) = self.gate_draft
            && self
                .projection
                .gates
                .iter()
                .any(|open| open.id == gate && matches!(open.kind, GateKind::Permission { .. }))
        {
            self.gate_draft = None;
            self.answering = Some(gate);
            let answer = GateAnswer::Permission {
                choice: fleet_core::agents::PermissionChoice::Edit,
                edited_payload: Some(text),
            };
            self.remember_answer(gate, answer.clone());
            self.input.update(cx, MultilineInput::clear);
            self.dispatch(
                BridgeCommand::AgentRespond {
                    thread: self.thread,
                    gate,
                    answer,
                },
                cx,
            );
            self.prepare(cx);
            return;
        }
        // A plan's refinement is the same `⏎`: whether it implements or refines is decided by
        // whether the composer holds anything.
        if let ComposerMode::PlanFollowUp(_) = mode {
            let action = if text.is_empty() {
                DecisionAction::Implement
            } else {
                DecisionAction::Refine
            };
            self.act_on_decision(&action, cx);
            return;
        }

        if let Err(refusal) = submit_gate(&text, 0, self.pending.len(), self.is_unreachable()) {
            self.refuse_send(refusal, text, cx);
            return;
        }
        self.dispatch_turn(text, cx);
    }

    /// `⌘⏎`: the same send, decided by the one function that decides `⏎`.
    ///
    /// On a thread that has not started it starts it in the background and keeps the composer;
    /// on a started thread it is identical to `⏎`, which is why the intent is one function and
    /// the difference is a return value rather than a second code path.
    pub(crate) fn send_background(&mut self, cx: &mut Context<Self>) {
        let is_new_thread = self.projection.turns.is_empty();
        let text = self.input.read(cx).text().to_owned();
        match submit_intent(false, true, is_new_thread) {
            Some(Submit::Background) => {
                self.send_text(text, cx);
                self.notice("started in background", cx);
            }
            Some(Submit::Foreground) => self.send_text(text, cx),
            // `⇧⏎` never reaches an action: the composer owns it, and a `None` here would mean
            // the keymap grew a modifier the composer already handles.
            None => {}
        }
    }

    /// States why a send was refused, in the one place the refusal can still keep the draft.
    fn refuse_send(&mut self, refusal: Refusal, text: String, cx: &mut Context<Self>) {
        match refusal {
            // The daemon cannot carry this to a machine it has no link to. The draft goes back
            // — `submit` already emptied the buffer — so the message survives until the link
            // does come back.
            Refusal::Unreachable => {
                if let Some(host) = self.host.clone() {
                    let notice = unreachable_notice(&host.name);
                    self.input.update(cx, |input, cx| input.set_text(text, cx));
                    self.notice(notice, cx);
                    cx.notify();
                }
            }
            // A dispatch of ours has not landed yet. Saying so is better than sending twice.
            Refusal::Unacknowledged => {
                self.input.update(cx, |input, cx| input.set_text(text, cx));
                self.notice("the previous message has not been acknowledged yet", cx);
            }
            Refusal::Empty => {}
        }
    }

    /// Sends one turn, with the optimistic bubble that makes it land in the same frame.
    ///
    /// A message sent while a turn runs is a **steer**, dispatched immediately. There is no
    /// queue and no queued row: the shipped queue lived in a GPUI view — lost on app restart,
    /// invisible to `fleet agent` — and it re-implemented what both harnesses already do.
    fn dispatch_turn(&mut self, text: String, cx: &mut Context<Self>) {
        let steered = self.is_working();
        let first = self.projection.turns.is_empty() && self.pending.is_empty();
        self.input.update(cx, |input, cx| {
            input.push_history(text.clone());
            input.clear(cx);
        });
        // Client-minted, and the row's identity for its whole life: the id goes out with the
        // message, the adapter adopts it, and reconciliation is by id with no temp-id swap.
        let item = ItemId::new();
        self.pending.push(PendingSend {
            id: item,
            text: strip_send_time_context(&text),
            steered,
            failed: false,
            echoes: super::sync::count_user_items(&self.projection, &text),
        });
        self.pending_rev = self.pending_rev.wrapping_add(1);
        self.send_controls(cx);
        self.dispatch(
            BridgeCommand::AgentSend {
                thread: self.thread,
                input: user_input(text, item),
            },
            cx,
        );
        self.prepare(cx);
        self.install_rows_for_send(cx);
        // B7.2/B7.5: the **first** user message of a thread anchors near the top; every later
        // send goes straight to following the end.
        self.transcript.update(cx, |list, cx| {
            if first {
                list.anchor_new_turn(cx);
            } else {
                list.scroll_to_latest(cx);
            }
        });
    }

    /// Dispatches the control draft ahead of the turn, in §B6.4's order.
    ///
    /// Metadata, then the access ladder, then the interaction mode, then the turn. Nothing
    /// reaches the daemon before this moment, which is what makes every picker instant over an
    /// SSH link — and it is why **nothing applies mid-turn**.
    fn send_controls(&mut self, cx: &mut Context<Self>) {
        let wire_mode = self.controls.wire_mode(self.projection.mode);
        let model = self.controls.model().cloned();
        let model_changed = model
            .as_ref()
            .is_some_and(|model| self.projection.model.as_ref() != Some(model));
        let mode_changed = wire_mode != self.projection.mode
            && !matches!(
                (self.projection.mode, wire_mode),
                (PermissionMode::Plan, _) | (_, PermissionMode::Plan)
            );
        let restart = restart_with_resume(RestartInputs {
            mode_changed,
            cwd_changed: false,
            instance_changed: false,
            model_changed,
            // Nothing has probed the harness yet, so the conservative answer is the truthful
            // one: a control Fleet cannot prove is live is treated as a restart.
            can_switch_model: false,
            controls_ride_the_turn: self.projection.provider
                == fleet_core::agents::AgentKind::Codex,
        });
        if model_changed || mode_changed {
            // One structured line with every input to the decision. The first time a user
            // reports "it lost my context" it pays for itself.
            tracing::debug!(
                thread = %self.thread,
                mode_changed,
                model_changed,
                restart,
                has_resume_cursor = !self.projection.turns.is_empty(),
                "agent control change"
            );
        }
        if let Some(model) = model.filter(|_| model_changed) {
            if restart {
                self.publish_starting(cx);
            }
            self.dispatch(
                BridgeCommand::AgentSetModel {
                    thread: self.thread,
                    model,
                },
                cx,
            );
        }
        if wire_mode != self.projection.mode {
            if mode_changed {
                self.publish_starting(cx);
            }
            self.dispatch(
                BridgeCommand::AgentSetMode {
                    thread: self.thread,
                    mode: wire_mode,
                },
                cx,
            );
        }
    }

    /// Publishes `session = Starting` optimistically, so the tab says `starting…` in the same
    /// frame rather than after the round trip (§7.1).
    fn publish_starting(&mut self, cx: &mut Context<Self>) {
        if self.projection.session == fleet_core::agents::SessionState::Starting {
            return;
        }
        self.projection.session = fleet_core::agents::SessionState::Starting;
        self.rows_key = None;
        self.sync_clock();
        self.prepare(cx);
        cx.notify();
    }

    /// Installs the rows a send just produced without resetting a measured height.
    pub(crate) fn install_rows_for_send(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows.to_vec();
        self.transcript
            .update(cx, |list, cx| list.set_rows(rows, cx));
    }

    /// The composer changed: re-filter the open picker on what follows its trigger.
    pub(crate) fn on_composer_changed(&mut self, cx: &mut Context<Self>) {
        // A question's free-text answer is the composer's text, and typing it clears the
        // selection: the two are mutually exclusive, enforced in the model.
        if !self.composer_mode().binds_draft() {
            let text = self.input.read(cx).text().to_owned();
            self.sync_wizard();
            self.wizard.set_custom(text);
            self.prepare(cx);
            cx.notify();
            return;
        }
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let trigger = self.input.read(cx).active_trigger();
        match trigger.filter(|trigger| PickerKind::for_trigger(trigger.symbol) == Some(picker.kind))
        {
            Some(trigger) => picker.filter(&trigger.query),
            // The caret left the trigger, so the surface it opened has nothing left to filter.
            None => self.picker = None,
        }
        cx.notify();
    }

    /// `esc`: the cascade, in order — picker, gate draft, scroll mode, interrupt.
    ///
    /// On an idle thread with nothing open, `esc` does nothing. It never quits.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            cx.notify();
            return;
        }
        // A correction or a refinement is abandoned before anything else `esc` means: the gate
        // it was being typed for is still open behind it.
        if self.gate_draft.take().is_some() {
            self.input.update(cx, MultilineInput::clear);
            self.prepare(cx);
            cx.notify();
            return;
        }
        if self.scrolling {
            self.set_scroll_mode(false, cx);
            return;
        }
        // An approval's `esc` is `deny and stop`, which is a different thing from an interrupt
        // and is reachable on both harnesses.
        if matches!(self.composer_mode(), ComposerMode::Approval(_)) {
            self.act_on_decision(&DecisionAction::DenyAndStop, cx);
            return;
        }
        if !self.is_working() {
            return;
        }
        // Rule 2: holding `esc` fires one interrupt, not forty.
        if self.stopping {
            return;
        }
        self.stopping = true;
        self.dispatch(
            BridgeCommand::AgentInterrupt {
                thread: self.thread,
            },
            cx,
        );
    }

    /// `⇧⇥`: toggle Build ⇄ Plan, which takes effect at the next send.
    ///
    /// Leaving plan mode restores the **base** access ladder rather than a hardcoded default,
    /// because the two are separate axes in the draft and only collapse on the wire.
    pub(crate) fn toggle_plan_mode(&mut self, cx: &mut Context<Self>) {
        let next = match self.interaction_mode() {
            InteractionMode::Build => InteractionMode::Plan,
            InteractionMode::Plan => InteractionMode::Build,
        };
        self.controls.set_interaction(next);
        self.prepare(cx);
        cx.notify();
    }

    /// `↑` / `ctrl-p`: move the open picker, else recall or move the caret.
    pub(crate) fn history(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut() {
            picker.retreat();
            cx.notify();
            return;
        }
        // §B5.2: `↑` recalls nothing under an approval or a question — recalling a past prompt
        // into an answer field would answer the harness with an unrelated message.
        if !self.composer_mode().history_enabled() {
            return;
        }
        self.input.update(cx, |input, cx| input.caret_up(cx));
        cx.notify();
    }

    /// `↓` / `ctrl-n`: the same, forwards.
    pub(crate) fn history_next(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut() {
            picker.advance();
            cx.notify();
            return;
        }
        if !self.composer_mode().history_enabled() {
            return;
        }
        self.input.update(cx, |input, cx| input.caret_down(cx));
        cx.notify();
    }

    // -- completion surfaces ---------------------------------------------------------------

    /// `@`, `$`, `/`, `^s m`, `^s e`, `^s t`: open a surface, or advance the open one.
    pub(crate) fn open_picker(&mut self, kind: PickerKind, cx: &mut Context<Self>) {
        if kind.completes_text() && !self.composer_mode().triggers_enabled() {
            return;
        }
        match self.picker.as_mut() {
            Some(picker) if picker.kind == kind => picker.advance(),
            _ => {
                let candidates = self.candidates(kind);
                // DESIGN-SYSTEM §4: an invalid command is not listed. A picker with nothing to
                // offer draws nothing, and would still swallow the next `⏎`.
                if candidates.is_empty() {
                    self.picker = None;
                    cx.notify();
                    return;
                }
                self.picker = Some(Picker::new(kind, candidates));
            }
        }
        let query = self
            .input
            .read(cx)
            .active_trigger()
            .filter(|trigger| PickerKind::for_trigger(trigger.symbol) == Some(kind))
            .map(|trigger| trigger.query.to_string())
            .unwrap_or_default();
        if let Some(picker) = self.picker.as_mut() {
            picker.filter(&query);
        }
        cx.notify();
    }

    /// What one surface offers.
    fn candidates(&self, kind: PickerKind) -> Vec<String> {
        match kind {
            PickerKind::Files => self.files.clone(),
            // Fleet's own built-ins lead, then the harness's commands.
            PickerKind::Commands => ["model", "plan", "default", "compact"]
                .into_iter()
                .map(str::to_owned)
                .chain(self.commands.iter().cloned())
                .collect(),
            PickerKind::Skills => self.skills.clone(),
            PickerKind::Models => self.model_candidates(),
            PickerKind::Traits => self.trait_candidates(),
            PickerKind::Access => ACCESS_LADDER
                .iter()
                .filter(|(mode, _)| *mode != self.controls.access(self.projection.mode))
                .map(|(_, label)| (*label).to_owned())
                .collect(),
        }
    }

    /// The models `^s m` offers.
    ///
    /// Fleet never hardcodes an effort ladder: the legal set is the harness's own, and until a
    /// probe publishes one the picker offers the model the session reports and nothing invented.
    fn model_candidates(&self) -> Vec<String> {
        self.projection
            .model
            .iter()
            .map(|model| model.model.clone())
            .collect()
    }

    /// The traits `^s e` offers, which are the harness's declared options and no others.
    fn trait_candidates(&self) -> Vec<String> {
        Vec::new()
    }

    /// `⏎` while a surface is open: accept its highlighted row.
    fn accept_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(picker) = self.picker.take() else {
            return false;
        };
        let Some(accepted) = picker.accepted() else {
            cx.notify();
            return true;
        };
        match picker.kind {
            PickerKind::Models => self.pick_model(accepted.to_string(), cx),
            PickerKind::Access => self.pick_access(&accepted, cx),
            PickerKind::Traits => {}
            PickerKind::Commands => self.accept_command(&picker, &accepted, cx),
            PickerKind::Files | PickerKind::Skills => {
                self.replace_trigger(&picker, &accepted, cx);
            }
        }
        cx.notify();
        true
    }

    /// A built-in command is applied locally and **deleted from the text**; a harness command is
    /// inserted and the harness expands it.
    fn accept_command(&mut self, picker: &Picker, accepted: &SharedString, cx: &mut Context<Self>) {
        match accepted.as_ref() {
            "model" => {
                self.clear_trigger(picker, cx);
                self.open_picker(PickerKind::Models, cx);
            }
            "plan" => {
                self.clear_trigger(picker, cx);
                self.controls.set_interaction(InteractionMode::Plan);
                self.prepare(cx);
            }
            "default" => {
                self.clear_trigger(picker, cx);
                self.controls.set_interaction(InteractionMode::Build);
                self.prepare(cx);
            }
            _ => self.replace_trigger(picker, accepted, cx),
        }
    }

    /// Replaces the trigger and the query it filtered on with the accepted row.
    ///
    /// The replacement is guarded against the text it expects to replace, so a racing edit
    /// cannot corrupt the prompt: `@src/li` + `src/lib.rs` is `@src/lib.rs `, never
    /// `@src/lisrc/lib.rs `.
    fn replace_trigger(&mut self, picker: &Picker, accepted: &str, cx: &mut Context<Self>) {
        let Some(symbol) = picker.kind.prefix() else {
            return;
        };
        let text = self.input.read(cx).text().to_owned();
        let kept = text
            .rfind(symbol)
            .map_or(text.len(), |at| at + symbol.len_utf8());
        let text = format!("{}{accepted} ", &text[..kept]);
        self.input.update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Deletes a built-in's own trigger text, because a built-in sends nothing.
    fn clear_trigger(&mut self, picker: &Picker, cx: &mut Context<Self>) {
        let Some(symbol) = picker.kind.prefix() else {
            return;
        };
        let text = self.input.read(cx).text().to_owned();
        let kept = text.rfind(symbol).unwrap_or(0);
        let text = text[..kept].to_owned();
        self.input.update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Records a human's model pick, which no later seed may overwrite.
    fn pick_model(&mut self, model: String, cx: &mut Context<Self>) {
        let instance = SharedString::new_static(self.projection.provider.executable());
        let effort = self
            .projection
            .model
            .as_ref()
            .and_then(|current| current.effort.clone());
        self.controls.pick_model(
            instance,
            ModelSelection {
                model,
                effort,
                provider: None,
            },
        );
        self.prepare(cx);
        cx.notify();
    }

    /// Records the access ladder, which takes effect at the next send.
    fn pick_access(&mut self, label: &str, cx: &mut Context<Self>) {
        let Some((mode, _)) = ACCESS_LADDER.iter().find(|(_, name)| *name == label) else {
            return;
        };
        self.controls.set_access(*mode);
        self.prepare(cx);
        cx.notify();
    }

    // -- decisions -------------------------------------------------------------------------

    /// Routes one bare key, or a click on a hint, to the decision that owns the keyboard.
    pub(crate) fn act_on_decision(&mut self, action: &DecisionAction, cx: &mut Context<Self>) {
        // While a reply is in flight every option is disabled, so a second keystroke — or a
        // click on a hint the drawer is still drawing — must not dispatch the same answer twice.
        if self
            .composer_mode()
            .gate()
            .is_some_and(|gate| self.answering == Some(gate))
        {
            return;
        }
        let typed = self.input.read(cx).text().trim().to_owned();
        // A plan the harness produced as an item has no gate: its verbs send a turn.
        if let Some(gate) = self.open_gate().cloned() {
            let routed = decisions::route(&gate, action, &mut self.wizard, &typed);
            self.apply_routed(routed, Some(gate.id), cx);
            return;
        }
        if let Some((_, markdown)) = decisions::plan_item(&self.projection) {
            let routed = decisions::route_plan_item(action, &markdown, &typed);
            self.apply_routed(routed, None, cx);
        }
    }

    /// Applies whatever a routed key decided.
    fn apply_routed(
        &mut self,
        routed: Routed,
        gate: Option<fleet_core::agents::GateId>,
        cx: &mut Context<Self>,
    ) {
        match routed {
            Routed::Answer(answer) => {
                let Some(gate) = gate else {
                    return;
                };
                // Optimistically disable the drawer rather than clearing it: a transient
                // failure restores it to pending, and only a terminal event closes it.
                self.answering = Some(gate);
                self.remember_answer(gate, answer.clone());
                self.gate_draft = None;
                self.wizard = decisions::QuestionWizard::default();
                self.input.update(cx, MultilineInput::clear);
                self.dispatch(
                    BridgeCommand::AgentRespond {
                        thread: self.thread,
                        gate,
                        answer,
                    },
                    cx,
                );
                self.prepare(cx);
            }
            Routed::Send { text, plan_mode } => {
                self.controls.set_interaction(if plan_mode {
                    InteractionMode::Plan
                } else {
                    InteractionMode::Build
                });
                self.input.update(cx, MultilineInput::clear);
                self.dispatch_turn(text, cx);
            }
            Routed::Compose(seed) => {
                self.gate_draft = gate;
                self.input.update(cx, |input, cx| input.set_text(seed, cx));
                self.prepare(cx);
                cx.notify();
            }
            Routed::Local => {
                self.prepare(cx);
                self.install_rows_for_send(cx);
                cx.notify();
            }
            Routed::None => {}
        }
    }

    /// Remembers what this window answered a gate with, so its settled record can say so.
    pub(crate) fn remember_answer(&mut self, gate: fleet_core::agents::GateId, answer: GateAnswer) {
        self.answered.insert(gate, answer);
    }

    /// Resolves one bare keystroke against the open decision, from its own vocabulary.
    ///
    /// The status bar mirrors the same source, so the bar can never advertise a scope the
    /// drawer does not offer — and `⏎` is **not** bound on an approval, because a queued Return
    /// keystroke must never approve a shell command.
    pub(crate) fn decide_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(action) = fleet_ui_kit::Decision::head(&self.decisions)
            .and_then(|decision| decision.action_for_key(key))
        else {
            return;
        };
        self.act_on_decision(&action, cx);
    }

    // -- rows and scrolling ----------------------------------------------------------------

    /// Expands or collapses whatever the row with this key draws.
    pub(crate) fn toggle_row(&mut self, key: SharedString, cx: &mut Context<Self>) {
        match super::target_of(&self.targets, &key) {
            Some(RowTarget::Item(item)) => {
                if !self.expanded.insert(item) {
                    self.expanded.remove(&item);
                }
                self.expanded_rev = self.expanded_rev.wrapping_add(1);
            }
            Some(RowTarget::Turn(turn)) => {
                if !self.unfolded.insert(turn) {
                    self.unfolded.remove(&turn);
                }
                self.unfolded_rev = self.unfolded_rev.wrapping_add(1);
            }
            Some(RowTarget::Gate(gate)) => {
                if !self.expanded_gates.insert(gate) {
                    self.expanded_gates.remove(&gate);
                }
                self.expanded_rev = self.expanded_rev.wrapping_add(1);
            }
            None => return,
        }
        self.prepare(cx);
        self.install_rows_for_send(cx);
        cx.notify();
    }

    /// `⏎` in row focus: expand or collapse the focused row.
    pub(crate) fn expand_row(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.focused_row_key(cx) else {
            return;
        };
        self.toggle_row(key, cx);
    }

    /// One row-focus verb against the focused row.
    pub(crate) fn row_action(
        &mut self,
        key: SharedString,
        action: RowAction,
        cx: &mut Context<Self>,
    ) {
        let target = super::target_of(&self.targets, &key);
        match action {
            // §5: `[u]` is drawn only where a checkpoint exists, so reaching it without one is
            // a key the user pressed in a row that did not advertise it — a stale listing, or a
            // checkpoint the collector swept. Saying so is the honest answer; silently doing
            // nothing is the one thing a bound key must not do.
            RowAction::Revert => match target {
                Some(RowTarget::Turn(turn)) => match self.checkpoints.get(&turn).cloned() {
                    Some(checkpoint) => {
                        self.dispatch(
                            BridgeCommand::AgentRevert {
                                thread: self.thread,
                                checkpoint,
                            },
                            cx,
                        );
                        // The listing is re-read rather than guessed at: a revert can sweep the
                        // checkpoints newer than the one it restored.
                        cx.emit(AgentThreadEvent::RefreshCheckpoints);
                    }
                    None => self.notice("this turn has no checkpoint to revert to", cx),
                },
                _ => self.notice("only a turn footer can be reverted", cx),
            },
            RowAction::Open => {
                if let Some(path) = target.and_then(|target| self.path_of(target)) {
                    cx.emit(AgentThreadEvent::OpenInEditor(path));
                }
            }
            RowAction::Copy => {
                if let Some(text) = self.payload_of(&key, target) {
                    cx.emit(AgentThreadEvent::Copy(text));
                }
            }
            RowAction::Diff => {
                if let Some(RowTarget::Item(item)) = target {
                    if !self.expanded.insert(item) {
                        self.expanded.remove(&item);
                    }
                    self.expanded_rev = self.expanded_rev.wrapping_add(1);
                    self.prepare(cx);
                    self.install_rows_for_send(cx);
                    cx.notify();
                }
            }
        }
    }

    /// One row-focus verb against the row the transcript's focus ring is on.
    ///
    /// Row focus exists only inside scroll mode, so a verb that finds no focused row is a key
    /// pressed in a context this view is not in — it does nothing rather than guessing a row.
    pub(crate) fn focused_row_verb(&mut self, action: RowAction, cx: &mut Context<Self>) {
        let Some(key) = self.focused_row_key(cx) else {
            return;
        };
        self.row_action(key, action, cx);
    }

    /// The row key the transcript's focus ring is on.
    fn focused_row_key(&self, cx: &Context<Self>) -> Option<SharedString> {
        let list = self.transcript.read(cx);
        let index = list.focused_row()?;
        list.rows().get(index).map(|row| row.id.key())
    }

    /// The path a row's `[o]` opens.
    fn path_of(&self, target: RowTarget) -> Option<String> {
        let RowTarget::Item(item) = target else {
            return None;
        };
        let item = self
            .projection
            .items
            .iter()
            .find(|entry| entry.id == item)?;
        let ItemKind::Tool(call) = &item.kind else {
            return None;
        };
        call.diff
            .as_ref()
            .map(|diff| diff.path.display().to_string())
            .or_else(|| {
                call.input
                    .get("file_path")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
    }

    /// The text a row's `[y]` copies: what the row itself is showing.
    fn payload_of(&self, key: &SharedString, target: Option<RowTarget>) -> Option<String> {
        if let Some(RowTarget::Item(id)) = target
            && let Some(item) = self.projection.items.iter().find(|entry| entry.id == id)
        {
            let text = super::rows::item_text(item);
            if !text.is_empty() {
                return Some(text);
            }
        }
        let row = self
            .transcript_rows()
            .iter()
            .find(|row| row.id.key() == *key)?;
        match &row.kind {
            TranscriptRowKind::Work(work) => Some(
                work.body
                    .as_ref()
                    .map_or_else(|| work.summary.to_string(), ToString::to_string),
            ),
            TranscriptRowKind::Plan(plan) => Some(plan.title.to_string()),
            TranscriptRowKind::Gate(gate) => Some(
                gate.payload
                    .as_ref()
                    .map_or_else(|| gate.detail.to_string(), ToString::to_string),
            ),
            _ => None,
        }
    }

    /// The prepared rows, for the copy lookup above.
    fn transcript_rows(&self) -> &[TranscriptRow] {
        &self.rows
    }

    /// `^s [`: toggle transcript scroll mode, which `esc` also leaves.
    pub(crate) fn toggle_scroll_mode(&mut self, cx: &mut Context<Self>) {
        let scrolling = !self.scrolling;
        self.set_scroll_mode(scrolling, cx);
    }

    /// Enters or leaves transcript scroll mode.
    ///
    /// Entering freezes the tail so new output cannot pull the viewport out from under the
    /// reader, and focuses a row — which is what finally makes `⏎`/`u`/`o`/`y`/`d` fire.
    pub(crate) fn set_scroll_mode(&mut self, scrolling: bool, cx: &mut Context<Self>) {
        self.scrolling = scrolling;
        self.transcript
            .update(cx, |list, cx| list.scroll_mode(scrolling, cx));
        cx.notify();
    }

    /// `j` / `k`: move the row focus, and scroll to it.
    pub(crate) fn move_row_focus(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |list, cx| list.move_row_focus(delta, cx));
    }

    /// `ctrl-d` / `ctrl-u` / `ctrl-f` / `ctrl-b`: move the viewport by a fraction of itself.
    pub(crate) fn scroll_viewports(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |list, cx| list.scroll_viewports(fraction, cx));
    }

    /// `gg`: the oldest retained row.
    pub(crate) fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.transcript.update(cx, TranscriptList::scroll_to_top);
    }

    /// `G`: the newest row, **without** leaving scroll mode.
    ///
    /// `G` is documented as "newest", not as a way out of the mode: the status bar's `SCROLL`
    /// word stays on until `q` / `i` / `esc`, and so must the frozen tail.
    pub(crate) fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.transcript.update(cx, TranscriptList::scroll_to_end);
    }
}

/// The access ladder `^s t` offers, which is a presentation over the harness's own axes.
///
/// Plan mode is deliberately absent: it is the **other** axis, toggled with `⇧⇥`, and offering
/// it here would let one picker silently change two things.
pub(crate) const ACCESS_LADDER: &[(PermissionMode, &str)] = &[
    (PermissionMode::Ask, mode_label(PermissionMode::Ask)),
    (
        PermissionMode::AcceptEdits,
        mode_label(PermissionMode::AcceptEdits),
    ),
    (
        PermissionMode::FullAccess,
        mode_label(PermissionMode::FullAccess),
    ),
];
