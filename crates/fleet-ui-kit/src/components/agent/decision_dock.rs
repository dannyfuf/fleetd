//! `DecisionDock` — the drawer docked to the top edge of the composer.
//!
//! §6.1 of `docs/NATIVE-AGENTS.md`, and it reverses an earlier revision on purpose: **an
//! approval or a question renders in a drawer docked to the composer, never as a transcript
//! card and never as a modal.** A card can be scrolled off screen while it owns the keyboard,
//! which is a modal with the chrome removed; the dock is always on screen by construction, and
//! it lets the transcript scroll freely behind the decision so the reader can read the twelve
//! rows that make the command make sense.
//!
//! The **attachment seam** is what makes it a drawer rather than a stacked toast: the dock
//! rounds only its top corners, overlaps the composer by one hairline, and paints a
//! surface-coloured strip over the border the two share. Nothing separates them, so they read
//! as one panel.
//!
//! It draws no buttons: every affordance is a [`KeyHint`], and the same
//! [`Decision::key_hints`] the status bar mirrors.

use std::rc::Rc;

use gpui::{AnyElement, App, FocusHandle, SharedString, Window, div, prelude::*};

use super::{
    decision::{Decision, DecisionAction, DecisionKind, DecisionQuestion, QuestionSet},
    format::format_counter,
    metrics::{AGENT_CONTENT_W, AGENT_WELL_MAX_H},
};
use crate::{
    components::{KeyHint, markdown},
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// What the dock hands its owner when a decision row is answered.
type ActionHandler = Rc<dyn Fn(DecisionAction, &mut Window, &mut App) + 'static>;

/// The docked decision drawer.
#[derive(IntoElement)]
pub struct DecisionDock {
    decision: Decision,
    diff: Option<AnyElement>,
    payload_focus: Option<FocusHandle>,
    on_action: Option<ActionHandler>,
}

impl DecisionDock {
    /// The drawer for one pending decision — the head of the queue.
    #[must_use]
    pub fn new(decision: Decision) -> Self {
        Self {
            decision,
            diff: None,
            payload_focus: None,
            on_action: None,
        }
    }

    /// The diff of an edit or patch approval, joined by item id and rendered by the owner's
    /// `DiffView` at a bounded height. This is what t3code cannot show.
    #[must_use]
    pub fn diff(mut self, diff: impl IntoElement) -> Self {
        self.diff = Some(diff.into_any_element());
        self
    }

    /// The focus handle of the payload well, so a keyboard user can scroll a long invocation.
    ///
    /// The composer keeps the tab's focus handle in every mode, including while an approval is
    /// open; this is the one exception, and it is opt-in.
    #[must_use]
    pub fn payload_focus(mut self, focus: FocusHandle) -> Self {
        self.payload_focus = Some(focus);
        self
    }

    /// What a key — resolved through [`Decision::action_for_key`] by the surface that owns the
    /// focus handle — and a click on a hint dispatch.
    #[must_use]
    pub fn on_action(
        mut self,
        on_action: impl Fn(DecisionAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }
}

impl RenderOnce for DecisionDock {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let decision = self.decision;
        let on_action = self.on_action;

        let body = match &decision.kind {
            DecisionKind::Approval(approval) => {
                let well = div()
                    .id("decision-payload")
                    .w_full()
                    .max_h(AGENT_WELL_MAX_H)
                    // Never truncated and never line-clamped: it is the invocation, and it is
                    // what `[e]` seeds the composer with. It scrolls in both axes instead.
                    .overflow_x_scroll()
                    .overflow_y_scroll()
                    .whitespace_nowrap()
                    .px(theme.space.md)
                    .py(theme.space.sm)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .child(Text::data_small(approval.payload.clone()))
                    .children(
                        approval
                            .rationale
                            .clone()
                            .map(|rationale| Text::hint(rationale).faint()),
                    );
                let well = match self.payload_focus {
                    Some(focus) => well.track_focus(&focus).into_any_element(),
                    None => well.into_any_element(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space.sm)
                    .child(well)
                    // A caution rides as a one-line `⚠` prefix, never as a second line.
                    .children(approval.caution.clone().map(|caution| {
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space.xs)
                            .child(
                                Icon::TriangleAlert
                                    .el()
                                    .size(IconSize::Small)
                                    .tone(Tone::Warning),
                            )
                            .child(Text::hint(caution).tone(Tone::Warning).ellipsize())
                    }))
                    .children(self.diff)
                    .into_any_element()
            }
            DecisionKind::Question(set) => question_body(set, &theme, on_action.clone()),
            DecisionKind::PlanReady {
                title,
                markdown: body,
            } => div()
                .flex()
                .flex_col()
                .gap(theme.space.sm)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.sm)
                        .child(Text::ui(SharedString::new_static("plan ready")).muted())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Text::ui_strong(title.clone()).ellipsize()),
                        ),
                )
                .children(body.as_ref().map(|body| markdown(body, cx)))
                .into_any_element(),
        };

        let hints = decision
            .options()
            .into_iter()
            .enumerate()
            .map(|(ix, option)| {
                // A `1–N choose` hint names a *range* of keys, not one action: clicking it would
                // answer the model's question with option 1, which nobody picked. The option rows
                // are the click target for choosing.
                let dispatch = on_action
                    .clone()
                    .filter(|_| !matches!(option.action, DecisionAction::Choose(_)))
                    .filter(|_| !decision.answering);
                let action = option.action.clone();
                div()
                    .id(("decision-hint", ix))
                    .flex_none()
                    .when_some(dispatch, |el, dispatch| {
                        let action = action.clone();
                        el.cursor_pointer()
                            .on_click(move |_, window, cx| dispatch(action.clone(), window, cx))
                    })
                    .child(KeyHint::labeled(option.key, option.label))
            });

        div()
            .w_full()
            .max_w(AGENT_CONTENT_W)
            .relative()
            // The seam: rounded on top only, bordered on three sides, overlapping the composer
            // by the hairline the two share.
            .rounded_t(theme.radii.md)
            .border_t(theme.metrics.hairline)
            .border_l(theme.metrics.hairline)
            .border_r(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .bg(theme.colors.surface)
            .mb(-theme.metrics.hairline)
            .flex()
            .items_stretch()
            .child(
                // Flush left and outside the padding: attention is a static bar, never an
                // animation, and the 2 px width is the one the cursor bar uses.
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space.sm)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Text::ui_strong(decision.title.clone()).ellipsize()),
                            )
                            // `1/N`, and no "approve all": only the head is actionable.
                            .children((decision.queued > 1).then(|| {
                                Text::hint(format_counter(decision.index, decision.queued))
                                    .faint()
                                    .flex_none()
                            })),
                    )
                    .child(body)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(theme.space.sm)
                            .when(decision.answering, |el| {
                                el.opacity(theme.metrics.dimmed_opacity)
                            })
                            .children(hints)
                            .children(decision.answering.then(|| {
                                Text::hint(SharedString::new_static("answering…")).faint()
                            })),
                    ),
            )
            // The mask: one surface-coloured hairline over the border the dock and the composer
            // share, so the two read as one panel rather than as a card stacked on a field.
            .child(
                div()
                    .absolute()
                    .bottom(-theme.metrics.hairline)
                    .left(theme.metrics.hairline)
                    .right(theme.metrics.hairline)
                    .h(theme.metrics.hairline)
                    .bg(theme.colors.surface),
            )
    }
}

/// The question panel: header-as-disclosure, `i+1/n`, numbered option rows.
fn question_body(set: &QuestionSet, theme: &Theme, on_action: Option<ActionHandler>) -> AnyElement {
    let Some(question) = set.active() else {
        return div().into_any_element();
    };
    let total = set.questions.len();
    div()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    Icon::MessageSquareWarning
                        .el()
                        .size(IconSize::Small)
                        .tone(Tone::Warning),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Text::label(question.header.clone()).ellipsize()),
                )
                .children((total > 1).then(|| {
                    Text::hint(format_counter(set.cursor, total))
                        .faint()
                        .flex_none()
                })),
        )
        .child(Text::ui(question.prompt.clone()))
        .children(option_rows(question, set, theme, on_action))
        .into_any_element()
}

/// The numbered option rows of the question under the cursor.
fn option_rows(
    question: &DecisionQuestion,
    set: &QuestionSet,
    theme: &Theme,
    on_action: Option<ActionHandler>,
) -> Vec<AnyElement> {
    let mut labels: Vec<(SharedString, Option<SharedString>)> = question
        .options
        .iter()
        .map(|option| (option.label.clone(), option.description.clone()))
        .collect();
    if question.allow_other {
        labels.push((
            SharedString::new_static(super::decision::SOMETHING_ELSE),
            None,
        ));
    }
    labels.truncate(question.option_count());
    let cursor = set.cursor;
    labels
        .into_iter()
        .enumerate()
        .map(|(ox, (label, description))| {
            let dispatch = on_action.clone();
            // Selection is a background, never blue: blue is focus, never state.
            let selected = set.is_selected(cursor, ox);
            div()
                .id(("decision-option", ox))
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .px(theme.space.xs)
                .rounded(theme.radii.sm)
                .when(selected, |el| el.bg(theme.colors.row_selected))
                .when_some(dispatch, |el, dispatch| {
                    el.cursor_pointer().on_click(move |_, window, cx| {
                        dispatch(DecisionAction::Choose(ox), window, cx);
                    })
                })
                .child(
                    Text::hint(SharedString::from(format!("{}", ox + 1)))
                        .faint()
                        .flex_none(),
                )
                .child(Text::ui(label))
                .children(
                    description.map(|description| Text::hint(description).faint().ellipsize()),
                )
                .into_any_element()
        })
        .collect()
}
