//! Gate presentation and key routing for the docked decision drawer (§6, `spec-B` §B4).
//!
//! A gate is projected into the kit's domain-neutral [`Decision`], and the bare keys the
//! `Agent > AgentDecision > *` contexts dispatch are turned back into a harness-neutral
//! [`GateAnswer`]. Both directions are pure, so the whole routing table is testable without a
//! window — and the key *vocabulary* lives in the kit, which is what lets the status bar mirror
//! the drawer from the same source and never advertise a scope the drawer does not offer.

use fleet_core::agents::{
    AgentKind, GateAnswer, GateId, GateKind, ItemId, ItemKind, OpenGate, PermissionChoice,
    PlanAnswer, Question, ThreadProjection, TurnState,
};
use fleet_ui_kit::{
    ApprovalRequest, Decision, DecisionAction, DecisionKind, DecisionQuestion, QuestionOption,
    QuestionSet, SOMETHING_ELSE, parse_markdown_document,
};
use gpui::SharedString;

use super::rows::item::{kind_word, split_plan};

/// The de-facto prefix both harnesses read as "go do it". Copied verbatim: rewording it risks
/// changing the harness's behaviour.
pub(crate) const PLAN_IMPLEMENTATION_PROMPT_PREFIX: &str = "PLEASE IMPLEMENT THIS PLAN:\n";

/// The key context one gate owns while it is open.
#[must_use]
pub(crate) const fn decision_context(gate: &OpenGate) -> &'static str {
    match gate.kind {
        GateKind::Permission { .. } => "AgentPermission",
        GateKind::Question { .. } => "AgentQuestion",
        GateKind::Plan { .. } => "AgentPlan",
    }
}

/// The key context a plan the harness produced as an *item* owns.
pub(crate) const PLAN_CONTEXT: &str = "AgentPlan";

/// The user's in-progress answer to a question request (`spec-B` §B4.3).
///
/// One request carries one to four questions and they are answered one at a time. The wizard
/// position is per request, and the per-question free-text drafts are kept beside the
/// selections: pressing `[p]` must never lose a typed answer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct QuestionWizard {
    /// The question the bare keys address.
    cursor: usize,
    /// Chosen option indices, one entry per question.
    selected: Vec<Vec<usize>>,
    /// Free-text answers, one entry per question.
    custom: Vec<String>,
}

impl QuestionWizard {
    /// A wizard sized for `questions`, at its first question with nothing chosen.
    pub(crate) fn new(questions: usize) -> Self {
        Self {
            cursor: 0,
            selected: vec![Vec::new(); questions],
            custom: vec![String::new(); questions],
        }
    }

    /// A wizard sized for `questions` whose keys address question `cursor`.
    ///
    /// The status bar mirrors the drawer's keys and rebuilds the decision from `AppState` rather
    /// than from the view's live selection, so it needs the cursor without the choices.
    pub(crate) fn at(questions: usize, cursor: usize) -> Self {
        let mut wizard = Self::new(questions);
        wizard.cursor = cursor.min(questions.saturating_sub(1));
        wizard
    }

    /// Resize the wizard for a different request, keeping nothing.
    pub(crate) fn resize(&mut self, questions: usize) {
        if self.selected.len() != questions {
            *self = Self::new(questions);
        }
    }

    /// The question the bare keys address.
    pub(crate) const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether a free-text answer is being typed for the question under the cursor.
    ///
    /// While one is, the gate's key context stands down: the answer may start with a `y`, and
    /// `space` is one of the drawer's keys.
    pub(crate) fn is_composing(&self) -> bool {
        self.custom
            .get(self.cursor)
            .is_some_and(|text| !text.trim().is_empty())
    }

    /// The free-text answer of the question under the cursor.
    pub(crate) fn custom(&self) -> &str {
        self.custom.get(self.cursor).map_or("", String::as_str)
    }

    /// Writes the free-text answer of the question under the cursor.
    ///
    /// Typing and selecting are **mutually exclusive**, enforced here rather than in the view:
    /// a non-empty custom answer clears the selection, and choosing an option clears the text.
    pub(crate) fn set_custom(&mut self, text: impl Into<String>) {
        let text = text.into();
        let cursor = self.cursor;
        if !text.trim().is_empty()
            && let Some(selected) = self.selected.get_mut(cursor)
        {
            selected.clear();
        }
        if let Some(slot) = self.custom.get_mut(cursor) {
            *slot = text;
        }
    }

    /// `1`–`9`: choose an option, toggling instead of replacing on a multi-select question.
    pub(crate) fn choose(&mut self, option: usize, multi_select: bool) {
        let cursor = self.cursor;
        if let Some(slot) = self.custom.get_mut(cursor) {
            slot.clear();
        }
        let Some(selected) = self.selected.get_mut(cursor) else {
            return;
        };
        if multi_select {
            if let Some(position) = selected.iter().position(|chosen| *chosen == option) {
                selected.remove(position);
            } else {
                selected.push(option);
            }
            return;
        }
        selected.clear();
        selected.push(option);
    }

    /// `space`: toggle the newest selection of a multi-select question.
    pub(crate) fn toggle(&mut self, multi_select: bool) {
        if !multi_select {
            return;
        }
        let cursor = self.cursor;
        let newest = self
            .selected
            .get(cursor)
            .and_then(|selected| selected.last().copied())
            .unwrap_or(0);
        self.choose(newest, true);
    }

    /// `⏎` on a question that is not the last: step forward.
    pub(crate) fn advance(&mut self, questions: usize) -> bool {
        if self.cursor + 1 < questions {
            self.cursor += 1;
            return true;
        }
        false
    }

    /// `[p]`: step back, keeping every answer already given.
    pub(crate) fn previous(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        true
    }

    /// Whether the question under the cursor carries an answer.
    fn answered(&self, index: usize) -> bool {
        self.selected
            .get(index)
            .is_some_and(|selected| !selected.is_empty())
            || self
                .custom
                .get(index)
                .is_some_and(|text| !text.trim().is_empty())
    }

    /// The harness answer, one label array per question in question order.
    ///
    /// All-or-nothing: `None` while any question is unanswered, so `⏎` on an incomplete wizard
    /// keeps the drawer open rather than sending an empty array the user was shown as a choice.
    /// A selection whose index the question no longer offers drops silently, and an option's own
    /// `label` is sent rather than its display text — never a label when the option carries a
    /// value, and option identifiers are never trimmed.
    pub(crate) fn answers(&self, questions: &[Question]) -> Option<Vec<Vec<String>>> {
        let mut answers = Vec::with_capacity(questions.len());
        for (index, question) in questions.iter().enumerate() {
            if !self.answered(index) {
                return None;
            }
            let custom = self.custom.get(index).map_or("", String::as_str).trim();
            // A custom answer wins over a selection, but only where one is allowed.
            if question.allows_other && !custom.is_empty() {
                answers.push(vec![custom.to_owned()]);
                continue;
            }
            let mut labels: Vec<String> = self
                .selected
                .get(index)
                .into_iter()
                .flatten()
                .filter_map(|option| match question.options.get(*option) {
                    Some(choice) => Some(choice.label.clone()),
                    // The free-text row sits one past the harness's own options.
                    None if question.allows_other && !custom.is_empty() => Some(custom.to_owned()),
                    None => None,
                })
                .collect();
            if labels.is_empty() {
                return None;
            }
            if !question.multi_select {
                labels.truncate(1);
            }
            answers.push(labels);
        }
        Some(answers)
    }

    /// The selection, as the kit's drawer draws it.
    fn selection(&self) -> Vec<Vec<usize>> {
        self.selected.clone()
    }
}

/// What a routed key did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Routed {
    /// Send this answer to the daemon and clear the gate optimistically.
    Answer(GateAnswer),
    /// Send this text as a new turn — a plan the harness produced as an item has no gate.
    Send {
        /// The turn's text.
        text: String,
        /// Whether the turn runs in plan mode.
        plan_mode: bool,
    },
    /// The key changed local state only: a selection, a wizard step, an expansion.
    Local,
    /// Seed the composer with this text and stand the gate's keys down.
    Compose(String),
    /// The key claimed nothing.
    None,
}

/// The pending decisions of one thread, newest gate last, in creation order.
///
/// The kit's [`Decision::head`] applies the priority ladder — approval > question > plan-ready —
/// over this list, so the ordering here is creation order and nothing else.
pub(crate) fn decisions(
    projection: &ThreadProjection,
    wizard: &QuestionWizard,
    answering: Option<GateId>,
) -> Vec<Decision> {
    let mut pending: Vec<Decision> = projection
        .gates
        .iter()
        .map(|gate| decision_for(gate, projection.provider, wizard))
        .collect();
    if let Some(plan) = plan_ready(projection) {
        pending.push(plan);
    }
    let total = pending.len();
    for (index, decision) in pending.iter_mut().enumerate() {
        let answering = answering.is_some_and(|gate| gate.to_string() == decision.id.as_ref());
        *decision = decision.clone().queued(index, total).answering(answering);
    }
    pending
}

/// The drawer's one-line plan reminder, when a plan the harness produced as an item is ready.
///
/// Claude's plan arrives as a gate and is covered by the loop above; Codex's arrives as an
/// `item/completed` with no request pending, so the verbs have to be derived. All of the §6.4
/// conditions that Fleet can answer locally hold here: the latest turn has settled, the thread
/// is in plan mode, and no user message follows the plan.
fn plan_ready(projection: &ThreadProjection) -> Option<Decision> {
    if projection.mode != fleet_core::agents::PermissionMode::Plan {
        return None;
    }
    if matches!(projection.turn, TurnState::Running(_)) {
        return None;
    }
    let (index, item) = projection
        .items
        .iter()
        .enumerate()
        .rev()
        .find(|(_, item)| matches!(item.kind, ItemKind::Plan { .. }))?;
    // A plan the user has already answered by typing something else is not actionable: the
    // implicit rejection §6.4 describes is exactly "a user message after the plan".
    let answered = projection
        .items
        .iter()
        .skip(index + 1)
        .any(|later| matches!(later.kind, ItemKind::UserMessage { .. }));
    if answered {
        return None;
    }
    let ItemKind::Plan { text } = &item.kind else {
        return None;
    };
    let (title, body) = split_plan(text);
    Some(Decision::new(
        item.id.to_string(),
        SharedString::from(format!("plan ready \u{b7} {title}")),
        DecisionKind::PlanReady {
            title,
            markdown: Some(parse_markdown_document(&body)),
        },
    ))
}

/// The plan item the composer's verbs act on, when the drawer is showing one.
pub(crate) fn plan_item(projection: &ThreadProjection) -> Option<(ItemId, String)> {
    let decision = plan_ready(projection)?;
    let item = projection
        .items
        .iter()
        .find(|item| item.id.to_string() == decision.id.as_ref())?;
    let ItemKind::Plan { text } = &item.kind else {
        return None;
    };
    Some((item.id, text.clone()))
}

/// One gate, as the drawer draws it.
pub(crate) fn decision_for(
    gate: &OpenGate,
    provider: AgentKind,
    wizard: &QuestionWizard,
) -> Decision {
    let id = SharedString::from(gate.id.to_string());
    match &gate.kind {
        GateKind::Permission {
            tool,
            title,
            payload,
            rationale,
            options,
        } => {
            let mut approval = ApprovalRequest::new(
                SharedString::from(kind_word(tool)),
                SharedString::new(payload.as_str()),
            )
            // §6.2 / DESIGN-SYSTEM §7: `[e]` is drawn only where the harness accepts an
            // amended invocation — Claude does, Codex never does.
            .allows_edit(
                options
                    .iter()
                    .any(|option| option.label == PermissionChoice::Edit),
            );
            if let Some(rationale) = rationale
                .as_deref()
                .filter(|rationale| adds_to(rationale, title, payload))
            {
                approval = approval.rationale(SharedString::new(rationale));
            }
            // The `[a]` label always spells the promise it makes. The word "always" never
            // appears on a command or a file change on either harness.
            approval = approval.session_label(session_label(gate));
            Decision::new(
                id,
                SharedString::from(format!(
                    "{} wants to {}",
                    provider.executable(),
                    kind_word(tool)
                )),
                DecisionKind::Approval(approval),
            )
        }
        GateKind::Question { questions } => {
            let mut wizard = wizard.clone();
            wizard.resize(questions.len());
            let set = QuestionSet::new(questions.iter().map(question_for).collect())
                .cursor(wizard.cursor().min(questions.len().saturating_sub(1)))
                .selected(wizard.selection());
            let set = QuestionSet {
                custom: wizard.is_composing(),
                ..set
            };
            Decision::new(
                id,
                SharedString::from(format!("{} asks", provider.executable())),
                DecisionKind::Question(set),
            )
        }
        GateKind::Plan { markdown, .. } => {
            let (title, body) = split_plan(markdown);
            Decision::new(
                id,
                SharedString::from(format!("plan ready \u{b7} {title}")),
                DecisionKind::PlanReady {
                    title,
                    markdown: Some(parse_markdown_document(&body)),
                },
            )
        }
    }
}

/// One harness question, as the drawer draws it.
///
/// The free-text row is appended by Fleet, not by the harness, and only where the harness
/// accepts one — so a question that forbids custom answers draws no row no key can honour.
fn question_for(question: &Question) -> DecisionQuestion {
    let mut options: Vec<QuestionOption> = question
        .options
        .iter()
        .map(|choice| {
            let option = QuestionOption::new(SharedString::new(choice.label.as_str()));
            if choice.description.trim().is_empty() {
                option
            } else {
                option.description(SharedString::new(choice.description.as_str()))
            }
        })
        .collect();
    if question.allows_other {
        options.push(QuestionOption::new(SharedString::new_static(
            SOMETHING_ELSE,
        )));
    }
    DecisionQuestion::new(
        SharedString::new(question.header.as_str()),
        SharedString::new(question.prompt.as_str()),
    )
    .options(options)
    .multi_select(question.multi_select)
    .allow_other(question.allows_other)
}

/// The `[a]` label of one permission, which always states the scope it really grants.
fn session_label(gate: &OpenGate) -> SharedString {
    let GateKind::Permission { options, .. } = &gate.kind else {
        return SharedString::new_static("allow for this session");
    };
    if options
        .iter()
        .any(|option| option.label == PermissionChoice::AllowDirectory)
    {
        return SharedString::new_static("allow for this directory");
    }
    SharedString::new_static("allow for this session")
}

/// The wider grant `[a]` means for this gate, narrowed to what the adapter mapped.
///
/// §6.2 offers no directory or project scope in v1, so `[a]` is a session grant on both
/// harnesses; a harness whose own option list carries a directory scope is still honoured,
/// because the drawer renders exactly the options the adapter mapped and never a wider one.
fn session_choice(gate: &OpenGate) -> PermissionChoice {
    let GateKind::Permission { options, .. } = &gate.kind else {
        return PermissionChoice::AllowSession;
    };
    for wider in [
        PermissionChoice::AllowSession,
        PermissionChoice::AllowDirectory,
    ] {
        if options.iter().any(|option| option.label == wider) {
            return wider;
        }
    }
    // A harness that advertises no wider scope gets the narrow one rather than a promise the
    // wire cannot carry.
    PermissionChoice::AllowOnce
}

/// Routes one drawer action against the gate that owns the keyboard.
pub(crate) fn route(
    gate: &OpenGate,
    action: &DecisionAction,
    wizard: &mut QuestionWizard,
    typed: &str,
) -> Routed {
    match (&gate.kind, action) {
        (GateKind::Permission { .. }, DecisionAction::Edit) => {
            let GateKind::Permission { payload, .. } = &gate.kind else {
                return Routed::None;
            };
            // `[e]` *opens* the composer rather than answering at once, so the correction can
            // start with a `y` and contain spaces.
            Routed::Compose(payload.clone())
        }
        (GateKind::Permission { .. }, action) => {
            let choice = match action {
                DecisionAction::AllowOnce => PermissionChoice::AllowOnce,
                DecisionAction::AllowSession => session_choice(gate),
                DecisionAction::Deny => PermissionChoice::Deny,
                DecisionAction::DenyAndStop => PermissionChoice::DenyAndStop,
                _ => return Routed::None,
            };
            Routed::Answer(GateAnswer::Permission {
                choice,
                edited_payload: None,
            })
        }
        (GateKind::Question { questions }, action) => {
            let multi = questions
                .get(wizard.cursor())
                .is_some_and(|question| question.multi_select);
            match action {
                DecisionAction::Choose(option) => {
                    let offered = questions.get(wizard.cursor()).map_or(0, |question| {
                        question.options.len() + usize::from(question.allows_other)
                    });
                    // A `3` on a two-option question is ignored, never stored as an answer no
                    // label matches.
                    if *option >= offered {
                        return Routed::None;
                    }
                    wizard.choose(*option, multi);
                    // Single-select advances so the wizard walks itself; multi-select waits for
                    // `⏎`, because the next `space` still belongs to this question.
                    if !multi {
                        wizard.advance(questions.len());
                    }
                    Routed::Local
                }
                DecisionAction::Toggle => {
                    wizard.toggle(multi);
                    Routed::Local
                }
                DecisionAction::Previous => {
                    wizard.previous();
                    Routed::Local
                }
                DecisionAction::Answer => {
                    if !typed.trim().is_empty() {
                        wizard.set_custom(typed);
                    }
                    if wizard.advance(questions.len()) {
                        return Routed::Local;
                    }
                    wizard.answers(questions).map_or(Routed::Local, |answers| {
                        Routed::Answer(GateAnswer::Question { answers })
                    })
                }
                _ => Routed::None,
            }
        }
        (GateKind::Plan { .. }, DecisionAction::Implement) => {
            Routed::Answer(GateAnswer::Plan(PlanAnswer::Approve))
        }
        (GateKind::Plan { .. }, DecisionAction::Refine) if typed.trim().is_empty() => {
            // `[n]` opens the composer rather than answering at once: refining *is* the note.
            Routed::Compose(String::new())
        }
        (GateKind::Plan { .. }, DecisionAction::Refine) => {
            Routed::Answer(GateAnswer::Plan(PlanAnswer::AskForChanges {
                note: typed.trim().to_owned(),
            }))
        }
        _ => Routed::None,
    }
}

/// Routes one drawer action against a plan the harness produced as an item, which has no gate.
///
/// The verbs live on the composer, and which one fires is decided by whether the user typed
/// anything: empty implements, non-empty refines. There is no reject verb and none on the wire.
pub(crate) fn route_plan_item(action: &DecisionAction, markdown: &str, typed: &str) -> Routed {
    match action {
        DecisionAction::Implement if typed.trim().is_empty() => Routed::Send {
            text: format!("{PLAN_IMPLEMENTATION_PROMPT_PREFIX}{markdown}"),
            plan_mode: false,
        },
        // A composer holding text means refine, whichever key was pressed: the state decides.
        DecisionAction::Implement | DecisionAction::Refine if !typed.trim().is_empty() => {
            Routed::Send {
                text: typed.trim().to_owned(),
                plan_mode: true,
            }
        }
        DecisionAction::Refine => Routed::Compose(String::new()),
        _ => Routed::None,
    }
}

/// Whether a harness rationale is worth its own line on a permission drawer.
///
/// The drawer is title + payload + keys; a rationale earns the fourth line only by *adding*
/// something. Claude's `description` is often the bare file name the payload already spells out
/// in full, and a card that says the same thing three times reads as three separate facts.
fn adds_to(rationale: &str, title: &str, payload: &str) -> bool {
    let rationale = rationale.trim();
    !rationale.is_empty()
        && !title.contains(rationale)
        && !payload.lines().any(|line| line.trim().contains(rationale))
}
