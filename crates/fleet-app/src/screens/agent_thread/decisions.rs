//! Decision-card presentation and key routing (`docs/NATIVE-AGENTS.md` §2, §9).
//!
//! A gate is projected into the kit's domain-neutral [`DecisionCard`], and the bare keys the
//! `Agent > AgentDecision > *` contexts dispatch are turned back into a provider-neutral
//! [`GateAnswer`]. Both directions are pure so the whole routing table is testable.

use fleet_core::agents::{
    AgentKind, GateAnswer, GateKind, OpenGate, PermissionChoice, PlanAnswer, ToolKind,
};
use fleet_ui_kit::{
    DecisionAction, DecisionCard, DecisionCardKind, DecisionOption, DecisionQuestion,
    question_actions,
};
use gpui::SharedString;

/// The key context a gate owns while it is open.
#[must_use]
pub(crate) const fn decision_context(gate: &OpenGate) -> &'static str {
    match gate.kind {
        GateKind::Permission { .. } => "AgentPermission",
        GateKind::Question { .. } => "AgentQuestion",
        GateKind::Plan { .. } => "AgentPlan",
    }
}

/// The user's in-progress answer to a question gate.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct QuestionSelection {
    /// Selected option indexes, one set per question, in question order.
    chosen: Vec<Vec<usize>>,
    /// The question the bare keys currently address.
    cursor: usize,
    /// The option `space` toggles.
    highlight: usize,
}

impl QuestionSelection {
    /// An empty selection sized for `questions`.
    pub(crate) fn new(questions: usize) -> Self {
        Self {
            chosen: vec![Vec::new(); questions],
            cursor: 0,
            highlight: 0,
        }
    }

    /// A selection sized for `questions` whose keys address question `cursor`.
    ///
    /// The status bar mirrors the open card's keys, and it rebuilds the card from the app state
    /// rather than from the view's live selection, so it needs the cursor without the choices.
    pub(crate) fn at(questions: usize, cursor: usize) -> Self {
        let mut selection = Self::new(questions);
        selection.cursor = cursor.min(questions.saturating_sub(1));
        selection
    }

    /// The question bare keys address.
    pub(crate) const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Selected indexes for one question.
    pub(crate) fn selected(&self, question: usize) -> &[usize] {
        self.chosen.get(question).map_or(&[], Vec::as_slice)
    }

    /// Whether every question carries at least one answer.
    pub(crate) fn complete(&self) -> bool {
        !self.chosen.is_empty() && self.chosen.iter().all(|answers| !answers.is_empty())
    }

    /// The index the free-text option occupies for one question, when it offers one.
    fn other_index(question: &fleet_core::agents::Question) -> Option<usize> {
        question.allow_other.then_some(question.options.len())
    }

    /// Whether the question the keys address is currently answered with "Something else…".
    ///
    /// §3.2 keys that option to free text the composer carries, so choosing it hands the
    /// keyboard back: the answer may contain a space, and `space` is a card key.
    pub(crate) fn wants_free_text(&self, gate: &OpenGate) -> bool {
        let GateKind::Question { questions } = &gate.kind else {
            return false;
        };
        let Some(question) = questions.get(self.cursor) else {
            return false;
        };
        Self::other_index(question).is_some_and(|other| self.selected(self.cursor).contains(&other))
    }

    /// `1`–`4`: pick an option, toggling instead of replacing when the question is multi-select.
    pub(crate) fn choose(&mut self, option: usize, multi_select: bool) {
        let cursor = self.cursor;
        let Some(answers) = self.chosen.get_mut(cursor) else {
            return;
        };
        self.highlight = option;
        if multi_select {
            if let Some(position) = answers.iter().position(|chosen| *chosen == option) {
                answers.remove(position);
            } else {
                answers.push(option);
            }
            return;
        }
        answers.clear();
        answers.push(option);
        self.advance();
    }

    /// `space`: toggle the highlighted option of a multi-select question.
    pub(crate) fn toggle(&mut self, multi_select: bool) {
        if multi_select {
            let highlight = self.highlight;
            self.choose(highlight, true);
        }
    }

    /// Moves to the next unanswered question, staying on the last one.
    fn advance(&mut self) {
        if self.cursor + 1 < self.chosen.len() {
            self.cursor += 1;
            self.highlight = 0;
        }
    }

    /// The provider answer, one label array per question in question order.
    ///
    /// §3.2 keys the "other" option to free text, which the composer carries. A question whose
    /// only selection is that option and has no text is *not* answered: returning `None` keeps
    /// the card open instead of sending an empty array the user was shown as a choice.
    fn answers(&self, gate: &OpenGate, other: Option<&str>) -> Option<Vec<Vec<String>>> {
        let GateKind::Question { questions } = &gate.kind else {
            return None;
        };
        let mut answers = Vec::with_capacity(questions.len());
        for (index, question) in questions.iter().enumerate() {
            let mut labels = Vec::new();
            for option in self.selected(index) {
                if let Some(choice) = question.options.get(*option) {
                    labels.push(choice.label.clone());
                } else if Self::other_index(question) == Some(*option) {
                    labels.push(other?.to_owned());
                }
            }
            if labels.is_empty() {
                return None;
            }
            answers.push(labels);
        }
        Some(answers)
    }
}

/// One bare key pressed while a decision card owns the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecisionKey {
    /// `y` on a permission.
    AllowOnce,
    /// `a` on a permission.
    AllowSession,
    /// `n` on a permission.
    Deny,
    /// `esc` on a permission.
    DenyAndStop,
    /// `1`–`4` on a question.
    Choose(usize),
    /// `space` on a multi-select question.
    Toggle,
    /// `⏎` on a question.
    Answer,
    /// `y` on a plan.
    ApprovePlan,
    /// `n` on a plan.
    AskChanges,
    /// `⏎` on a plan: show the whole proposal, which answers nothing.
    ViewPlan,
}

/// Turns one decision key into the answer the daemon receives, when the key completes a gate.
///
/// `edited` carries the composer text after `e`, and `note` the feedback typed for a plan; both
/// are ignored by the gates they do not belong to. `None` means the key changed local selection
/// only — a multi-select toggle, or an incomplete question — and nothing is dispatched yet.
pub(crate) fn answer_for(
    gate: &OpenGate,
    key: DecisionKey,
    selection: &mut QuestionSelection,
    edited: Option<String>,
) -> Option<GateAnswer> {
    match (&gate.kind, key) {
        (GateKind::Permission { options, .. }, key) => {
            let choice = match key {
                DecisionKey::AllowOnce => PermissionChoice::AllowOnce,
                DecisionKey::AllowSession => session_choice(gate),
                DecisionKey::Deny => PermissionChoice::Deny,
                DecisionKey::DenyAndStop => PermissionChoice::DenyAndStop,
                _ => return None,
            };
            // A provider that does not offer the scope keyed here falls back to the narrow one,
            // so `a` can never widen a grant the adapter cannot actually map.
            let choice = if matches!(choice, PermissionChoice::AllowOnce)
                || options.iter().any(|option| option.label == choice)
            {
                choice
            } else {
                PermissionChoice::AllowOnce
            };
            Some(GateAnswer::Permission {
                choice,
                edited_payload: edited,
            })
        }
        (GateKind::Question { questions }, key) => {
            let cursor = questions.get(selection.cursor());
            let multi = cursor.is_some_and(|question| question.multi_select);
            // DESIGN-SYSTEM §4: an invalid command is not listed, and `card` only advertises
            // the digits a question actually has. `1`-`4` are bound unconditionally, so a `3`
            // on a two-option question would otherwise store an index no label answers and
            // wedge the card: every later `⏎` would find an unresolvable selection.
            let options = cursor.map_or(0, |question| {
                question.options.len() + usize::from(question.allow_other)
            });
            match key {
                DecisionKey::Choose(option) if option < options => {
                    selection.choose(option, multi);
                    None
                }
                DecisionKey::Choose(_) => None,
                DecisionKey::Toggle => {
                    selection.toggle(multi);
                    None
                }
                DecisionKey::Answer => selection
                    .complete()
                    .then(|| selection.answers(gate, edited.as_deref()))
                    .flatten()
                    .map(|answers| GateAnswer::Question { answers }),
                _ => None,
            }
        }
        (GateKind::Plan { .. }, DecisionKey::ApprovePlan) => {
            Some(GateAnswer::Plan(PlanAnswer::Approve))
        }
        (GateKind::Plan { .. }, DecisionKey::AskChanges) => {
            Some(GateAnswer::Plan(PlanAnswer::AskForChanges {
                note: edited.unwrap_or_default(),
            }))
        }
        // Viewing the plan is a local expansion, not an answer: the gate stays open.
        (GateKind::Plan { .. }, DecisionKey::ViewPlan) => None,
        _ => None,
    }
}

/// The wider grant `a` means for this provider, which OpenCode stores per directory (§4.2).
fn session_choice(gate: &OpenGate) -> PermissionChoice {
    let GateKind::Permission { options, .. } = &gate.kind else {
        return PermissionChoice::AllowSession;
    };
    if options
        .iter()
        .any(|option| option.label == PermissionChoice::AllowDirectory)
    {
        PermissionChoice::AllowDirectory
    } else {
        PermissionChoice::AllowSession
    }
}

/// The card one open gate presents, with the exact §9 keys for this provider.
#[must_use]
pub(crate) fn card(
    gate: &OpenGate,
    provider: AgentKind,
    selection: &QuestionSelection,
    expanded: bool,
) -> DecisionCard {
    let id = SharedString::from(gate.id.to_string());
    match &gate.kind {
        GateKind::Permission {
            tool,
            title,
            payload,
            rationale,
            options,
        } => {
            let mut actions = vec![option("y", "allow once", DecisionAction::AllowOnce)];
            // DESIGN-SYSTEM §6.6: "an action always spells out its effective scope", and §4:
            // "an invalid command is **not listed**". The Claude adapter withholds the session
            // grant for a request the CLI says needs a human, and `answer_for` then silently
            // narrows `a` back to `allow once` — a card promising a session-wide grant the wire
            // cannot carry, after which the same tool asks again.
            if options.iter().any(|provider| {
                matches!(
                    provider.label,
                    PermissionChoice::AllowSession | PermissionChoice::AllowDirectory
                )
            }) {
                actions.push(match session_choice(gate) {
                    PermissionChoice::AllowDirectory => option(
                        "a",
                        "allow for this directory",
                        DecisionAction::AllowDirectory,
                    ),
                    _ => option("a", "allow for this session", DecisionAction::AllowSession),
                });
            }
            actions.push(option("n", "deny", DecisionAction::Deny));
            // §7: a command the provider cannot accept a correction for does not advertise one.
            if options
                .iter()
                .any(|provider| provider.label == PermissionChoice::Edit)
            {
                actions.push(option("e", "edit the command", DecisionAction::Edit));
            }
            actions.push(option("esc", "deny and stop", DecisionAction::DenyAndStop));
            DecisionCard {
                id,
                cursor: 0,
                selected: Vec::new(),
                title: SharedString::new(title.as_str()),
                kind: DecisionCardKind::Permission {
                    tool: SharedString::from(tool_word(tool)),
                    payload: SharedString::new(payload.as_str()),
                    rationale: rationale
                        .as_deref()
                        .filter(|rationale| adds_to(rationale, title, payload))
                        .map(SharedString::new),
                },
                expanded,
                actions,
            }
        }
        GateKind::Question { questions } => {
            let cursor = selection.cursor();
            let multi = questions
                .get(cursor)
                .is_some_and(|question| question.multi_select);
            // DESIGN-SYSTEM §4: an invalid command is not listed. A two-option question does
            // not advertise `3`, `4`, and a single-select one does not advertise `space`.
            let options = questions.get(cursor).map_or(0, |question| {
                question.options.len() + usize::from(question.allow_other)
            });
            let actions = question_actions(options, multi);
            DecisionCard {
                id,
                expanded,
                // The kit bounds its own digit routing against the question the keys address,
                // which is the same one `question_actions` advertised the range for.
                cursor,
                // The card is rebuilt on every key, so the choice travels with it (§3).
                selected: (0..questions.len())
                    .map(|question| selection.selected(question).to_vec())
                    .collect(),
                title: SharedString::from(format!("{} asks", provider.executable())),
                kind: DecisionCardKind::Question {
                    questions: questions
                        .iter()
                        .map(|question| DecisionQuestion {
                            header: SharedString::new(question.header.as_str()),
                            text: SharedString::new(question.text.as_str()),
                            options: question
                                .options
                                .iter()
                                .map(|choice| SharedString::new(choice.label.as_str()))
                                .collect(),
                            multi_select: question.multi_select,
                            allow_other: question.allow_other,
                        })
                        .collect(),
                },
                actions,
            }
        }
        GateKind::Plan { markdown, steps } => DecisionCard {
            id,
            expanded,
            cursor: 0,
            selected: Vec::new(),
            title: SharedString::new_static("plan"),
            kind: DecisionCardKind::Plan {
                markdown: SharedString::new(markdown.as_str()),
                steps: steps
                    .iter()
                    .map(|step| SharedString::new(step.as_str()))
                    .collect(),
            },
            actions: vec![
                option("y", "approve and build", DecisionAction::ApprovePlan),
                option("n", "ask for changes", DecisionAction::AskForChanges),
                option("\u{23ce}", "view full plan", DecisionAction::ViewPlan),
            ],
        },
    }
}

/// One keycap action of a card.
fn option(key: &'static str, label: &'static str, action: DecisionAction) -> DecisionOption {
    DecisionOption {
        key: SharedString::new_static(key),
        label: SharedString::new_static(label),
        action,
    }
}

/// Whether a provider rationale is worth a line of its own on a permission card.
///
/// §2's card is title + payload + keys; a rationale earns the fourth line only by *adding*
/// something. Claude's `description` is often the bare file name the payload already spells out
/// in full (r2-04 stated `/…/smoke-7a8f27.txt` in the payload and `smoke-7a8f27.txt` under it),
/// and a card that says the same thing three times reads as three separate facts.
fn adds_to(rationale: &str, title: &str, payload: &str) -> bool {
    let rationale = rationale.trim();
    !rationale.is_empty()
        && !title.contains(rationale)
        && !payload.lines().any(|line| line.trim().contains(rationale))
}

/// The kind column word a permission card shows for its tool.
fn tool_word(kind: &ToolKind) -> String {
    match kind {
        ToolKind::Read => "read".to_owned(),
        ToolKind::Edit => "edit".to_owned(),
        ToolKind::Write => "write".to_owned(),
        ToolKind::Bash => "bash".to_owned(),
        ToolKind::Search => "search".to_owned(),
        ToolKind::Grep => "grep".to_owned(),
        ToolKind::Fetch => "fetch".to_owned(),
        ToolKind::Agent => "agent".to_owned(),
        ToolKind::Todo => "todo".to_owned(),
        ToolKind::Skill => "skill".to_owned(),
        ToolKind::Mcp { server } => format!("mcp:{server}"),
        ToolKind::Unknown { name } => name.clone(),
    }
}
