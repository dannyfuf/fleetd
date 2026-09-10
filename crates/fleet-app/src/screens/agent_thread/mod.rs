//! The native structured agent thread: transcript, docked composer, and inline decisions.
//!
//! The daemon owns the thread; this screen owns a projection copy, the row model built from it
//! (§5), and the local interaction state a client may not persist — expansion, the queue, an
//! in-progress question answer and the completion surfaces. Every mutation leaves as a typed
//! [`BridgeCommand`] emitted to the workspace, so the view itself issues no I/O and renders
//! purely from state it already holds.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use fleet_core::agents::{
    GateId, GateKind, ItemId, ModelSelection, PermissionMode, Seq, ThreadId, ThreadProjection,
    TurnId, TurnState, UserInput,
};
use fleet_lazygit::diff_view::DiffView;
use fleet_ui_kit::{
    AGENT_CONTENT_W, ActiveTheme, DecisionCard, Icon, IconSize, KeyHint, MultilineInput,
    MultilineInputEvent, Text, Tone, TranscriptEvent, TranscriptList, TranscriptRow,
};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
    div, prelude::*,
};

use crate::bridge::BridgeCommand;

pub(crate) mod decisions;
pub(crate) mod picker;
pub(crate) mod presentation;
pub(crate) mod rows;
#[cfg(test)]
mod tests;

use decisions::{DecisionKey, QuestionSelection};
use picker::{Picker, PickerKind};
use presentation::{composer_placeholder, metadata_left, metadata_right, unreachable_placeholder};
use rows::{RowAnchors, RowInputs};

/// What the thread asks the workspace to do on its behalf.
#[derive(Debug, Clone)]
pub(crate) enum AgentThreadEvent {
    /// Send a typed native-agent command to the daemon.
    Command(BridgeCommand),
    /// Open a path in the user's editor.
    OpenInEditor(String),
    /// Say something short to the user, as a transient toast.
    Notice(SharedString),
}

/// The machine one thread's worktree lives on, as the thread view has to state it (§12).
///
/// A local thread has none of this: `host` is `None` on the model side and the badge, like the
/// header's, is simply absent. P3-T04 puts the badge in the composer's metadata row and stands
/// the composer down while the link is `Down`, rather than letting a send fail on submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadHost {
    /// Configured host id, which is what the badge reads.
    pub(crate) name: SharedString,
    /// Whether the daemon reports the link to that machine as down.
    pub(crate) unreachable: bool,
}

/// Entity owning one thread projection, transcript list, and docked composer.
pub struct AgentThreadView {
    thread: ThreadId,
    projection: ThreadProjection,
    transcript: Entity<TranscriptList>,
    input: Entity<MultilineInput>,
    focus: FocusHandle,
    /// The current row model, kept so a `ContentDelta` can patch exactly one row.
    rows: Vec<TranscriptRow>,
    /// One inline diff surface per item that carries a patch, keyed by the tool row's id.
    ///
    /// Shared with the transcript's body renderer, which only reads it: a render pass may not
    /// update the view that owns the list it is drawing.
    diffs: Rc<RefCell<HashMap<SharedString, Entity<DiffView>>>>,
    /// Where each streaming item's row sits in [`AgentThreadView::rows`].
    row_of_item: HashMap<ItemId, usize>,
    /// Items the user expanded with `⏎`.
    expanded: HashSet<ItemId>,
    /// Turns whose `worked …` fold the user opened.
    unfolded: HashSet<TurnId>,
    /// Composer text waiting for the active turn to settle.
    queued: Vec<String>,
    /// The in-progress answer to the open question gate.
    selection: QuestionSelection,
    /// The gate whose payload is being edited in the composer after `e`.
    editing: Option<GateId>,
    /// The plan gate whose `ask for changes` note is being typed after `n`.
    plan_note: Option<GateId>,
    /// Plan gates whose full Markdown the user opened with `⏎`.
    expanded_plans: HashSet<GateId>,
    /// The open completion surface, if any.
    picker: Option<Picker>,
    /// Provider slash commands learned from `SessionStarted`.
    commands: Vec<String>,
    /// Worktree paths offered by `@`, filled by the workspace.
    files: Vec<String>,
    /// Whether the transcript is in scroll mode (`^s [`).
    scrolling: bool,
    /// The permission mode `⇧⇥` entered plan mode from, so leaving it is a round trip.
    ///
    /// §9 / KEYMAP.md bind `⇧⇥` to "cycle plan / permission mode": a thread on `full access`
    /// that visits plan mode has to come back to `full access`, not be quietly reduced to
    /// `asks before edits` while §2's mode word claims the policy is in force.
    mode_before_plan: Option<PermissionMode>,
    /// The remote machine the thread runs on, when it is not this one (§12, P3-T04).
    host: Option<ThreadHost>,
    /// What each row toggles, so a click can reach the item or turn behind it.
    anchors: RowAnchors,
    _subscriptions: Vec<Subscription>,
}

impl AgentThreadView {
    /// Creates a native agent-thread view from an installed projection.
    #[must_use]
    pub fn new(projection: ThreadProjection, cx: &mut Context<Self>) -> Self {
        let thread = projection.thread;
        let placeholder = composer_placeholder(projection.provider);
        let transcript = cx.new(TranscriptList::new);
        let input = cx.new(|cx| MultilineInput::new(cx, placeholder.into()));
        // B5 owns the composer's keys; the same intents also arrive as actions from the
        // workspace, and both paths funnel into the one send/queue decision below.
        let subscriptions = vec![
            cx.subscribe(&input, Self::on_input_event),
            cx.subscribe(&transcript, Self::on_transcript_event),
        ];
        let diffs: Rc<RefCell<HashMap<SharedString, Entity<DiffView>>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let rendered = Rc::clone(&diffs);
        transcript.update(cx, |list, cx| {
            list.set_tool_body(
                move |row, _cx| {
                    rendered
                        .borrow()
                        .get(&row.id)
                        .map(|view| view.clone().into_any_element())
                },
                cx,
            );
        });
        let mut view = Self {
            thread,
            projection,
            transcript,
            input,
            focus: cx.focus_handle(),
            rows: Vec::new(),
            diffs,
            row_of_item: HashMap::new(),
            expanded: HashSet::new(),
            unfolded: HashSet::new(),
            queued: Vec::new(),
            selection: QuestionSelection::default(),
            editing: None,
            plan_note: None,
            expanded_plans: HashSet::new(),
            picker: None,
            commands: Vec::new(),
            files: Vec::new(),
            scrolling: false,
            mode_before_plan: None,
            host: None,
            anchors: RowAnchors::default(),
            _subscriptions: subscriptions,
        };
        view.rebuild(cx);
        view
    }

    /// Thread rendered by this view.
    #[must_use]
    pub fn thread(&self) -> ThreadId {
        self.thread
    }

    /// Latest installed daemon projection.
    #[must_use]
    pub fn projection(&self) -> &ThreadProjection {
        &self.projection
    }

    /// Replaces the daemon projection.
    pub fn set_projection(&mut self, projection: ThreadProjection, cx: &mut Context<Self>) {
        self.thread = projection.thread;
        self.projection = projection;
        self.rebuild(cx);
        cx.notify();
    }

    /// Transcript-list entity owned by this screen.
    #[must_use]
    pub fn transcript(&self) -> &Entity<TranscriptList> {
        &self.transcript
    }

    /// Composer entity owned by this screen.
    #[must_use]
    pub fn input(&self) -> &Entity<MultilineInput> {
        &self.input
    }

    /// Names the machine this thread runs on, or clears the badge for a local one (§12).
    ///
    /// The composer's placeholder is part of the state: while the link is down it says so, so a
    /// composer that refuses `⏎` never looks like one that simply lost the key.
    pub(crate) fn set_host(&mut self, host: Option<ThreadHost>, cx: &mut Context<Self>) {
        if self.host == host {
            return;
        }
        let unreachable = host.as_ref().is_some_and(|host| host.unreachable);
        self.host = host;
        let placeholder = match self.host.as_ref().filter(|_| unreachable) {
            Some(host) => unreachable_placeholder(&host.name),
            None => composer_placeholder(self.projection.provider),
        };
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
            input.set_read_only(unreachable, cx);
        });
        self.sync_composer_ring(cx);
        cx.notify();
    }

    /// The machine this thread runs on, when it is a remote one.
    #[must_use]
    pub(crate) const fn host(&self) -> Option<&ThreadHost> {
        self.host.as_ref()
    }

    /// Whether the thread's machine is out of reach, which stands the composer down (P3-T04).
    #[must_use]
    pub(crate) fn is_unreachable(&self) -> bool {
        self.host.as_ref().is_some_and(|host| host.unreachable)
    }

    /// Offers the worktree paths `@` completes, replacing any earlier listing.
    pub(crate) fn set_files(&mut self, files: Vec<String>) {
        self.files = files;
    }

    /// The paths `@` currently completes, so a test can tell a served listing from none.
    #[cfg(test)]
    pub(crate) fn files(&self) -> &[String] {
        &self.files
    }

    /// The sequence the user has now seen, which is what clears `finished` and `unread`.
    pub(crate) fn last_seq(&self) -> Seq {
        self.projection.last_seq
    }

    /// The gate that owns the keyboard, which is always the newest open one.
    pub(crate) fn open_gate(&self) -> Option<&fleet_core::agents::OpenGate> {
        self.projection.gates.last()
    }

    /// Whether the provider, the turn, or a background task is alive (§3.3).
    pub(crate) fn is_working(&self) -> bool {
        matches!(self.projection.turn, TurnState::Running(_))
            || self.projection.session == fleet_core::agents::SessionState::Running
            || !self.projection.background_tasks.is_empty()
    }

    /// Adopts the mirror's projection, patching only the rows a stream moved (§5).
    ///
    /// Structural change — a new item, a settled turn, an opened gate — rebuilds the grouping;
    /// pure text growth on already-known streaming items moves those rows and nothing else, so
    /// a fast model does not schedule a full row rebuild per token.
    pub(crate) fn sync(&mut self, projection: &ThreadProjection, cx: &mut Context<Self>) {
        if projection.last_seq == self.projection.last_seq {
            return;
        }
        let streaming_only = self.streaming_only_change(projection);
        self.projection = projection.clone();
        if streaming_only {
            let items: Vec<ItemId> = self.row_of_item.keys().copied().collect();
            let mut moved = false;
            // Every streaming row is patched; `any` would stop at the first one that moved.
            for item in items {
                moved |= self.patch_streaming_row(item);
            }
            if moved {
                let rows = self.rows.clone();
                self.transcript
                    .update(cx, |list, cx| list.set_rows(rows, cx));
            }
            cx.notify();
            return;
        }
        self.rebuild(cx);
        // §3.3 rule 6 sends a queued message "when the turn settles", and `send` queues on the
        // wider `is_working()` — a live session or a background task counts. Flushing on the
        // narrower "no running turn" dispatched a message queued behind a subagent on the very
        // next structural event, which is the opposite of what `queued · [esc] unqueue` says.
        if !self.is_working() {
            self.flush_queue(cx);
        }
        cx.notify();
    }

    /// Whether the only difference is accumulated text on items that already have rows.
    fn streaming_only_change(&self, next: &ThreadProjection) -> bool {
        streaming_only_change(&self.projection, next, &self.row_of_item)
    }

    /// Replaces the provider slash commands `/` completes.
    pub(crate) fn set_commands(&mut self, commands: Vec<String>) {
        self.commands = commands;
    }

    /// Rewrites the text of the row that carries `item`, returning whether it moved.
    fn patch_streaming_row(&mut self, item: ItemId) -> bool {
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
        let text = source.text.clone().unwrap_or_default();
        let Some(row) = self.rows.get_mut(index) else {
            return false;
        };
        match row {
            TranscriptRow::AssistantText { markdown } => {
                *markdown = fleet_ui_kit::parse_markdown_document(&text);
            }
            TranscriptRow::Thinking { text: current, .. } => {
                *current = gpui::SharedString::from(text);
            }
            _ => return false,
        }
        true
    }

    /// Rebuilds the whole row model, which every structural event requires (§5).
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.sync_diffs(cx);
        let queued = std::mem::take(&mut self.queued);
        let cards = self.cards();
        let built = rows::build_rows(&RowInputs {
            projection: &self.projection,
            expanded: &self.expanded,
            unfolded: &self.unfolded,
            queued: &queued,
            cards: &cards,
        });
        self.queued = queued;
        self.row_of_item = index_streaming_rows(&self.projection, &built.rows);
        self.anchors = built.anchors;
        self.rows = built.rows.clone();
        let streaming = self.is_working();
        self.transcript.update(cx, |list, cx| {
            list.set_rows(built.rows, cx);
            // §2: only gray spinners and the text cursor animate, and the cursor belongs to the
            // paragraph the model is still writing.
            list.set_streaming(streaming, cx);
        });
        self.sync_composer_ring(cx);
    }

    /// Builds, updates and drops the inline diff surface of every item that carries a patch.
    ///
    /// The patch text is what identifies a change, so an item whose diff grew replaces its own
    /// text in place and keeps the prepared model the background pass already built.
    fn sync_diffs(&mut self, cx: &mut Context<Self>) {
        let patches: Vec<(SharedString, Option<String>, String)> = self
            .projection
            .items
            .iter()
            .filter_map(|item| {
                let diff = item.diff.as_ref()?;
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

    /// The cards for the open gates, in opening order; the newest one owns the keys.
    fn cards(&self) -> Vec<DecisionCard> {
        self.projection
            .gates
            .iter()
            .map(|gate| {
                decisions::card(
                    gate,
                    self.projection.provider,
                    &self.selection,
                    self.expanded_plans.contains(&gate.id),
                )
            })
            .collect()
    }

    // -- composer -------------------------------------------------------------------------

    /// `⏎`: accept a completion, else send, else queue behind the active turn.
    pub(crate) fn send(&mut self, cx: &mut Context<Self>) {
        if self.accept_completion(cx) {
            return;
        }
        let text = self.input.read(cx).text().to_owned();
        self.send_text(text, cx);
    }

    /// The same intent, over text the composer reported rather than text it still holds.
    ///
    /// DESIGN-SYSTEM §6.6: "a submit … is reported, and the owner decides what they mean" —
    /// [`MultilineInput::submit`] has already emptied the buffer and pushed the entry to
    /// history by the time the event arrives, so re-reading the composer would send nothing.
    /// `enter` is unbound in `Agent > AgentDecision > AgentPermission` and
    /// `Agent > AgentNativeScroll`, where the key reaches the composer and this is the only
    /// path the draft survives.
    pub(crate) fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        // P3-T04: the daemon cannot carry this to a machine it has no link to. The draft is put
        // back — `MultilineInput::submit` has already emptied the buffer by the time a composer
        // submit arrives here — so the message survives until the link does come back.
        if let Some(host) = self.host.as_ref().filter(|host| host.unreachable) {
            let notice = presentation::unreachable_notice(&host.name);
            self.input.update(cx, |input, cx| input.set_text(text, cx));
            cx.emit(AgentThreadEvent::Notice(SharedString::from(notice)));
            cx.notify();
            return;
        }
        if let Some(gate) = self.editing.take() {
            self.answer_edited(gate, text, cx);
            self.sync_composer_ring(cx);
            return;
        }
        if let Some(gate) = self.plan_note.take() {
            self.input.update(cx, MultilineInput::clear);
            self.sync_composer_ring(cx);
            self.dispatch(
                BridgeCommand::AgentRespond {
                    thread: self.thread,
                    gate,
                    answer: fleet_core::agents::GateAnswer::Plan(
                        fleet_core::agents::PlanAnswer::AskForChanges { note: text },
                    ),
                },
                cx,
            );
            return;
        }
        // §3.2 answers "Something else…" with free text, which the composer carries: while it
        // is chosen the card has stood its keys down, so `⏎` arrives here and not at `decide`.
        if self
            .open_gate()
            .is_some_and(|gate| self.selection.wants_free_text(gate))
        {
            self.decide(DecisionKey::Answer, cx);
            return;
        }
        self.input.update(cx, |input, cx| {
            input.push_history(text.clone());
            input.clear(cx);
        });
        // §3.3's `Working` — which is what the `AgentWorking` context, the tab and the
        // `⏎ queue` hint are all derived from — is broader than a running turn: a live session
        // or a background task counts too. The queue decision uses the same predicate, so the
        // status bar can never say `queue` while the message is dispatched instead.
        if self.is_working() {
            self.queued.push(text);
            self.rebuild(cx);
            cx.notify();
            return;
        }
        self.dispatch(
            BridgeCommand::AgentSend {
                thread: self.thread,
                input: UserInput {
                    text,
                    attachments: Vec::new(),
                },
            },
            cx,
        );
    }

    /// `⏎` while working: the same decision, stated by its own action.
    pub(crate) fn queue(&mut self, cx: &mut Context<Self>) {
        self.send(cx);
    }

    /// Sends everything queued once the turn settles (§3.3 rule 6).
    fn flush_queue(&mut self, cx: &mut Context<Self>) {
        if self.queued.is_empty() {
            return;
        }
        for text in std::mem::take(&mut self.queued) {
            self.dispatch(
                BridgeCommand::AgentSend {
                    thread: self.thread,
                    input: UserInput {
                        text,
                        attachments: Vec::new(),
                    },
                },
                cx,
            );
        }
        self.rebuild(cx);
    }

    /// `esc`: leave scroll mode, else unqueue the newest queued message, else interrupt.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            cx.notify();
            return;
        }
        // A note or a correction is abandoned before anything else `esc` means: the card the
        // user was answering is still open behind it.
        if self.plan_note.take().is_some() || self.editing.take().is_some() {
            self.input.update(cx, MultilineInput::clear);
            self.sync_composer_ring(cx);
            cx.notify();
            return;
        }
        // KEYMAP.md: a mode that freezes the tail is left again, never a one-way door.
        if self.scrolling {
            self.set_scroll_mode(false, cx);
            return;
        }
        if self.queued.pop().is_some() {
            self.rebuild(cx);
            cx.notify();
            return;
        }
        // §9 binds `esc` to `stop` only where there is work to stop. In `AgentIdle` the key is
        // unbound and reaches the composer instead, which reports it as
        // `MultilineInputEvent::Escape` and lands here — with nothing running, so an interrupt
        // would only earn a `conflict: … has no active turn` the user cannot act on. `esc` on an
        // idle thread means "never mind", and that is already what dismissing the draft did.
        if !self.is_working() {
            return;
        }
        self.dispatch(
            BridgeCommand::AgentInterrupt {
                thread: self.thread,
            },
            cx,
        );
    }

    /// `⇧⇥`: toggle plan mode, returning to the mode it was entered from.
    pub(crate) fn toggle_plan_mode(&mut self, cx: &mut Context<Self>) {
        let mode = if self.projection.mode == PermissionMode::Plan {
            // A thread that was already in plan mode when the tab opened has no recorded
            // origin; `ask` is the only mode §2 lets the word claim without knowing more.
            self.mode_before_plan.take().unwrap_or(PermissionMode::Ask)
        } else {
            self.mode_before_plan = Some(self.projection.mode);
            PermissionMode::Plan
        };
        self.dispatch(
            BridgeCommand::AgentSetMode {
                thread: self.thread,
                mode,
            },
            cx,
        );
    }

    /// `↑` / `ctrl-p`: move backwards through the open completion surface.
    ///
    /// DESIGN-SYSTEM §4 gives a list under a text field `↓`/`↑` and `ctrl-n`/`ctrl-p`, in that
    /// sense: `↑` moves the highlight *up*. Advancing on `↑` inverted the one direction that
    /// was bound.
    ///
    /// Prompt recall itself belongs to the composer — this screen only feeds it with
    /// [`MultilineInput::push_history`], because the kit contract exposes no recall call.
    pub(crate) fn history(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut() {
            picker.retreat();
            cx.notify();
            return;
        }
        // The action binding consumes the key before the composer sees it (on macOS a binding
        // always wins over the input handler), so `↑` is handed back to the composer's own
        // resolution of it: the previous prompt only when the caret is parked at the top of an
        // untouched buffer, otherwise one row up. Recalling unconditionally replaced a `⇧⏎`
        // multi-line draft the moment the caret was moved up a line.
        self.input.update(cx, |input, cx| input.caret_up(cx));
        cx.notify();
    }

    /// `↓` / `ctrl-n`: move forwards through the open completion surface.
    ///
    /// With no picker open the key is handed back to the composer, exactly as [`Self::history`]
    /// hands `↑` back: a `⇧⏎` draft still moves its caret down a row.
    pub(crate) fn history_next(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut() {
            picker.advance();
            cx.notify();
            return;
        }
        self.input.update(cx, |input, cx| input.caret_down(cx));
        cx.notify();
    }

    /// `@`, `/` and `^s m`: open a completion surface, or advance the open one.
    pub(crate) fn open_picker(&mut self, kind: PickerKind, cx: &mut Context<Self>) {
        match self.picker.as_mut() {
            Some(picker) if picker.kind == kind => picker.advance(),
            _ => {
                let candidates = match kind {
                    PickerKind::Files => self.files.clone(),
                    PickerKind::Commands => self.commands.clone(),
                    PickerKind::Models => self.model_candidates(),
                };
                // DESIGN-SYSTEM §4: an invalid command is not listed. A picker with nothing to
                // offer draws nothing, and would still swallow the next `⏎` in
                // `accept_completion` — the composer's message would silently not be sent.
                if candidates.is_empty() {
                    self.picker = None;
                    cx.notify();
                    return;
                }
                self.picker = Some(Picker::new(kind, candidates));
            }
        }
        let query = self.completion_query(cx);
        if let Some(picker) = self.picker.as_mut() {
            picker.filter(&query);
        }
        cx.notify();
    }

    /// The text typed after the open picker's trigger.
    fn completion_query(&self, cx: &Context<Self>) -> String {
        let Some(prefix) = self.picker.as_ref().and_then(|picker| picker.kind.prefix()) else {
            return String::new();
        };
        let text = self.input.read(cx).text();
        text.rsplit_once(prefix)
            .map(|(_, tail)| tail.to_owned())
            .unwrap_or_default()
    }

    /// The models `^s m` offers: the active one across the provider's effort ladder.
    ///
    /// The effort the session is already on is not offered — DESIGN-SYSTEM §4 does not list a
    /// command that does nothing — so every row in this picker is a change. A wider list would
    /// need the provider's own catalogue, which no event on the wire carries today.
    fn model_candidates(&self) -> Vec<String> {
        let Some(model) = &self.projection.model else {
            return Vec::new();
        };
        ["low", "medium", "high"]
            .into_iter()
            .filter(|effort| model.effort.as_deref() != Some(*effort))
            .map(|effort| format!("{} \u{b7} {effort}", model.model))
            .collect()
    }

    /// `⏎` while a picker is open: accept its highlighted row.
    fn accept_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(picker) = self.picker.take() else {
            return false;
        };
        let Some(accepted) = picker.accepted() else {
            cx.notify();
            return true;
        };
        match picker.kind {
            PickerKind::Models => {
                let model = self.projection.model.clone().map(|model| ModelSelection {
                    effort: accepted.rsplit(' ').next().map(str::to_owned),
                    ..model
                });
                if let Some(model) = model {
                    self.dispatch(
                        BridgeCommand::AgentSetModel {
                            thread: self.thread,
                            model,
                        },
                        cx,
                    );
                }
            }
            PickerKind::Files | PickerKind::Commands => {
                // The picker filters on what follows the trigger, so the accepted row replaces
                // that query rather than being appended behind it: `@src/li` + `src/lib.rs` is
                // `@src/lib.rs `, never `@src/lisrc/lib.rs `.
                let text = self.input.read(cx).text();
                let kept = picker
                    .kind
                    .prefix()
                    .and_then(|prefix| text.rfind(prefix).map(|at| at + prefix.len_utf8()))
                    .unwrap_or(text.len());
                let text = format!("{}{accepted} ", &text[..kept]);
                self.input.update(cx, |input, cx| input.set_text(text, cx));
            }
        }
        cx.notify();
        true
    }

    // -- decisions ------------------------------------------------------------------------

    /// Routes one bare key to the open gate (§9).
    pub(crate) fn decide(&mut self, key: DecisionKey, cx: &mut Context<Self>) {
        let Some(gate) = self.open_gate().cloned() else {
            return;
        };
        // `⏎ view full plan` shows the whole proposal; it answers nothing and never leaves this
        // client, so it is handled here rather than through a `GateAnswer`.
        if matches!(key, DecisionKey::ViewPlan) {
            if !self.expanded_plans.insert(gate.id) {
                self.expanded_plans.remove(&gate.id);
            }
            self.rebuild(cx);
            cx.notify();
            return;
        }
        if let GateKind::Question { questions } = &gate.kind
            && self.selection == QuestionSelection::default()
        {
            self.selection = QuestionSelection::new(questions.len());
        }
        // KEYMAP.md: while a plan note is being typed the card's keys stand down. `n` is what
        // opens the note, so the note itself may start with a `y` or an `n`.
        if matches!(key, DecisionKey::AskChanges)
            && matches!(gate.kind, GateKind::Plan { .. })
            && self.plan_note.is_none()
        {
            self.plan_note = Some(gate.id);
            self.sync_composer_ring(cx);
            cx.notify();
            return;
        }
        // The composer carries the plan note `n` sends, and the free text a question's
        // "Something else…" option answers with.
        let typed = matches!(key, DecisionKey::AskChanges | DecisionKey::Answer)
            .then(|| self.input.read(cx).text().trim().to_owned())
            .filter(|note| !note.is_empty());
        let answer = decisions::answer_for(&gate, key, &mut self.selection, typed);
        let Some(answer) = answer else {
            self.rebuild(cx);
            cx.notify();
            return;
        };
        self.selection = QuestionSelection::default();
        self.plan_note = None;
        self.input.update(cx, MultilineInput::clear);
        self.sync_composer_ring(cx);
        self.dispatch(
            BridgeCommand::AgentRespond {
                thread: self.thread,
                gate: gate.id,
                answer,
            },
            cx,
        );
    }

    /// Whether the composer, not the open card, owns the bare letters right now.
    ///
    /// §9 routes `y`/`a`/`n`/`e` to a card, which makes them unavailable to the composer while
    /// one is open. Correcting a payload after `e`, and writing the note `n` sends with, are
    /// both typing: the card gives the keyboard back for as long as they last.
    pub(crate) fn is_composing(&self, _cx: &App) -> bool {
        // The first two states are entered by an explicit gesture — `e` on a permission, `n` on
        // a plan — rather than inferred from the composer already holding text: the *first*
        // character of a note is exactly the one that would otherwise be swallowed as a card
        // key. Choosing "Something else…" is that same gesture for a question: §3.2 answers it
        // with free text, which may contain a space, and `space` is one of the card's keys.
        self.editing.is_some()
            || self.plan_note.is_some()
            || self
                .open_gate()
                .is_some_and(|gate| self.selection.wants_free_text(gate))
    }

    /// `e`: move the protected payload into the composer so it can be corrected before `y`.
    pub(crate) fn edit_command(&mut self, cx: &mut Context<Self>) {
        let Some(gate) = self.open_gate() else {
            return;
        };
        let GateKind::Permission {
            payload, options, ..
        } = &gate.kind
        else {
            return;
        };
        // OpenCode has no edit answer: `once`, `always` and `reject` are all it accepts, so
        // seeding the composer would promise a correction the wire cannot carry (§4.2).
        if !options
            .iter()
            .any(|option| option.label == fleet_core::agents::PermissionChoice::Edit)
        {
            cx.emit(AgentThreadEvent::Notice(SharedString::new_static(
                "this agent cannot edit a command before allowing it",
            )));
            return;
        }
        let (id, payload) = (gate.id, payload.clone());
        self.editing = Some(id);
        self.input
            .update(cx, |input, cx| input.set_text(payload, cx));
        self.sync_composer_ring(cx);
        cx.notify();
    }

    /// `⏎` after `e`: allow the corrected invocation, and only the corrected one.
    ///
    /// The adapters honour an edited payload under [`PermissionChoice::Edit`]; an `AllowOnce`
    /// with a payload beside it would run the original command while the user believes their
    /// correction went out (§4.1). A provider that offers no `Edit` option never gets here,
    /// because `e` is refused for it in [`AgentThreadView::edit_command`].
    fn answer_edited(&mut self, gate: GateId, payload: String, cx: &mut Context<Self>) {
        self.input.update(cx, MultilineInput::clear);
        self.dispatch(
            BridgeCommand::AgentRespond {
                thread: self.thread,
                gate,
                answer: fleet_core::agents::GateAnswer::Permission {
                    choice: fleet_core::agents::PermissionChoice::Edit,
                    edited_payload: Some(payload),
                },
            },
            cx,
        );
    }

    // -- rows -----------------------------------------------------------------------------

    /// `⏎` on a focused row: expand or collapse it, and open the newest fold otherwise.
    pub(crate) fn expand_row(&mut self, cx: &mut Context<Self>) {
        let expandable = self
            .projection
            .items
            .iter()
            .rev()
            .find(|item| item.output.is_some() || item.diff.is_some())
            .map(|item| item.id);
        if let Some(item) = expandable {
            if !self.expanded.insert(item) {
                self.expanded.remove(&item);
            }
        } else if let Some(turn) = self.projection.turns.last().map(|turn| turn.id)
            && !self.unfolded.insert(turn)
        {
            self.unfolded.remove(&turn);
        }
        self.rebuild(cx);
        cx.notify();
    }

    /// `u`: revert the focused edit or the whole turn.
    ///
    /// Reverting restores files from a Fleet-owned turn checkpoint (§7). Neither the checkpoint
    /// service nor the `AgentRevert` request exists yet, and the two plausible stand-ins are
    /// both worse than saying so: interrupting the turn is a different, destructive action
    /// under the same key, and doing nothing silently teaches the user the key is broken.
    pub(crate) fn revert(&mut self, cx: &mut Context<Self>) {
        cx.emit(AgentThreadEvent::Notice(SharedString::new_static(
            "revert needs turn checkpoints, which are not built yet",
        )));
    }

    /// `o`: open the newest touched path in the user's editor.
    pub(crate) fn open_in_editor(&mut self, cx: &mut Context<Self>) {
        let path = self
            .projection
            .items
            .iter()
            .rev()
            .find_map(|item| item.diff.as_ref())
            .map(|diff| diff.path.display().to_string());
        if let Some(path) = path {
            cx.emit(AgentThreadEvent::OpenInEditor(path));
        }
    }

    /// `^s [`: toggle transcript scroll mode, which `esc` also leaves.
    pub(crate) fn toggle_scroll_mode(&mut self, cx: &mut Context<Self>) {
        let scrolling = !self.scrolling;
        self.set_scroll_mode(scrolling, cx);
    }

    /// Enters or leaves transcript scroll mode.
    pub(crate) fn set_scroll_mode(&mut self, scrolling: bool, cx: &mut Context<Self>) {
        self.scrolling = scrolling;
        self.transcript
            .update(cx, |list, cx| list.scroll_mode(scrolling, cx));
        cx.notify();
    }

    /// Whether the transcript's tail is frozen, which is what `Agent > AgentNativeScroll` and
    /// the status bar's `SCROLL` word are both derived from.
    #[must_use]
    pub(crate) fn is_scrolling(&self) -> bool {
        self.scrolling
    }

    /// The question an open card's bare keys address, which the status bar mirrors (§2).
    #[must_use]
    pub(crate) const fn question_cursor(&self) -> usize {
        self.selection.cursor()
    }

    /// `j` / `k`: move the frozen viewport by rows.
    pub(crate) fn scroll_rows(&mut self, rows: f32, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |list, cx| list.scroll_rows(rows, cx));
    }

    /// `ctrl-d` / `ctrl-u` / `ctrl-f` / `ctrl-b`: move it by a fraction of the viewport.
    pub(crate) fn scroll_viewports(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |list, cx| list.scroll_viewports(fraction, cx));
    }

    /// `gg`: the oldest retained row.
    pub(crate) fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.transcript.update(cx, TranscriptList::scroll_to_top);
    }

    /// `G`: back to the newest row, still in scroll mode.
    ///
    /// The jump is not the way out of the mode: KEYMAP.md keeps `SCROLL` on the status bar
    /// until `q`/`i`/`esc` leaves it, and that is what re-arms the tail.
    pub(crate) fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.transcript.update(cx, TranscriptList::scroll_to_end);
    }

    /// Keeps the composer's focus ring in step with who owns the bare keys (§2).
    ///
    /// docs/APP-CONTRACTS.md:101 — render prepares nothing. The ring is a function of the open
    /// gate, of whether the card has stood its keys down, and of whether the machine is in
    /// reach, so it is written here, at every transition that moves one of the three, rather
    /// than from the render body, where the composer's own `cx.notify()` scheduled a second
    /// frame for a state the first one had already drawn.
    fn sync_composer_ring(&self, cx: &mut Context<Self>) {
        let visible =
            (self.open_gate().is_none() || self.is_composing(cx)) && !self.is_unreachable();
        self.input
            .update(cx, |input, cx| input.set_focus_visible(visible, cx));
    }

    /// Takes the keyboard for the composer, which is what an agent tab focuses.
    ///
    /// A stood-down composer must not take the handle, but something on this view has to: the
    /// shell hands `FocusTarget::AgentThread` here and never touches focus again, so declining
    /// outright left an agent tab whose overlay had just closed with no focus owner at all and
    /// every `Agent > …` binding unresolved. The view's own tracked handle is the right
    /// fallback rather than the shell body: gpui dispatches through ancestors, so focusing it
    /// keeps both the thread's listeners and the workspace root's on the path.
    pub(crate) fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_unreachable() {
            if !self.focus.is_focused(window) {
                self.focus.focus(window, cx);
            }
            return;
        }
        let handle = self.input.read(cx).focus_handle().clone();
        if !handle.is_focused(window) {
            handle.focus(window, cx);
        }
    }

    /// Emits one typed command for the workspace to send.
    fn dispatch(&mut self, command: BridgeCommand, cx: &mut Context<Self>) {
        cx.emit(AgentThreadEvent::Command(command));
        cx.notify();
    }

    /// Bridges a click on a transcript row into the same toggle `⏎` would do.
    ///
    /// Row focus does not exist yet (§10), so without this every `[⏎] show` in the transcript
    /// is an affordance nothing can fire.
    fn on_transcript_event(
        &mut self,
        _transcript: Entity<TranscriptList>,
        event: &TranscriptEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TranscriptEvent::Toggle(id) => self.toggle_row(id, cx),
            // DESIGN-SYSTEM §6.6: the surface holding the focus handle owns the key event and
            // routes it here. Row focus does not exist yet, so clicking a card's action hint is
            // the only way to reach it with a mouse — and every action the card draws is
            // clickable, so discarding this made all of them inert.
            // `e` seeds the composer instead of answering, so it is not one of `decide`'s
            // keys — but it is drawn, and a drawn affordance that does nothing is worse than
            // an absent one (DESIGN-SYSTEM §7).
            TranscriptEvent::Decision {
                action: fleet_ui_kit::DecisionAction::Edit,
                ..
            } => self.edit_command(cx),
            TranscriptEvent::Decision { action, .. } => {
                if let Some(key) = decision_key_for(action) {
                    self.decide(key, cx);
                }
            }
        }
    }

    /// Expands or collapses whatever the row with this key draws.
    fn toggle_row(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if let Some(item) = self
            .projection
            .items
            .iter()
            .find(|item| item.id.to_string() == id.as_ref())
            .map(|item| item.id)
        {
            if !self.expanded.insert(item) {
                self.expanded.remove(&item);
            }
        } else if let Some(index) = id
            .strip_prefix("row-")
            .and_then(|index| index.parse::<usize>().ok())
        {
            if let Some(item) = self.anchors.items.get(&index).copied() {
                if !self.expanded.insert(item) {
                    self.expanded.remove(&item);
                }
            } else if let Some(turn) = self.anchors.folds.get(&index).copied()
                && !self.unfolded.insert(turn)
            {
                self.unfolded.remove(&turn);
            }
        }
        self.rebuild(cx);
        cx.notify();
    }

    /// Bridges the composer's own key handling into the same intents the actions use.
    fn on_input_event(
        &mut self,
        _input: Entity<MultilineInput>,
        event: &MultilineInputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            MultilineInputEvent::Submit(text) => self.send_text(text.clone(), cx),
            MultilineInputEvent::Trigger('@') => self.open_picker(PickerKind::Files, cx),
            MultilineInputEvent::Trigger('/') => self.open_picker(PickerKind::Commands, cx),
            MultilineInputEvent::Trigger(_) => {}
            MultilineInputEvent::Escape => self.stop(cx),
        }
    }
}

/// The bare key a clicked card action stands for.
fn decision_key_for(action: &fleet_ui_kit::DecisionAction) -> Option<DecisionKey> {
    use fleet_ui_kit::DecisionAction as Action;

    Some(match action {
        Action::AllowOnce => DecisionKey::AllowOnce,
        Action::AllowSession | Action::AllowDirectory => DecisionKey::AllowSession,
        Action::Deny => DecisionKey::Deny,
        Action::DenyAndStop => DecisionKey::DenyAndStop,
        Action::Answer => DecisionKey::Answer,
        Action::Choose(option) => DecisionKey::Choose(*option),
        Action::Toggle => DecisionKey::Toggle,
        Action::ApprovePlan => DecisionKey::ApprovePlan,
        Action::AskForChanges => DecisionKey::AskChanges,
        Action::ViewPlan => DecisionKey::ViewPlan,
        // `e` seeds the composer instead of answering, so it is not one of `decide`'s keys.
        Action::Edit => return None,
    })
}

/// Whether one projection differs from the next only by text on items that already have rows.
///
/// A free function so the rule can be asserted without a window: it decides whether `sync`
/// takes the cheap streaming path, and a part of the projection missing from the comparison is
/// a row that silently never appears.
fn streaming_only_change(
    current: &ThreadProjection,
    next: &ThreadProjection,
    row_of_item: &HashMap<ItemId, usize>,
) -> bool {
    let structure_held = next.items.len() == current.items.len()
        && next.turns.len() == current.turns.len()
        && next.gates.len() == current.gates.len()
        // §5 gives notices and checkpoints transcript rows of their own, and a `GateResolved`
        // immediately followed by a `GateOpened` keeps the count at one while replacing the
        // card the transcript draws: every row-bearing part of the projection has to be
        // compared, not just the three that happen to be lists of items.
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
        && next.exit_code == current.exit_code;
    if !structure_held {
        return false;
    }
    next.items
        .iter()
        .zip(&current.items)
        .all(|(next, current)| {
            let same_row = next.id == current.id
                && next.status == current.status
                && next.summary == current.summary
                && next.result == current.result
                && next.diff == current.diff;
            // Text may only have grown on an item this view already has a row for; a text change
            // anywhere else is structural as far as the row model is concerned.
            same_row && (next.text == current.text || row_of_item.contains_key(&next.id))
        })
}

/// The rows a `ContentDelta` may patch in place, by the item that streams into them.
fn index_streaming_rows(
    projection: &ThreadProjection,
    rows: &[TranscriptRow],
) -> HashMap<ItemId, usize> {
    // Streaming rows appear in projection order, so walking both lists once pairs them without
    // giving `TranscriptRow` an identity field it does not need. Only *top-level* items are
    // paired: `build_rows` draws a nested item inside its `ToolRow { children }` and emits no
    // row of its own for it, so counting one — a `Task` subagent's text or thinking — would
    // consume a slot and shift every later row's item by one, and §5's "a `ContentDelta`
    // mutates only the text of the row for that `item`" would rewrite the wrong paragraph.
    let mut streaming = projection
        .items
        .iter()
        .filter(|item| {
            item.parent.is_none()
                && matches!(
                    item.kind,
                    fleet_core::agents::ItemKind::AssistantText
                        | fleet_core::agents::ItemKind::Thinking
                )
        })
        .map(|item| item.id);
    let mut index = HashMap::new();
    for (position, row) in rows.iter().enumerate() {
        if matches!(
            row,
            TranscriptRow::AssistantText { .. } | TranscriptRow::Thinking { .. }
        ) && let Some(item) = streaming.next()
        {
            index.insert(item, position);
        }
    }
    index
}

impl EventEmitter<AgentThreadEvent> for AgentThreadView {}

impl Focusable for AgentThreadView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AgentThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // §2: while a card is open the composer stays visible at 60 % and the keys route away.
        let deciding = self.open_gate().is_some();
        // §2's 60 % is the state where "bare keys route to the card". While a correction, a
        // plan note or a free-text answer is being typed the card has stood its keys down and
        // the composer is where you are, so it is drawn at full strength — the same predicate
        // that restores its focus ring below.
        let composing = self.is_composing(cx);
        // P3-T04: a composer that cannot reach its machine is dimmed like one that has stood
        // its keys down, because that is exactly what it has done.
        let unreachable = self.is_unreachable();
        let composer_opacity = if unreachable || (deciding && !composing) {
            1.0 - theme.metrics.dimmed_opacity
        } else {
            1.0
        };

        // The badge sits with the metadata rather than over the transcript: §2 keeps the
        // transcript for the thread's own content, and the row already carries what the thread
        // is running as. The right half states the refusal while the link is down.
        let metadata = div()
            .flex()
            .items_center()
            .justify_between()
            .h(theme.metrics.strip_h)
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .children(self.host_badge(&theme))
                    .child(Text::hint(metadata_left(&self.projection)).muted()),
            )
            .child(match self.host.as_ref().filter(|_| unreachable) {
                Some(host) => {
                    Text::hint(presentation::unreachable_hint(&host.name)).tone(Tone::Warning)
                }
                None => Text::hint(metadata_right(&self.projection)).muted(),
            });

        // §2: blue is where you are. While the card owns the bare keys the composer is not
        // where you are, so it drops its focus ring even though it still holds the handle —
        // written by `sync_composer_ring` on every transition, because render prepares nothing.
        let composer = div()
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .opacity(composer_opacity)
            .child(
                // The composer grows one line at a time to eight (§2), so the row it sits in is
                // a floor rather than a ceiling; the `❯` belongs to the composer itself.
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .min_h(theme.metrics.text_field_h)
                    .w_full()
                    .child(div().flex_1().min_w_0().child(self.input.clone())),
            )
            .child(metadata);

        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(theme.colors.bg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .max_w(AGENT_CONTENT_W)
                            .px(theme.space.lg)
                            .child(div().flex_1().min_h_0().child(self.transcript.clone()))
                            .children(self.picker_element(&theme))
                            .child(composer),
                    ),
            )
    }
}

impl AgentThreadView {
    /// The `⛅ dev-box` badge of a remote thread, in the composer's metadata row (P3-T04).
    ///
    /// It uses the workspace header's own pair of icons and tones, so one thread never reads as
    /// reachable in the header and unreachable here.
    fn host_badge(&self, theme: &fleet_ui_kit::Theme) -> Option<gpui::Div> {
        let host = self.host()?;
        let (icon, tone) = if host.unreachable {
            (Icon::CloudOff, Tone::Warning)
        } else {
            (Icon::Cloud, Tone::Secondary)
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(theme.space.xxs)
                .child(icon.el().size(IconSize::Small).color(tone.color(theme)))
                .child(Text::hint(host.name.clone()).tone(tone)),
        )
    }

    /// The completion surface, drawn over the composer while one is open.
    fn picker_element(&self, theme: &fleet_ui_kit::Theme) -> Option<gpui::Div> {
        let picker = self.picker.as_ref()?;
        if picker.is_empty() {
            return None;
        }
        let highlight = picker.highlight();
        let rows: Vec<_> = picker
            .matches()
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                div()
                    .flex()
                    .items_center()
                    .h(theme.metrics.row_h)
                    .px(theme.space.sm)
                    .when(index == highlight, |el| el.bg(theme.colors.row_selected))
                    .child(Text::ui(row.to_owned()))
            })
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .bg(theme.colors.elevated)
                // DESIGN-SYSTEM §2.8: `border_strong` is the hairline that has to survive on
                // top of `elevated`, and the width is the theme's, not a hard-coded pixel.
                .border(theme.metrics.hairline)
                .border_color(theme.colors.border_strong)
                .rounded(theme.radii.md)
                // §2.6 level 2: an `elevated` surface carries a shadow, and this one floats over
                // the composer exactly as `Select`'s open option list floats over its field.
                .shadow(theme.sheet_shadow())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.sm)
                        .px(theme.space.sm)
                        .child(Text::hint(picker.kind.title()).muted())
                        .child(KeyHint::labeled("\u{23ce}", "accept").tone(Tone::Muted)),
                )
                .children(rows),
        )
    }
}
