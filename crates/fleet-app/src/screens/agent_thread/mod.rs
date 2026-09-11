//! The native structured agent tab: a flat transcript over a docked composer (§5, §6, §7).
//!
//! The daemon owns the thread. This screen owns a projection copy, the memoised row model built
//! from it, and the interaction state a client may not persist — expansion, the wizard position,
//! the control draft and the optimistic bubbles. Every mutation leaves as a typed
//! [`BridgeCommand`] emitted to the workspace, so the view issues no I/O itself and renders
//! purely from values it prepared in an update path.
//!
//! Three structural decisions carry the design and none of them is negotiable in code:
//!
//! - **The transcript is a flat list of rows**, never a tree of turn widgets. A turn is an
//!   emergent run of rows ([`rows`]); nested containers make variable-height virtualization and
//!   scroll anchoring unsolvable.
//! - **Approvals and questions dock above the composer.** Only a proposed plan is an
//!   in-transcript card, and it carries no buttons. A card in the transcript can be scrolled out
//!   of the viewport while it still owns the keyboard, which is a modal with the chrome removed.
//! - **Render prepares nothing.** Rows, decisions and metadata segments are all memoised behind
//!   a revision key and `render` composes them.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Instant,
};

use fleet_core::agents::{
    GateId, ItemId, ItemKind, Seq, ThreadId, ThreadProjection, TurnId, UserInput,
};
use fleet_lazygit::diff_view::DiffView;
use fleet_ui_kit::{
    Decision, MetadataFit, MetadataSegment, MultilineInput, MultilineInputEvent, TranscriptEvent,
    TranscriptList, TranscriptRow, TranscriptRowKind,
};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
    prelude::*,
};

use fleet_proto::agents::{CheckpointId, CheckpointScope, TurnCheckpoint};

use crate::bridge::BridgeCommand;

pub(crate) mod actions;
pub(crate) mod composer;
pub(crate) mod decisions;
pub(crate) mod picker;
pub(crate) mod presentation;
pub(crate) mod rows;
mod sync;
#[cfg(test)]
mod tests;
mod view;

use composer::{ComposerMode, ControlDraft, InteractionMode};
use decisions::QuestionWizard;
use picker::Picker;
use presentation::composer_placeholder;
use rows::{PendingSend, ResolvedGate, RowTarget};

/// What the thread asks the workspace to do on its behalf.
#[derive(Debug, Clone)]
pub(crate) enum AgentThreadEvent {
    /// Send a typed native-agent command to the daemon.
    Command(BridgeCommand),
    /// Open a path in the user's editor.
    OpenInEditor(String),
    /// Copy text to the clipboard.
    Copy(String),
    /// Say something short to the user, as a transient toast.
    Notice(SharedString),
    /// The reader reached the oldest row this client holds: load the page behind it.
    ///
    /// Emitted whatever the mirror holds, because the view does not know: the workspace asks the
    /// mirror for a page cursor and does nothing when there is none.
    LoadOlder,
    /// Re-read the checkpoints this thread can be reverted to.
    ///
    /// Separate from [`AgentThreadEvent::Command`] because the answer has to come *back* into
    /// this view: `[u]` is drawn only where a checkpoint exists, so the listing is state the row
    /// model reads rather than a mutation to forget.
    RefreshCheckpoints,
}

/// The machine one thread's worktree lives on, as the thread view has to state it (§12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadHost {
    /// Configured host id, which is what the badge reads.
    pub(crate) name: SharedString,
    /// Whether the daemon reports the link to that machine as down.
    pub(crate) unreachable: bool,
}

/// The memo key of one row projection (`spec-B` §B7.7).
///
/// A cache hit clones an `Rc<[TranscriptRow]>`; a miss rebuilds and splices only the rows that
/// really changed. Everything in the key is a cheap comparable scalar, so checking it costs
/// nothing on the streaming path that consults it most.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RowsKey {
    /// Which thread, so a tab switch can never serve another thread's rows.
    thread: ThreadId,
    /// The last sequence the projection applied.
    last_seq: Seq,
    /// Bumps on any expand or collapse.
    expanded_rev: u32,
    /// Bumps on any turn unfold.
    unfolded_rev: u32,
    /// Bumps on any change to the optimistic bubbles.
    pending_rev: u32,
    /// Bumps when the checkpoint listing changes, because a turn footer gains or loses `[u]`.
    checkpoints_rev: u64,
    /// Which composer mode is live, because the plan-ready reminder depends on it.
    mode: ComposerMode,
}

/// Entity owning one thread projection, its transcript list, and its docked composer.
pub struct AgentThreadView {
    thread: ThreadId,
    projection: ThreadProjection,
    transcript: Entity<TranscriptList>,
    input: Entity<MultilineInput>,
    focus: FocusHandle,

    /// The prepared row model. `render` composes this and never rebuilds it.
    rows: Rc<[TranscriptRow]>,
    /// The key `rows` was built at.
    rows_key: Option<RowsKey>,
    /// What each row key toggles.
    targets: HashMap<SharedString, RowTarget>,
    /// Where each streaming item's row sits, so a `ContentDelta` rewrites exactly one row.
    row_of_item: HashMap<ItemId, usize>,
    /// The prepared decisions, in creation order; the kit applies the priority ladder.
    decisions: Vec<Decision>,
    /// The prepared metadata strip and its per-width fit memo.
    metadata: Vec<MetadataSegment>,
    trailing: Vec<MetadataSegment>,
    metadata_fit: MetadataFit,
    metadata_rev: u32,

    /// One inline diff surface per item that carries a patch, keyed by the row's item id.
    ///
    /// Shared with the transcript's body renderer, which only reads it: a render pass may not
    /// update the view that owns the list it is drawing.
    diffs: Rc<RefCell<HashMap<SharedString, Entity<DiffView>>>>,

    expanded: HashSet<ItemId>,
    unfolded: HashSet<TurnId>,
    expanded_gates: HashSet<GateId>,
    /// The decisions this window watched resolve, oldest first, so the transcript keeps the
    /// history that makes docking the live drawer safe.
    resolved: Vec<ResolvedGate>,
    /// What this window answered each gate with, which is how a record knows its outcome.
    answered: HashMap<GateId, fleet_core::agents::GateAnswer>,
    expanded_rev: u32,
    unfolded_rev: u32,
    pending_rev: u32,

    /// The in-progress answer to the open question request.
    wizard: QuestionWizard,
    /// The gate whose answer is being typed in the composer after `[e]` or `[n]`.
    ///
    /// While one is open the gate's key context stands down, so the draft may start with a `y`.
    gate_draft: Option<GateId>,
    /// The gate whose reply is in flight: every option is disabled and the drawer says
    /// `answering…`. A transient failure restores it to pending.
    answering: Option<GateId>,
    /// Optimistic user bubbles the daemon has not reflected back yet.
    pending: Vec<PendingSend>,
    /// Tier one of §B6.4: what the next send will carry.
    controls: ControlDraft,
    /// The open completion surface.
    picker: Option<Picker>,
    /// Harness slash commands learned from `SessionConfigured`.
    commands: Vec<String>,
    /// Harness skills, which `$` completes.
    skills: Vec<String>,
    /// Worktree paths offered by `@`, filled by the workspace.
    files: Vec<String>,
    /// Whether the transcript's tail is frozen (`^s [`).
    scrolling: bool,
    /// Whether an interrupt is in flight. Held until the daemon reports liveness cleared.
    stopping: bool,
    /// The remote machine the thread runs on, when it is not this one.
    host: Option<ThreadHost>,
    /// The checkpoint each turn can be reverted to, as the daemon last listed them.
    ///
    /// Empty on a daemon without the `agent.checkpoints` capability, and that is exactly why
    /// `[u]` is not drawn there: a drawn affordance that does nothing is worse than an absent
    /// one. The id is kept, not just the turn, because `[u]` has to name what it reverts to.
    checkpoints: HashMap<TurnId, CheckpointId>,
    /// Bumped when the listing changes, so the row memo rebuilds the footers that gained a `[u]`.
    checkpoints_rev: u64,
    /// Settled turns at the last listing, so a new one re-reads it and nothing else does.
    listed_turns: usize,
    /// When the live row's clock started, and which turn it belongs to.
    started_at: Option<Instant>,
    clock_turn: Option<TurnId>,
    _subscriptions: Vec<Subscription>,
}

impl AgentThreadView {
    /// Creates a native agent-thread view from an installed projection.
    #[must_use]
    pub fn new(projection: ThreadProjection, cx: &mut Context<Self>) -> Self {
        let thread = projection.thread;
        let placeholder =
            composer_placeholder(ComposerMode::Normal, projection.provider, None, false);
        let transcript = cx.new(TranscriptList::new);
        let input = cx.new(|cx| MultilineInput::new(cx, placeholder.into()));
        let subscriptions = vec![
            cx.subscribe(&input, Self::on_input_event),
            cx.subscribe(&transcript, Self::on_transcript_event),
        ];
        let diffs: Rc<RefCell<HashMap<SharedString, Entity<DiffView>>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let rendered = Rc::clone(&diffs);
        transcript.update(cx, |list, cx| {
            list.set_row_body(
                move |row, _cx| match &row.kind {
                    // ADR 0010 keeps `fleet_lazygit::DiffView` out of the kit, so the owner
                    // hands the prepared element back per frame instead.
                    TranscriptRowKind::Diff(diff) => rendered
                        .borrow()
                        .get(&diff.item)
                        .map(|view| view.clone().into_any_element()),
                    _ => None,
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
            rows: Rc::from(Vec::new()),
            rows_key: None,
            targets: HashMap::new(),
            row_of_item: HashMap::new(),
            decisions: Vec::new(),
            metadata: Vec::new(),
            trailing: Vec::new(),
            metadata_fit: MetadataFit::new(),
            metadata_rev: 0,
            diffs,
            expanded: HashSet::new(),
            unfolded: HashSet::new(),
            expanded_gates: HashSet::new(),
            resolved: Vec::new(),
            answered: HashMap::new(),
            expanded_rev: 0,
            unfolded_rev: 0,
            pending_rev: 0,
            wizard: QuestionWizard::default(),
            gate_draft: None,
            answering: None,
            pending: Vec::new(),
            controls: ControlDraft::default(),
            picker: None,
            commands: Vec::new(),
            skills: Vec::new(),
            files: Vec::new(),
            scrolling: false,
            stopping: false,
            host: None,
            checkpoints: HashMap::new(),
            checkpoints_rev: 0,
            listed_turns: 0,
            started_at: None,
            clock_turn: None,
            _subscriptions: subscriptions,
        };
        view.prepare(cx);
        let rows = view.rows.to_vec();
        view.transcript
            .update(cx, |list, cx| list.set_thread(rows, cx));
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

    /// The prepared rows, so a test can assert the projection without a window.
    #[cfg(test)]
    pub(crate) fn rows(&self) -> &[TranscriptRow] {
        &self.rows
    }

    /// The prepared decisions, newest gate last.
    #[cfg(test)]
    pub(crate) fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// The sequence the user has now seen, which is what clears `finished` and `unread`.
    pub(crate) fn last_seq(&self) -> Seq {
        self.projection.last_seq
    }

    /// The gate that owns the keyboard, which is always the newest open one.
    pub(crate) fn open_gate(&self) -> Option<&fleet_core::agents::OpenGate> {
        self.projection.gates.last()
    }

    /// §3.3's `Working`: a running turn, a live session, a background task, or a backoff.
    pub(crate) fn is_working(&self) -> bool {
        self.row_inputs_working()
    }

    /// The composer's mode, derived from **daemon state** rather than from the view.
    ///
    /// That derivation is what makes a decision own the keyboard in the same frame the gate
    /// appears rather than one frame later. The one local input is `gate_draft`: while a
    /// correction or a refinement is being typed the gate stands its keys down.
    pub(crate) fn composer_mode(&self) -> ComposerMode {
        if let Some(gate) = self.open_gate() {
            if self.gate_draft == Some(gate.id) {
                return ComposerMode::Normal;
            }
            return match gate.kind {
                fleet_core::agents::GateKind::Permission { .. } => ComposerMode::Approval(gate.id),
                fleet_core::agents::GateKind::Question { .. } => ComposerMode::Question(gate.id),
                fleet_core::agents::GateKind::Plan { .. } => {
                    // A plan gate's verbs live on the composer, and the composer is the field
                    // the refinement is typed into: it is a follow-up, not an answer form.
                    ComposerMode::PlanFollowUp(
                        self.projection
                            .items
                            .iter()
                            .rev()
                            .find(|item| matches!(item.kind, ItemKind::Plan { .. }))
                            .map_or_else(ItemId::new, |item| item.id),
                    )
                }
            };
        }
        decisions::plan_item(&self.projection).map_or(ComposerMode::Normal, |(item, _)| {
            ComposerMode::PlanFollowUp(item)
        })
    }

    /// Whether the composer, rather than the open decision, owns the bare letters right now.
    ///
    /// `[e]` on a permission, `[n]` on a plan and a question's free-text answer all *open* the
    /// composer rather than answering at once, and while that draft is being typed the gate's
    /// context stands down so the text may start with a `y`.
    pub(crate) fn is_composing(&self, _cx: &App) -> bool {
        self.gate_draft.is_some() || self.wizard.is_composing()
    }

    /// Whether the transcript's tail is frozen, which `Agent > AgentNativeScroll` derives from.
    #[must_use]
    pub(crate) const fn is_scrolling(&self) -> bool {
        self.scrolling
    }

    /// Whether an interrupt is in flight, which is what holds `stopping…` on the tab.
    #[must_use]
    pub(crate) const fn is_stopping(&self) -> bool {
        self.stopping
    }

    /// The question an open request's bare keys address, which the status bar mirrors.
    #[must_use]
    pub(crate) const fn question_cursor(&self) -> usize {
        self.wizard.cursor()
    }

    /// Build or Plan, as the next send will carry it.
    #[must_use]
    pub(crate) fn interaction_mode(&self) -> InteractionMode {
        self.controls.interaction(self.projection.mode)
    }

    /// Whether the thread's machine is out of reach, which stands the composer down.
    #[must_use]
    pub(crate) fn is_unreachable(&self) -> bool {
        self.host.as_ref().is_some_and(|host| host.unreachable)
    }

    /// The machine this thread runs on, when it is a remote one.
    #[must_use]
    pub(crate) const fn host(&self) -> Option<&ThreadHost> {
        self.host.as_ref()
    }

    /// Names the machine this thread runs on, or clears the badge for a local one (§12).
    pub(crate) fn set_host(&mut self, host: Option<ThreadHost>, cx: &mut Context<Self>) {
        if self.host == host {
            return;
        }
        self.host = host;
        self.sync_composer(cx);
        cx.notify();
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

    /// Replaces the harness slash commands `/` completes and the skills `$` completes.
    pub(crate) fn set_commands(&mut self, commands: Vec<String>) {
        self.commands = commands;
    }

    /// Replaces the harness skills `$` completes.
    pub(crate) fn set_skills(&mut self, skills: Vec<String>) {
        self.skills = skills;
    }

    /// Emits one typed command for the workspace to send.
    pub(crate) fn dispatch(&mut self, command: BridgeCommand, cx: &mut Context<Self>) {
        cx.emit(AgentThreadEvent::Command(command));
        cx.notify();
    }

    /// Adopts the checkpoint listing the daemon answered with.
    ///
    /// Only `Turn` scope lands here: a turn footer's `[u] revert turn` reverts the whole
    /// worktree to the moment before the turn was submitted. File-scope checkpoints are listed
    /// too but name a turn rather than the edit they covered, so there is nothing for a tool row
    /// to key on yet — that is recorded as a gap in `docs/NATIVE-AGENTS.md` §13 rather than
    /// guessed at by ordinal.
    ///
    /// The newest checkpoint of a turn wins, which is what a re-submitted turn leaves behind.
    pub(crate) fn install_checkpoints(
        &mut self,
        checkpoints: &[TurnCheckpoint],
        cx: &mut Context<Self>,
    ) {
        let mut next: HashMap<TurnId, CheckpointId> = HashMap::new();
        let mut ordinals: HashMap<TurnId, u32> = HashMap::new();
        for checkpoint in checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.scope == CheckpointScope::Turn)
        {
            if ordinals
                .get(&checkpoint.turn)
                .is_none_or(|seen| *seen <= checkpoint.ordinal)
            {
                ordinals.insert(checkpoint.turn, checkpoint.ordinal);
                next.insert(checkpoint.turn, checkpoint.id.clone());
            }
        }
        if next == self.checkpoints {
            return;
        }
        self.checkpoints = next;
        self.checkpoints_rev = self.checkpoints_rev.wrapping_add(1);
        self.prepare(cx);
        self.install_rows_for_send(cx);
        cx.notify();
    }

    /// Says something short to the user.
    pub(crate) fn notice(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        cx.emit(AgentThreadEvent::Notice(text.into()));
    }

    /// Takes the keyboard for the composer, which is what an agent tab focuses.
    ///
    /// The composer holds the focus handle for the whole tab in **every** mode, approval
    /// included: that is what makes `[e]`, `[n]` on a plan and a question's free-text answer
    /// work without a focus round trip. A composer standing down still needs an owner, so the
    /// view's own handle is the fallback — gpui dispatches through ancestors, so focusing it
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

    /// Bridges a click or a row-focus key on the transcript into the same intent `⏎` has.
    fn on_transcript_event(
        &mut self,
        _transcript: Entity<TranscriptList>,
        event: &TranscriptEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TranscriptEvent::Toggle(key) => self.toggle_row(key.clone(), cx),
            TranscriptEvent::RowAction { row, action } => {
                self.row_action(row.clone(), *action, cx);
            }
            // The transcript reports the gesture; whether older history exists is the owner's
            // question, and only the mirror's page cursor can answer it (§8).
            TranscriptEvent::ReachedOldest => cx.emit(AgentThreadEvent::LoadOlder),
        }
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
            MultilineInputEvent::Trigger(trigger) => {
                if let Some(kind) = picker::PickerKind::for_trigger(trigger.symbol) {
                    self.open_picker(kind, cx);
                }
            }
            // The composer reports every change, which is what lets `@`, `$` and `/` stay
            // ordinary typable characters: the picker re-filters on the text after the symbol
            // instead of the keymap consuming the key.
            MultilineInputEvent::Changed => self.on_composer_changed(cx),
            MultilineInputEvent::Escape => self.stop(cx),
        }
    }
}

impl EventEmitter<AgentThreadEvent> for AgentThreadView {}

impl Focusable for AgentThreadView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// One user message, as the daemon receives it.
///
/// `item` is the identity the optimistic bubble was already drawn under, and it travels with the
/// message: the adapter adopts it, so the daemon's copy of the row **is** the client's row rather
/// than one the client has to re-join by text (§9.4, §13).
pub(crate) fn user_input(text: String, item: ItemId) -> UserInput {
    UserInput {
        text,
        attachments: Vec::new(),
        item: Some(item),
    }
}

/// The `RowTarget` a row key addresses, for the view's toggles.
pub(crate) fn target_of(
    targets: &HashMap<SharedString, RowTarget>,
    key: &SharedString,
) -> Option<RowTarget> {
    targets.get(key).copied()
}
