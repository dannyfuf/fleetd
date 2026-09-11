//! The decision vocabulary: what a pending approval, question or plan-ready reminder *is*, and
//! which key answers it.
//!
//! §6 of `docs/NATIVE-AGENTS.md`: **one slot, one occupant, strict priority approval >
//! question > plan-ready.** When several requests are pending only the head is actionable and
//! the rest are a `1/N` counter; there is **no "approve all"**.
//!
//! The types own the key *vocabulary* ([`Decision::action_for_key`],
//! [`Decision::key_hints`]); the surface holding the focus handle owns the key *event* and
//! routes it here. That split is what stops one thread from answering another thread's request,
//! and it is why the status bar can mirror the drawer's hints **from the same source** — the bar
//! can never advertise a scope the drawer does not offer.

use gpui::SharedString;

use crate::components::{KeyHint, KeyHintRow, MarkdownDocument};

/// The most numbered options one question may draw, honour and advertise.
///
/// `docs/KEYMAP.md` gives the question context one binding per digit, so this is the last digit
/// that dispatches: four harness options plus the [`SOMETHING_ELSE`] row a question that accepts
/// free text appends. It bounds the rows the panel draws as well as the keys it answers — a row
/// numbered past the last bound digit is an affordance no key can reach.
pub const MAX_QUESTION_OPTIONS: usize = 5;

/// The free-text option every question that accepts one offers last.
pub const SOMETHING_ELSE: &str = "Something else…";

/// What a key on the open decision does.
///
/// There is deliberately no `AllowDirectory` and no "always": §6.2 has no directory or project
/// scope in v1, because writing `.claude/settings.local.json` behind the user's back is not a v1
/// decision — and Codex's wire silently downgrades `acceptAlways` to a session allow, so
/// advertising it would promise something weaker than the word means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionAction {
    /// Allow this invocation only.
    AllowOnce,
    /// Allow matching calls for the rest of this session.
    AllowSession,
    /// Deny, and let the agent try something else. On Codex the turn continues.
    Deny,
    /// Deny and interrupt the turn.
    DenyAndStop,
    /// Open the composer seeded with the invocation, so it can be amended.
    Edit,
    /// Choose one zero-based option of the question under the cursor.
    Choose(usize),
    /// Toggle the current selection of a multi-select question.
    Toggle,
    /// Submit the current question, or advance the wizard.
    Answer,
    /// Step back to the previous question of a multi-question request.
    Previous,
    /// Implement the ready plan.
    Implement,
    /// Refine the ready plan — this opens the composer rather than answering at once.
    Refine,
}

/// One labelled action a decision advertises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOption {
    /// The keycap, as the canvas writes it.
    pub key: SharedString,
    /// What the key does, spelling out its effective scope.
    pub label: SharedString,
    /// What it dispatches.
    pub action: DecisionAction,
}

impl DecisionOption {
    /// A key and what it does.
    #[must_use]
    pub fn new(
        key: impl Into<SharedString>,
        label: impl Into<SharedString>,
        action: DecisionAction,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            action,
        }
    }
}

/// One option of a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    /// What the option says.
    pub label: SharedString,
    /// The harness's own description, suppressed when it equals the label.
    pub description: Option<SharedString>,
}

impl QuestionOption {
    /// An option with no description.
    #[must_use]
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            description: None,
        }
    }

    /// Attach a description. It is dropped when it repeats the label.
    #[must_use]
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        let description = description.into();
        self.description = (description != self.label).then_some(description);
        self
    }
}

/// One question of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionQuestion {
    /// The short heading, which is also the disclosure control.
    pub header: SharedString,
    /// The question itself.
    pub prompt: SharedString,
    /// The options offered.
    pub options: Vec<QuestionOption>,
    /// Whether several options may be chosen.
    pub multi_select: bool,
    /// Whether a free-text answer is accepted.
    pub allow_other: bool,
}

impl DecisionQuestion {
    /// A single-select question with no free-text option.
    #[must_use]
    pub fn new(header: impl Into<SharedString>, prompt: impl Into<SharedString>) -> Self {
        Self {
            header: header.into(),
            prompt: prompt.into(),
            options: Vec::new(),
            multi_select: false,
            allow_other: false,
        }
    }

    /// Set the options.
    #[must_use]
    pub fn options(mut self, options: Vec<QuestionOption>) -> Self {
        self.options = options;
        self
    }

    /// Allow several selections.
    #[must_use]
    pub const fn multi_select(mut self, multi_select: bool) -> Self {
        self.multi_select = multi_select;
        self
    }

    /// Accept a free-text answer.
    #[must_use]
    pub const fn allow_other(mut self, allow_other: bool) -> Self {
        self.allow_other = allow_other;
        self
    }

    /// How many numbered rows this question draws and honours.
    #[must_use]
    pub fn option_count(&self) -> usize {
        (self.options.len() + usize::from(self.allow_other)).min(MAX_QUESTION_OPTIONS)
    }
}

/// A protected invocation awaiting a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// The tool's own name, as the harness spells it.
    pub tool: SharedString,
    /// **The invocation**, never the model's prose about it, because this is what `[e]` seeds
    /// the composer with. It is never truncated and never line-clamped.
    pub payload: SharedString,
    /// A harness-supplied rationale, folded into the same block.
    pub rationale: Option<SharedString>,
    /// A harness-supplied caution — a prompt-injection warning — carried as a `⚠` prefix on one
    /// line, never as a second line.
    pub caution: Option<SharedString>,
    /// Whether the harness accepts an amended invocation. Claude does; Codex does not, and
    /// there the key is unbound and the hint absent.
    pub allows_edit: bool,
    /// The scope label of `[a]`, which always spells the promise it makes.
    pub session_label: SharedString,
}

impl ApprovalRequest {
    /// An approval for one invocation.
    #[must_use]
    pub fn new(tool: impl Into<SharedString>, payload: impl Into<SharedString>) -> Self {
        Self {
            tool: tool.into(),
            payload: payload.into(),
            rationale: None,
            caution: None,
            allows_edit: false,
            session_label: SharedString::new_static("allow for this session"),
        }
    }

    /// Attach the harness's rationale.
    #[must_use]
    pub fn rationale(mut self, rationale: impl Into<SharedString>) -> Self {
        self.rationale = Some(rationale.into());
        self
    }

    /// Attach a caution.
    #[must_use]
    pub fn caution(mut self, caution: impl Into<SharedString>) -> Self {
        self.caution = Some(caution.into());
        self
    }

    /// Offer `[e]`.
    #[must_use]
    pub const fn allows_edit(mut self, allows_edit: bool) -> Self {
        self.allows_edit = allows_edit;
        self
    }

    /// Override the `[a]` label.
    #[must_use]
    pub fn session_label(mut self, label: impl Into<SharedString>) -> Self {
        self.session_label = label.into();
        self
    }
}

/// A request's questions, plus where the wizard is and what has been chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionSet {
    /// One to four questions, rendered one at a time.
    pub questions: Vec<DecisionQuestion>,
    /// Which question the bare keys address.
    pub cursor: usize,
    /// The chosen option indices, one entry per question.
    pub selected: Vec<Vec<usize>>,
    /// Whether a free-text answer is being typed, which suppresses the checkmarks.
    pub custom: bool,
}

impl QuestionSet {
    /// A set at its first question with nothing chosen.
    #[must_use]
    pub fn new(questions: Vec<DecisionQuestion>) -> Self {
        let selected = vec![Vec::new(); questions.len()];
        Self {
            questions,
            cursor: 0,
            selected,
            custom: false,
        }
    }

    /// Move the wizard.
    #[must_use]
    pub const fn cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor;
        self
    }

    /// Set the selections.
    #[must_use]
    pub fn selected(mut self, selected: Vec<Vec<usize>>) -> Self {
        self.selected = selected;
        self
    }

    /// The question the bare keys address.
    #[must_use]
    pub fn active(&self) -> Option<&DecisionQuestion> {
        self.questions.get(self.cursor)
    }

    /// Whether option `option` of question `question` is chosen.
    #[must_use]
    pub fn is_selected(&self, question: usize, option: usize) -> bool {
        !self.custom
            && self
                .selected
                .get(question)
                .is_some_and(|chosen| chosen.contains(&option))
    }
}

/// What the drawer's one slot is holding.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionKind {
    /// A protected invocation.
    Approval(ApprovalRequest),
    /// One to four questions.
    Question(QuestionSet),
    /// The lowest-priority occupant: a one-line reminder that a plan is ready. The plan itself
    /// is a transcript card; only its verbs live here.
    PlanReady {
        /// The plan's title.
        title: SharedString,
        /// The plan body, for the reminder's own disclosure. Parsed by the projection.
        markdown: Option<MarkdownDocument>,
    },
}

impl DecisionKind {
    /// The priority ladder: lower sorts first, and only the head is actionable.
    #[must_use]
    pub const fn priority(&self) -> u8 {
        match self {
            DecisionKind::Approval(_) => 0,
            DecisionKind::Question(_) => 1,
            DecisionKind::PlanReady { .. } => 2,
        }
    }
}

/// One pending decision, and where it sits in the queue.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The gate's stable id.
    pub id: SharedString,
    /// The one-line title: `claude wants to run a command`.
    pub title: SharedString,
    /// What is being asked.
    pub kind: DecisionKind,
    /// This decision's position in the queue, zero-based.
    pub index: usize,
    /// How many decisions are pending in total, which is what draws `1/N`.
    pub queued: usize,
    /// Whether a reply is in flight: every option is disabled and the drawer says `answering…`.
    pub answering: bool,
}

impl Decision {
    /// A decision at the head of a queue of one.
    #[must_use]
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        kind: DecisionKind,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind,
            index: 0,
            queued: 1,
            answering: false,
        }
    }

    /// Place the decision in a queue.
    #[must_use]
    pub const fn queued(mut self, index: usize, queued: usize) -> Self {
        self.index = index;
        self.queued = queued;
        self
    }

    /// Mark the reply as in flight.
    #[must_use]
    pub const fn answering(mut self, answering: bool) -> Self {
        self.answering = answering;
        self
    }

    /// The head of a pending set: strict priority, then ascending creation order.
    ///
    /// The set arrives in creation order, which is why the sort is by priority alone — a stable
    /// sort keeps the older request first inside a priority band.
    #[must_use]
    pub fn head(pending: &[Decision]) -> Option<&Decision> {
        pending
            .iter()
            .enumerate()
            .min_by_key(|(order, decision)| (decision.kind.priority(), *order))
            .map(|(_, decision)| decision)
    }

    /// The options this decision advertises, in canvas order.
    ///
    /// Only what the harness can honour: `[e]` appears only where an amended invocation is
    /// accepted, `space` only on a multi-select question, `[p]` only past the first question.
    #[must_use]
    pub fn options(&self) -> Vec<DecisionOption> {
        match &self.kind {
            DecisionKind::Approval(approval) => {
                let mut options = vec![
                    DecisionOption::new("y", "allow once", DecisionAction::AllowOnce),
                    DecisionOption::new(
                        "a",
                        approval.session_label.clone(),
                        DecisionAction::AllowSession,
                    ),
                    DecisionOption::new("n", "deny", DecisionAction::Deny),
                ];
                if approval.allows_edit {
                    options.push(DecisionOption::new("e", "edit", DecisionAction::Edit));
                }
                options.push(DecisionOption::new(
                    "esc",
                    "deny and stop",
                    DecisionAction::DenyAndStop,
                ));
                options
            }
            DecisionKind::Question(set) => {
                let mut options = Vec::new();
                let count = set.active().map_or(0, DecisionQuestion::option_count);
                if count > 0 {
                    let keys = if count == 1 {
                        SharedString::new_static("1")
                    } else {
                        SharedString::from(format!("1–{count}"))
                    };
                    options.push(DecisionOption::new(
                        keys,
                        "choose",
                        DecisionAction::Choose(0),
                    ));
                }
                if set.active().is_some_and(|question| question.multi_select) {
                    options.push(DecisionOption::new(
                        "space",
                        "toggle",
                        DecisionAction::Toggle,
                    ));
                }
                let last = set.cursor + 1 >= set.questions.len();
                options.push(DecisionOption::new(
                    "⏎",
                    if last { "answer" } else { "next" },
                    DecisionAction::Answer,
                ));
                if set.cursor > 0 {
                    options.push(DecisionOption::new(
                        "p",
                        "previous",
                        DecisionAction::Previous,
                    ));
                }
                options
            }
            DecisionKind::PlanReady { .. } => vec![
                DecisionOption::new("y", "implement", DecisionAction::Implement),
                DecisionOption::new("n", "refine", DecisionAction::Refine),
            ],
        }
    }

    /// The drawer's keys, for the status bar to mirror while the drawer is open.
    #[must_use]
    pub fn key_hints(&self) -> KeyHintRow {
        self.options()
            .into_iter()
            .fold(KeyHintRow::new(), |row, option| {
                row.hint(KeyHint::labeled(option.key, option.label))
            })
    }

    /// Resolves one bare keystroke to what this decision does, or `None` when it claims nothing.
    ///
    /// **`⏎` is not bound on an approval.** A queued Return keystroke must never approve a
    /// shell command; that is the one property worth keeping exactly. While a reply is in
    /// flight nothing is claimed at all.
    #[must_use]
    pub fn action_for_key(&self, key: &str) -> Option<DecisionAction> {
        if self.answering {
            return None;
        }
        let key = normalize_key(key);
        if let DecisionKind::Question(set) = &self.kind
            && let Some(digit) = single_digit(&key)
        {
            let index = digit.checked_sub(1)?;
            let count = set.active().map_or(0, DecisionQuestion::option_count);
            // A `3` on a two-option question is ignored, never stored as an answer no label
            // matches.
            return (index < count).then_some(DecisionAction::Choose(index));
        }
        self.options()
            .into_iter()
            .find(|option| normalize_key(&option.key) == key)
            .map(|option| option.action)
    }
}

/// `1` from `"1"`, and nothing from `"12"` or `"a"`.
fn single_digit(key: &str) -> Option<usize> {
    let mut chars = key.chars();
    let digit = chars.next()?.to_digit(10)?;
    chars.next().is_none().then_some(digit as usize)
}

/// Normalize a keycap or a gpui key name to one comparable spelling.
///
/// The canvas writes `⏎` and `esc`; gpui reports `enter` and `escape`. Both have to answer the
/// same decision, so both collapse here rather than at four call sites.
fn normalize_key(key: &str) -> String {
    let key = key.trim().trim_start_matches('[').trim_end_matches(']');
    let key = key.trim().to_lowercase();
    match key.as_str() {
        "⏎" | "↵" | "enter" | "return" => "enter".to_owned(),
        "esc" | "escape" => "escape".to_owned(),
        "␣" | "space" => "space".to_owned(),
        _ => key,
    }
}

#[cfg(test)]
mod tests;
