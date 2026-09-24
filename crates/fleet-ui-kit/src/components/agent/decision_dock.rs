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
//! Every option is a real control (ADR 0023): an approval's and a plan's verbs are
//! [`Button`]s, a question's options are clickable rows. Each shows its key as a [`Kbd`] chip the
//! owner resolves from the live keymap ([`DecisionDock::kbd_for`]), and a click reports the very
//! [`DecisionAction`] the key resolves to, so the pointer and the keyboard share one path.

use std::rc::Rc;

use gpui::{AnyElement, App, FocusHandle, SharedString, Window, div, prelude::*};

use super::{
    decision::{Decision, DecisionAction, DecisionKind, DecisionQuestion, QuestionSet},
    format::format_counter,
    metrics::{AGENT_CONTENT_W, AGENT_WELL_MAX_H},
};
use crate::{
    components::{Button, ButtonStyle, Kbd, KbdSize, markdown},
    harness::HarnessTargetExt as _,
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// What the dock hands its owner when a decision control is pressed.
type ActionHandler = Rc<dyn Fn(DecisionAction, &mut Window, &mut App) + 'static>;

/// How the dock learns the key chip of one action: the owner's live-keymap lookup.
type KbdResolver = Rc<dyn Fn(&DecisionAction, &Window, &App) -> Option<Kbd> + 'static>;

/// The docked decision drawer.
#[derive(IntoElement)]
pub struct DecisionDock {
    decision: Decision,
    diff: Option<AnyElement>,
    diff_header: Option<SharedString>,
    payload_focus: Option<FocusHandle>,
    on_action: Option<ActionHandler>,
    kbd_for: Option<KbdResolver>,
}

impl DecisionDock {
    /// The drawer for one pending decision — the head of the queue.
    #[must_use]
    pub fn new(decision: Decision) -> Self {
        Self {
            decision,
            diff: None,
            diff_header: None,
            payload_focus: None,
            on_action: None,
            kbd_for: None,
        }
    }

    /// The diff of an edit or patch approval, joined by item id and rendered by the owner's
    /// `DiffView` at a bounded height. This is what t3code cannot show.
    #[must_use]
    pub fn diff(mut self, diff: impl IntoElement) -> Self {
        self.diff = Some(diff.into_any_element());
        self
    }

    /// The one-line header over the diff — the file it changes, and its size.
    #[must_use]
    pub fn diff_header(mut self, header: impl Into<SharedString>) -> Self {
        self.diff_header = Some(header.into());
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
    /// focus handle — and a click on a control dispatch.
    #[must_use]
    pub fn on_action(
        mut self,
        on_action: impl Fn(DecisionAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }

    /// The key chip each control shows, looked up in the live keymap by the owner, which knows
    /// which app action each [`DecisionAction`] is bound as. A control whose action resolves to
    /// nothing shows no chip. Never a hand-typed key.
    #[must_use]
    pub fn kbd_for(
        mut self,
        kbd_for: impl Fn(&DecisionAction, &Window, &App) -> Option<Kbd> + 'static,
    ) -> Self {
        self.kbd_for = Some(Rc::new(kbd_for));
        self
    }
}

impl RenderOnce for DecisionDock {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let decision = self.decision;
        // While a reply is in flight nothing dispatches: the controls are drawn disabled.
        let on_action = self.on_action.filter(|_| !decision.answering);
        let kbd = |action: &DecisionAction| {
            self.kbd_for
                .as_ref()
                .and_then(|resolve| resolve(action, window, cx))
        };

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
                    .border(theme.metrics.hairline)
                    .border_color(theme.colors.border)
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
                let diff = self.diff.map(|diff| {
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .rounded(theme.radii.sm)
                        .overflow_hidden()
                        .bg(theme.colors.bg)
                        .border(theme.metrics.hairline)
                        .border_color(theme.colors.border)
                        .children(self.diff_header.map(|header| {
                            div()
                                .px(theme.space.md)
                                .py(theme.space.xs)
                                .border_b(theme.metrics.hairline)
                                .border_color(theme.colors.border)
                                .child(Text::data_small(header).faint().ellipsize())
                        }))
                        .child(diff)
                });
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
                    .children(diff)
                    .into_any_element()
            }
            DecisionKind::Question(set) => question_body(set, &theme, on_action.clone(), &kbd),
            DecisionKind::PlanReady {
                title,
                markdown: body,
            } => div()
                .flex()
                .flex_col()
                .gap(theme.space.sm)
                .child(Text::ui_strong(title.clone()).ellipsize())
                .children(body.as_ref().map(|body| markdown(body, cx)))
                .into_any_element(),
        };

        let controls = control_row(&decision, &theme, on_action, &kbd);

        div()
            .w_full()
            .max_w(AGENT_CONTENT_W)
            .relative()
            // The seam: rounded on top only, bordered on three sides, overlapping the composer
            // by the hairline the two share.
            .rounded_t(theme.radii.lg)
            .border_t(theme.metrics.hairline)
            .border_l(theme.metrics.hairline)
            .border_r(theme.metrics.hairline)
            .border_color(theme.colors.border_strong)
            .bg(theme.colors.surface)
            .mb(-theme.metrics.hairline)
            .flex()
            .items_stretch()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(theme.space.md)
                    .px(theme.space.lg)
                    .flex()
                    .flex_col()
                    .gap(theme.space.md)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space.sm)
                            .child(
                                kind_icon(&decision.kind)
                                    .el()
                                    .size(IconSize::Medium)
                                    .tone(Tone::Warning),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Text::ui_strong(decision.title.clone()).ellipsize()),
                            )
                            // `1 of N`, and no "approve all": only the head is actionable.
                            .children((decision.queued > 1).then(|| {
                                Text::hint(format_counter(decision.index, decision.queued))
                                    .faint()
                                    .flex_none()
                            })),
                    )
                    .child(body)
                    .child(controls),
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

/// The glyph the header leads with: a lock for a permission, a question mark for a question,
/// a checklist for a ready plan. Always amber: every occupant is something waiting on you.
fn kind_icon(kind: &DecisionKind) -> Icon {
    match kind {
        DecisionKind::Approval(_) => Icon::Lock,
        DecisionKind::Question(_) => Icon::MessageSquareWarning,
        DecisionKind::PlanReady { .. } => Icon::ListChecks,
    }
}

/// The button row: the decision's verbs in canvas order, the first one primary, and a
/// destructive stop set apart at the far right.
///
/// A question's `Choose` and `Toggle` are not buttons: the option rows are the click target for
/// choosing, and a `1–N` key names a range, not one action.
fn control_row(
    decision: &Decision,
    theme: &Theme,
    on_action: Option<ActionHandler>,
    kbd: &dyn Fn(&DecisionAction) -> Option<Kbd>,
) -> AnyElement {
    let mut leading: Vec<AnyElement> = Vec::new();
    let mut trailing: Vec<AnyElement> = Vec::new();
    let answering = decision.answering;
    let mut primary_taken = false;
    for (ix, option) in decision.options().into_iter().enumerate() {
        if matches!(
            option.action,
            DecisionAction::Choose(_) | DecisionAction::Toggle
        ) {
            continue;
        }
        let stop = option.action == DecisionAction::DenyAndStop;
        let style = if stop {
            ButtonStyle::GhostDanger
        } else if !primary_taken {
            primary_taken = true;
            ButtonStyle::Primary
        } else {
            ButtonStyle::Secondary
        };
        let target = option.action.harness_target();
        let button = Button::new(("decision-button", ix), option.label.clone())
            .style(style)
            .disabled(answering)
            .when_some(kbd(&option.action), Button::kbd)
            .when_some(on_action.clone(), |button, dispatch| {
                let action = option.action.clone();
                button.on_click(move |_, window, cx| dispatch(action.clone(), window, cx))
            })
            // The approval controls are the part of the drawer a scenario clicks, so they are
            // the part `docs/TESTING-HARNESS.md` §3 gives frozen names to.
            .harness_target_named(target)
            .into_any_element();
        if stop {
            trailing.push(button);
        } else {
            leading.push(button);
        }
    }
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.space.sm)
        .children(leading)
        .children(answering.then(|| Text::hint(SharedString::new_static("answering…")).faint()))
        .child(div().flex_1())
        .children(trailing)
        .into_any_element()
}

/// The question panel: header-as-disclosure, `i+1 of n`, numbered option rows.
fn question_body(
    set: &QuestionSet,
    theme: &Theme,
    on_action: Option<ActionHandler>,
    kbd: &dyn Fn(&DecisionAction) -> Option<Kbd>,
) -> AnyElement {
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
        .child(
            div()
                .flex()
                .flex_col()
                .children(option_rows(question, set, theme, on_action, kbd)),
        )
        .into_any_element()
}

/// The numbered option rows of the question under the cursor.
///
/// Each row is a click target that reports `Choose(n)`, the same action its digit key makes;
/// on a multi-select question that toggles the option, and the row shows a checkbox.
fn option_rows(
    question: &DecisionQuestion,
    set: &QuestionSet,
    theme: &Theme,
    on_action: Option<ActionHandler>,
    kbd: &dyn Fn(&DecisionAction) -> Option<Kbd>,
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
    let multi = question.multi_select;
    let hover = theme.colors.row_hover;
    labels
        .into_iter()
        .enumerate()
        .map(|(ox, (label, description))| {
            let dispatch = on_action.clone();
            // Selection is a background, never blue: blue is focus, never state.
            let selected = set.is_selected(cursor, ox);
            let chip = kbd(&DecisionAction::Choose(ox))
                .map(|kbd| kbd.size(KbdSize::Small).into_any_element())
                .unwrap_or_else(|| {
                    Text::hint(SharedString::from(format!("{}", ox + 1)))
                        .faint()
                        .into_any_element()
                });
            let check = multi.then(|| {
                if selected {
                    Icon::SquareCheck
                } else {
                    Icon::Square
                }
                .el()
                .size(IconSize::Small)
                .tone(if selected { Tone::Default } else { Tone::Muted })
            });
            div()
                .id(("decision-option", ox))
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .min_h(theme.metrics.row_h)
                .px(theme.space.sm)
                .rounded(theme.radii.sm)
                .when(selected, |el| el.bg(theme.colors.row_selected))
                .when_some(dispatch, |el, dispatch| {
                    el.cursor_pointer()
                        .when(!selected, |el| el.hover(move |style| style.bg(hover)))
                        .on_click(move |_, window, cx| {
                            dispatch(DecisionAction::Choose(ox), window, cx);
                        })
                })
                .child(div().flex_none().child(chip))
                .children(check)
                .child(Text::ui(label).flex_none())
                .children(description.map(|description| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Text::hint(description).faint().ellipsize())
                }))
                .into_any_element()
        })
        .collect()
}
