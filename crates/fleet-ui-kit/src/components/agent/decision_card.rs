//! Inline permission, question and plan decision cards.
//!
//! §2 of `NATIVE-AGENTS.md`: decisions are cards in the thread, never modals. All three
//! variants share one shape — 760 px, the panel background, radius 6, a 2 px amber bar flush
//! left — and differ only in their body and their keys. The card owns the key *vocabulary*
//! ([`DecisionCard::action_for_key`]); the surface that holds the focus handle owns the key
//! *event* and routes it here, which is what keeps one thread from answering another thread's
//! card.

use std::rc::Rc;

use gpui::{AnyElement, App, ElementId, SharedString, Window, div, prelude::*};

use crate::{
    components::{KeyHint, KeyHintRow, markdown, parse_markdown_document},
    focus::FocusRing,
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

use super::metrics::AGENT_CONTENT_W;

/// One labelled action shown by a decision card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOption {
    /// Key shown in the action hint.
    pub key: SharedString,
    /// User-facing action label.
    pub label: SharedString,
    /// Action dispatched when selected.
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

/// Presentation shape of one provider question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionQuestion {
    /// Short question heading.
    pub header: SharedString,
    /// Full question text.
    pub text: SharedString,
    /// Selectable answer labels.
    pub options: Vec<SharedString>,
    /// Whether several options may be selected.
    pub multi_select: bool,
    /// Whether free text is accepted.
    pub allow_other: bool,
}

/// Presentation-level mirror of the provider-neutral gate variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionCardKind {
    /// A protected tool invocation.
    Permission {
        /// Tool kind label.
        tool: SharedString,
        /// Command, path, or provider payload.
        payload: SharedString,
        /// Optional sanitized rationale.
        rationale: Option<SharedString>,
    },
    /// One or more provider questions.
    Question {
        /// Ordered question presentations.
        questions: Vec<DecisionQuestion>,
    },
    /// A completed plan proposal.
    Plan {
        /// Full Markdown text.
        markdown: SharedString,
        /// Ordered short steps.
        steps: Vec<SharedString>,
    },
}

/// Semantic action emitted by decision-card key routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionAction {
    /// Allow this invocation only.
    AllowOnce,
    /// Allow matching calls for this session.
    AllowSession,
    /// Allow matching calls for this directory.
    AllowDirectory,
    /// Deny while allowing the agent to continue.
    Deny,
    /// Edit the command or provider payload.
    Edit,
    /// Deny and interrupt the turn.
    DenyAndStop,
    /// Choose one zero-based question option.
    Choose(usize),
    /// Toggle a selected option for multi-select.
    Toggle,
    /// Submit question answers.
    Answer,
    /// Approve a plan and proceed.
    ApprovePlan,
    /// Ask the agent to change its plan.
    AskForChanges,
    /// View the complete plan.
    ViewPlan,
}

/// Domain-neutral properties for one inline decision card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionCard {
    /// Stable UI key.
    pub id: SharedString,
    /// Card title.
    pub title: SharedString,
    /// Permission, question, or plan presentation.
    pub kind: DecisionCardKind,
    /// Ordered visible key actions.
    pub actions: Vec<DecisionOption>,
    /// Whether the plan card is showing its full Markdown (`⏎ view full plan`).
    pub expanded: bool,
    /// The chosen option indices of a question card, one entry per question.
    ///
    /// DESIGN-SYSTEM §3: selection is a background. The row that carries this card is rebuilt
    /// from the owner's in-progress answer, so the choice `1`-`4` and `space` make has to
    /// travel with it — otherwise those keys change state the user cannot see.
    pub selected: Vec<Vec<usize>>,
    /// The question the card's bare keys currently address, on a multi-question gate.
    ///
    /// The advertised `1–N` range follows the cursor, so the digits [`DecisionCard::action_for_key`]
    /// honours have to follow it too: bounding them against question 0 refuses a digit the card
    /// itself is offering (KEYMAP: "only the digits a question actually offers are honoured").
    pub cursor: usize,
}

/// The free-text option every provider question that accepts one offers last.
pub const SOMETHING_ELSE: &str = "Something else…";

/// The most numbered options one question may draw, honour and advertise.
///
/// KEYMAP gives `Agent > AgentDecision > AgentQuestion` one binding per digit, so this is the
/// last digit that dispatches: four provider options plus the [`SOMETHING_ELSE`] row every
/// question that accepts free text appends. It bounds the rows the card draws as well as the
/// keys it answers — a row numbered past the last bound digit is an affordance no key can
/// reach, which DESIGN-SYSTEM §4 forbids in the same way as a listed key that does nothing.
pub const MAX_QUESTION_OPTIONS: usize = 5;

/// The §9 permission keys, with the session-or-directory label the provider chose.
///
/// `session` is [`DecisionAction::AllowSession`] or [`DecisionAction::AllowDirectory`]
/// depending on what the provider offered; the label follows it, because "allow for this
/// directory" and "allow for this session" are not the same promise.
#[must_use]
pub fn permission_actions(
    session: DecisionAction,
    session_label: impl Into<SharedString>,
) -> Vec<DecisionOption> {
    vec![
        DecisionOption::new("y", "allow once", DecisionAction::AllowOnce),
        DecisionOption::new("a", session_label, session),
        DecisionOption::new("n", "deny", DecisionAction::Deny),
        DecisionOption::new("e", "edit the command", DecisionAction::Edit),
        DecisionOption::new("esc", "deny and stop", DecisionAction::DenyAndStop),
    ]
}

/// The §9 question keys for a question with `options` choices.
#[must_use]
pub fn question_actions(options: usize, multi_select: bool) -> Vec<DecisionOption> {
    let last = options.min(MAX_QUESTION_OPTIONS);
    let mut actions = Vec::new();
    // DESIGN-SYSTEM §4: an invalid command is not listed. A question with no options at all
    // has no digit to offer, and `action_for_key` refuses every one of them.
    if last > 0 {
        let keys = if last == 1 {
            SharedString::from("1")
        } else {
            SharedString::from(format!("1–{last}"))
        };
        actions.push(DecisionOption::new(
            keys,
            "choose",
            DecisionAction::Choose(0),
        ));
    }
    if multi_select {
        actions.push(DecisionOption::new(
            "space",
            "toggle",
            DecisionAction::Toggle,
        ));
    }
    actions.push(DecisionOption::new("⏎", "answer", DecisionAction::Answer));
    actions
}

/// The §9 plan keys.
#[must_use]
pub fn plan_actions() -> Vec<DecisionOption> {
    vec![
        DecisionOption::new("y", "approve and build", DecisionAction::ApprovePlan),
        DecisionOption::new("n", "ask for changes", DecisionAction::AskForChanges),
        DecisionOption::new("⏎", "view full plan", DecisionAction::ViewPlan),
    ]
}

/// Normalize a keycap or a gpui key name to one comparable spelling.
///
/// The canvas writes `⏎` and `esc`; gpui reports `enter` and `escape`. Both have to answer the
/// same card, so both collapse here rather than at four call sites.
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

impl DecisionCard {
    /// Expand or collapse the full plan Markdown.
    #[must_use]
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// A card with no actions yet.
    #[must_use]
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        kind: DecisionCardKind,
    ) -> Self {
        Self {
            expanded: false,
            id: id.into(),
            title: title.into(),
            kind,
            actions: Vec::new(),
            selected: Vec::new(),
            cursor: 0,
        }
    }

    /// Set the question the bare keys address.
    #[must_use]
    pub const fn cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor;
        self
    }

    /// Set the chosen option indices, one entry per question.
    #[must_use]
    pub fn selected(mut self, selected: Vec<Vec<usize>>) -> Self {
        self.selected = selected;
        self
    }

    /// Set the visible key actions.
    #[must_use]
    pub fn actions(mut self, actions: Vec<DecisionOption>) -> Self {
        self.actions = actions;
        self
    }

    /// How many options the question at `index` offers, or 0 for the other variants.
    #[must_use]
    pub fn option_count(&self, index: usize) -> usize {
        match &self.kind {
            DecisionCardKind::Question { questions } => {
                questions.get(index).map_or(0, |question| {
                    (question.options.len() + usize::from(question.allow_other))
                        .min(MAX_QUESTION_OPTIONS)
                })
            }
            _ => 0,
        }
    }

    /// Resolves one bare keystroke to the action this card performs, or `None` when the card
    /// does not claim that key.
    ///
    /// The declared [`DecisionCard::actions`] win, so a provider that labelled `a` as
    /// "allow for this directory" answers with [`DecisionAction::AllowDirectory`]. The §9
    /// defaults fill in for a card whose action list was not populated, and the digits of a
    /// question are always resolved against the options it actually has.
    #[must_use]
    pub fn action_for_key(&self, key: &str) -> Option<DecisionAction> {
        let key = normalize_key(key);

        if let DecisionCardKind::Question { .. } = self.kind
            && let Some(digit) = key.chars().next().and_then(|c| c.to_digit(10))
            && key.len() == 1
        {
            let index = usize::try_from(digit).ok()?.checked_sub(1)?;
            return (index < self.option_count(self.cursor))
                .then_some(DecisionAction::Choose(index));
        }

        if let Some(option) = self
            .actions
            .iter()
            .find(|option| normalize_key(&option.key) == key)
        {
            return Some(option.action.clone());
        }

        match &self.kind {
            DecisionCardKind::Permission { .. } => match key.as_str() {
                "y" => Some(DecisionAction::AllowOnce),
                "a" => Some(DecisionAction::AllowSession),
                "n" => Some(DecisionAction::Deny),
                "e" => Some(DecisionAction::Edit),
                "escape" => Some(DecisionAction::DenyAndStop),
                _ => None,
            },
            DecisionCardKind::Question { questions } => match key.as_str() {
                "space" => questions
                    .get(self.cursor)
                    .filter(|question| question.multi_select)
                    .map(|_| DecisionAction::Toggle),
                "enter" => Some(DecisionAction::Answer),
                _ => None,
            },
            DecisionCardKind::Plan { .. } => match key.as_str() {
                "y" => Some(DecisionAction::ApprovePlan),
                "n" => Some(DecisionAction::AskForChanges),
                "enter" => Some(DecisionAction::ViewPlan),
                _ => None,
            },
        }
    }
}

/// Renders an inline decision card.
///
/// This is the read-only form. [`DecisionCardElement`] adds the selection state, the free-text
/// answer slot, the expanded plan and the action callback.
pub fn decision_card(card: &DecisionCard, _cx: &App) -> impl IntoElement {
    DecisionCardElement::new(card.clone())
}

/// The interactive decision card.
#[derive(IntoElement)]
pub struct DecisionCardElement {
    card: DecisionCard,
    default_action: usize,
    selected: Vec<Vec<usize>>,
    expanded: bool,
    answer: Option<AnyElement>,
    #[allow(clippy::type_complexity)]
    on_action: Option<Rc<dyn Fn(DecisionAction, &mut Window, &mut App) + 'static>>,
}

impl DecisionCardElement {
    /// A card from its presentation properties.
    #[must_use]
    pub fn new(card: DecisionCard) -> Self {
        Self {
            card,
            default_action: 0,
            selected: Vec::new(),
            expanded: false,
            answer: None,
            on_action: None,
        }
    }

    /// Which action carries the focus token. The first action by default.
    #[must_use]
    pub fn default_action(mut self, index: usize) -> Self {
        self.default_action = index;
        self
    }

    /// The chosen option indices, one entry per question.
    #[must_use]
    pub fn selected(mut self, selected: Vec<Vec<usize>>) -> Self {
        self.selected = selected;
        self
    }

    /// Expand the full plan Markdown.
    #[must_use]
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// The free-text answer field, normally a [`crate::components::TextInput`] entity, shown
    /// under the questions once "Something else…" is chosen.
    #[must_use]
    pub fn answer(mut self, answer: impl IntoElement) -> Self {
        self.answer = Some(answer.into_any_element());
        self
    }

    /// What an action hint dispatches when it is clicked, and what the focus owner calls after
    /// [`DecisionCard::action_for_key`] resolved a key.
    #[must_use]
    pub fn on_action(
        mut self,
        on_action: impl Fn(DecisionAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }
}

impl RenderOnce for DecisionCardElement {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let DecisionCardElement {
            card,
            default_action,
            selected,
            expanded,
            answer,
            on_action,
        } = self;
        let cursor = card.cursor;
        let title = Text::ui_strong(card.title.clone());
        let is_selected = |question: usize, option: usize| {
            selected
                .get(question)
                .is_some_and(|chosen| chosen.contains(&option))
        };

        let body = match &card.kind {
            // §2's card is title + payload + keys. The tool is already in the title
            // (`Claude wants to use Write`) and on the transcript's own tool row above the
            // card, so a third bare `write` line under the payload states the target for the
            // third time and reads as an orphaned row rather than as part of the card.
            DecisionCardKind::Permission {
                tool: _,
                payload,
                rationale,
            } => div()
                .flex()
                .flex_col()
                .gap(theme.space.sm)
                .child(
                    // The dark code well: the payload is the one string that must be read
                    // literally, so it drops to the app ground under the card's surface.
                    div()
                        .w_full()
                        .px(theme.space.md)
                        .py(theme.space.sm)
                        .rounded(theme.radii.sm)
                        .bg(theme.colors.bg)
                        .child(Text::data(payload.clone())),
                )
                .children(
                    rationale
                        .clone()
                        .map(|rationale| Text::ui(rationale).muted()),
                )
                .into_any_element(),
            DecisionCardKind::Question { questions } => div()
                .flex()
                .flex_col()
                .gap(theme.space.md)
                .children(questions.iter().enumerate().map(|(qx, question)| {
                    let mut labels: Vec<SharedString> = question.options.clone();
                    if question.allow_other {
                        labels.push(SharedString::new_static(SOMETHING_ELSE));
                    }
                    // The same ceiling `option_count` and `question_actions` use: a row past
                    // the last bound digit would be numbered by nothing that can select it.
                    labels.truncate(MAX_QUESTION_OPTIONS);
                    let card_id = card.id.clone();
                    let on_action = on_action.clone();
                    // DESIGN-SYSTEM §3: the cursor is a 2 px bar on the leading edge. §9 routes
                    // `1`-`4` and `space` to one question at a time, so on a multi-question gate
                    // the bar is the only thing that says which one they answer.
                    let marked = questions.len() > 1;
                    FocusRing::cursor_row(marked && qx == cursor).content(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .when(marked, |el| el.pl(theme.space.sm))
                            .gap(theme.space.xs)
                            .child(Text::label(question.header.clone()))
                            .child(Text::ui(question.text.clone()))
                            .children(labels.into_iter().enumerate().map(|(ox, label)| {
                                // §2: blue is where you are and never a state. A chosen option is a
                                // selection, which DESIGN-SYSTEM §3 paints as a background.
                                //
                                // §10: row focus does not exist yet, so the click is how a mouse
                                // reaches an option at all. Only the cursor question's rows take it,
                                // because `Choose(n)` is answered against the cursor — exactly like
                                // the `1`-`4` keys the card advertises.
                                let clickable = qx == cursor;
                                let dispatch = on_action.clone();
                                div()
                                    .id(ElementId::from(SharedString::from(format!(
                                        "decision-{card_id}-q{qx}-o{ox}"
                                    ))))
                                    .flex()
                                    .items_center()
                                    .gap(theme.space.sm)
                                    .px(theme.space.xs)
                                    .rounded(theme.radii.sm)
                                    .when(is_selected(qx, ox), |el| {
                                        el.bg(theme.colors.row_selected)
                                    })
                                    .when_some(dispatch.filter(|_| clickable), |el, dispatch| {
                                        el.on_click(move |_, window, cx| {
                                            dispatch(DecisionAction::Choose(ox), window, cx);
                                        })
                                    })
                                    .child(Text::hint(format!("{}", ox + 1)).faint().flex_none())
                                    .child(Text::ui(label))
                            })),
                    )
                }))
                .children(answer)
                .into_any_element(),
            DecisionCardKind::Plan {
                markdown: source,
                steps,
            } => div()
                .flex()
                .flex_col()
                .gap(theme.space.sm)
                .children(steps.iter().enumerate().map(|(ix, step)| {
                    div()
                        .flex()
                        .items_start()
                        .gap(theme.space.sm)
                        .child(Text::hint(format!("{}", ix + 1)).faint().flex_none())
                        .child(Text::ui(step.clone()))
                }))
                .when(expanded, |el| {
                    el.child(markdown(&parse_markdown_document(source), cx))
                })
                .into_any_element(),
        };

        let actions = card.actions.iter().enumerate().map(|(ix, option)| {
            // §2 keeps blue for focus and location. The default action is neither, so it is
            // raised out of the muted set rather than painted as if it were the cursor.
            let hint = KeyHint::labeled(option.key.clone(), option.label.clone()).key_tone(
                if ix == default_action {
                    Tone::Default
                } else {
                    Tone::Muted
                },
            );
            // A `1–N choose` hint names a *range* of keys, not one action: clicking it would
            // answer the model's question with option 1, which the user never picked. The
            // option rows above are the click target for choosing (§10).
            let dispatch = on_action
                .clone()
                .filter(|_| !matches!(option.action, DecisionAction::Choose(_)));
            let action = option.action.clone();
            div()
                .id(ElementId::from(SharedString::from(format!(
                    "decision-{}-{ix}",
                    card.id
                ))))
                .flex_none()
                .when_some(dispatch, |el, dispatch| {
                    let action = action.clone();
                    el.on_click(move |_, window, cx| dispatch(action.clone(), window, cx))
                })
                .child(hint)
        });

        div()
            .w_full()
            .max_w(AGENT_CONTENT_W)
            .flex()
            .rounded(theme.radii.md)
            .overflow_hidden()
            .bg(theme.colors.surface)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(
                // Flush left and outside the padding: §2 makes attention a static bar, never
                // an animation, and the 2 px width is the same one the cursor bar uses.
                div()
                    .flex_none()
                    .w(theme.metrics.focus_ring_w)
                    .bg(theme.colors.warning),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(theme.space.md)
                    .px(theme.space.lg)
                    .flex()
                    .flex_col()
                    .gap(theme.space.sm)
                    .child(title)
                    .child(body)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(theme.space.sm)
                            .children(actions),
                    ),
            )
    }
}

/// A row of the card's keys, for the status bar to mirror while the card is open.
#[must_use]
pub fn decision_key_hints(card: &DecisionCard) -> KeyHintRow {
    card.actions.iter().fold(KeyHintRow::new(), |row, option| {
        row.key(option.key.clone(), option.label.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission() -> DecisionCard {
        DecisionCard::new(
            "g1",
            "claude wants to run a command",
            DecisionCardKind::Permission {
                tool: "bash".into(),
                payload: "rm -rf target".into(),
                rationale: Some("clean the build".into()),
            },
        )
        .actions(permission_actions(
            DecisionAction::AllowDirectory,
            "allow for this directory",
        ))
    }

    fn question(multi_select: bool, allow_other: bool) -> DecisionCard {
        DecisionCard::new(
            "g2",
            "claude has a question",
            DecisionCardKind::Question {
                questions: vec![DecisionQuestion {
                    header: "scope".into(),
                    text: "which files should change?".into(),
                    options: vec!["payroll".into(), "taxes".into(), "both".into()],
                    multi_select,
                    allow_other,
                }],
            },
        )
        .actions(question_actions(3, multi_select))
    }

    fn plan() -> DecisionCard {
        DecisionCard::new(
            "g3",
            "claude proposes a plan",
            DecisionCardKind::Plan {
                markdown: "# plan\n\nstep one".into(),
                steps: vec!["read the reducer".into(), "add the test".into()],
            },
        )
        .actions(plan_actions())
    }

    #[test]
    fn permission_keys_follow_the_option_the_provider_offered() {
        let card = permission();
        assert_eq!(card.action_for_key("y"), Some(DecisionAction::AllowOnce));
        assert_eq!(
            card.action_for_key("a"),
            Some(DecisionAction::AllowDirectory)
        );
        assert_eq!(card.action_for_key("n"), Some(DecisionAction::Deny));
        assert_eq!(card.action_for_key("e"), Some(DecisionAction::Edit));
        assert_eq!(
            card.action_for_key("escape"),
            Some(DecisionAction::DenyAndStop)
        );
        assert_eq!(card.action_for_key("q"), None);
    }

    #[test]
    fn a_permission_card_without_actions_still_answers_the_default_keys() {
        let card = DecisionCard::new(
            "g1",
            "title",
            DecisionCardKind::Permission {
                tool: "bash".into(),
                payload: "ls".into(),
                rationale: None,
            },
        );
        assert_eq!(card.action_for_key("a"), Some(DecisionAction::AllowSession));
        assert_eq!(
            card.action_for_key("esc"),
            Some(DecisionAction::DenyAndStop)
        );
    }

    #[test]
    fn question_digits_are_bounded_by_the_options_that_exist() {
        let card = question(true, false);
        assert_eq!(card.action_for_key("1"), Some(DecisionAction::Choose(0)));
        assert_eq!(card.action_for_key("3"), Some(DecisionAction::Choose(2)));
        assert_eq!(card.action_for_key("4"), None);
        assert_eq!(card.action_for_key("0"), None);
    }

    /// UX-6: the digits follow the cursor question, which is the range the card advertises.
    #[test]
    fn question_digits_are_resolved_against_the_question_the_keys_address() {
        let card = DecisionCard::new(
            "g4",
            "claude has questions",
            DecisionCardKind::Question {
                questions: vec![
                    DecisionQuestion {
                        header: "scope".into(),
                        text: "which files?".into(),
                        options: vec!["payroll".into(), "taxes".into()],
                        multi_select: false,
                        allow_other: false,
                    },
                    DecisionQuestion {
                        header: "depth".into(),
                        text: "how far?".into(),
                        options: vec!["one".into(), "two".into(), "three".into(), "four".into()],
                        multi_select: true,
                        allow_other: false,
                    },
                ],
            },
        );
        // On question 0 the card offers two options and `space` does nothing.
        assert_eq!(card.action_for_key("3"), None);
        assert_eq!(card.action_for_key("space"), None);

        // On question 1 it offers four, and the multi-select toggle is live.
        let card = card.cursor(1);
        assert_eq!(card.option_count(1), 4);
        assert_eq!(card.action_for_key("4"), Some(DecisionAction::Choose(3)));
        assert_eq!(card.action_for_key("space"), Some(DecisionAction::Toggle));
        // …and the hint row the status bar mirrors states the same range.
        assert_eq!(
            question_actions(card.option_count(card.cursor), true)
                .first()
                .map(|option| option.key.to_string()),
            Some("1\u{2013}4".to_owned())
        );
    }

    #[test]
    fn something_else_is_a_choosable_option() {
        let card = question(false, true);
        assert_eq!(card.option_count(0), 4);
        assert_eq!(card.action_for_key("4"), Some(DecisionAction::Choose(3)));
    }

    #[test]
    fn space_toggles_only_a_multi_select_question() {
        assert_eq!(
            question(true, false).action_for_key("space"),
            Some(DecisionAction::Toggle)
        );
        assert_eq!(question(false, false).action_for_key("space"), None);
        assert_eq!(
            question(false, false).action_for_key("⏎"),
            Some(DecisionAction::Answer)
        );
    }

    #[test]
    fn plan_keys_map_to_the_three_plan_actions() {
        let card = plan();
        assert_eq!(card.action_for_key("y"), Some(DecisionAction::ApprovePlan));
        assert_eq!(
            card.action_for_key("n"),
            Some(DecisionAction::AskForChanges)
        );
        assert_eq!(card.action_for_key("enter"), Some(DecisionAction::ViewPlan));
        assert_eq!(card.action_for_key("a"), None);
    }

    #[test]
    fn keycaps_and_gpui_names_normalize_to_the_same_key() {
        assert_eq!(normalize_key("[⏎]"), "enter");
        assert_eq!(normalize_key("esc"), "escape");
        assert_eq!(normalize_key(" Y "), "y");
        assert_eq!(normalize_key("space"), "space");
    }

    /// The advertised digits, the honoured digits and the drawn rows are one set.
    ///
    /// A four-option question that also accepts free text draws five numbered rows, so `5` has
    /// to be advertised and honoured; a question with no options at all advertises no digit.
    #[test]
    fn every_offered_option_is_advertised_and_no_absent_one_is() {
        let card = DecisionCard::new(
            "g5",
            "claude asks",
            DecisionCardKind::Question {
                questions: vec![DecisionQuestion {
                    header: "scope".into(),
                    text: "which crate?".into(),
                    options: vec![
                        "core".into(),
                        "app".into(),
                        "ui-kit".into(),
                        "daemon".into(),
                    ],
                    multi_select: false,
                    allow_other: true,
                }],
            },
        );
        assert_eq!(card.option_count(0), 5);
        assert_eq!(card.action_for_key("5"), Some(DecisionAction::Choose(4)));
        assert_eq!(
            question_actions(card.option_count(0), false)[0].key,
            "1\u{2013}5",
            "the row drawn as `5` has to be advertised as one of the keys"
        );
        assert!(
            !question_actions(0, false)
                .iter()
                .any(|option| matches!(option.action, DecisionAction::Choose(_))),
            "a question with no options must not list a digit `action_for_key` refuses"
        );
    }

    #[test]
    fn the_question_hint_row_states_the_range_that_exists() {
        let actions = question_actions(3, true);
        assert_eq!(actions[0].key, "1–3");
        assert_eq!(actions[1].action, DecisionAction::Toggle);
        assert_eq!(actions[2].action, DecisionAction::Answer);
        assert_eq!(question_actions(1, false)[0].key, "1");
        assert_eq!(question_actions(9, false)[0].key, "1–5");
    }
}
