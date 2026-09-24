//! `ConfirmDialog` — an alert that shows what is lost, sized and keyed by the facts.
//!
//! §1.7 made literal, and §3.8.3's escalation rule [D-10] enforced in one place:
//!
//! * every decisive fact known and benign -> the **compact** form, 480 px, facts on one line,
//!   `y` confirms and `Enter` is accepted;
//! * any risk true, or any decisive fact unknown -> the **expanded** form, 560 px, one line
//!   per fact, amber icon tile, and `Y` is required with `Enter` **not** accepted.
//!
//! Both forms share the alert frame ([`Dialog::alert`]): the icon in a tinted tile, the title,
//! the full target as the subtitle, the facts (risks first in amber with their numbers in bold,
//! safe facts green), the freshness stamp with its re-check button, and the consequence sentence.
//! The footer is two buttons, `Cancel` and the action. How dangerous the action is shows in the
//! action button: a primary `Delete` whose chip is `y` where `y` confirms, a red `Delete anyway`
//! whose chip is `⇧Y` where `Y` is required. The key rule is the facts', never the button's.
//!
//! The freshness stamp is mandatory on every facts confirm: this is the one surface where the
//! age of a fact decides an outcome. There is no "don't ask again", no second confirmation, no
//! countdown and no typed-name confirmation — the compact form *is* the answer to confirm
//! fatigue.
//!
//! The bound keys are exactly `y` / `Y` / `Enter` (confirm), `n` / `Esc` / `q` (cancel), plus
//! the re-check and the prune's KEEP toggle. Nothing else is bound, so muscle memory cannot
//! misfire. Every button dispatches the action its key does and shows that key from the live
//! keymap.

use gpui::{Action, AnyElement, App, Pixels, SharedString, Window, div, prelude::*};

use super::{
    button::{Button, ButtonSize, ButtonStyle},
    dismiss::{Dismiss, dismiss_builders},
};
use crate::{
    components::{ConfirmKey, Dialog, FactList, FreshnessStamp},
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// A destructive confirm.
#[derive(IntoElement)]
pub struct ConfirmDialog {
    title: SharedString,
    target: Option<SharedString>,
    facts: FactList,
    consequence: Option<SharedString>,
    stamp: Option<FreshnessStamp>,
    icon: Option<Icon>,
    confirm_key_override: Option<ConfirmKey>,
    action_label: SharedString,
    strong_label: Option<SharedString>,
    width_override: Option<Pixels>,
    body_override: Option<AnyElement>,
    footer_start: Option<AnyElement>,
    error: Option<SharedString>,
    dismiss: Option<Dismiss>,
    accept: Option<(Box<dyn Action>, Box<dyn Action>)>,
    accept_disabled: bool,
    recheck: Option<Box<dyn Action>>,
}

impl ConfirmDialog {
    /// A confirm with a title and its facts.
    pub fn new(title: impl Into<SharedString>, facts: FactList) -> Self {
        Self {
            title: title.into(),
            target: None,
            facts,
            consequence: None,
            stamp: None,
            icon: None,
            confirm_key_override: None,
            action_label: SharedString::new_static("Delete"),
            strong_label: None,
            width_override: None,
            body_override: None,
            footer_start: None,
            error: None,
            dismiss: None,
            accept: None,
            accept_disabled: false,
            recheck: None,
        }
    }

    dismiss_builders!();

    /// What is being acted on, in full: the subtitle under the title (`acme/api · ~/wt/hotfix`).
    pub fn target(mut self, target: impl Into<SharedString>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// The consequence sentence, in plain future tense, naming exactly what is lost. Users
    /// confirm the sentence.
    pub fn consequence(mut self, consequence: impl Into<SharedString>) -> Self {
        self.consequence = Some(consequence.into());
        self
    }

    /// The mandatory freshness stamp.
    pub fn stamp(mut self, stamp: FreshnessStamp) -> Self {
        self.stamp = Some(stamp);
        self
    }

    /// The re-check action: a `Re-check` link button after the stamp, showing its live key.
    pub fn recheck_action(mut self, action: Box<dyn Action>) -> Self {
        self.recheck = Some(action);
        self
    }

    /// The tile glyph: `trash`, `scissors`, `power`, `x`, or `triangle-alert` when expanded.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The two confirm actions: `lower` (`y`, and `Enter`) and `strong` (`Y`). The action
    /// button dispatches whichever the [`ConfirmDialog::confirm_key`] asks for and shows that
    /// action's key. Without them the footer states the key as a label only.
    pub fn accept_actions(mut self, lower: Box<dyn Action>, strong: Box<dyn Action>) -> Self {
        self.accept = Some((lower, strong));
        self
    }

    /// Draw the action button unavailable: the facts it needs are still being gathered, or a
    /// re-check is required first. The key does nothing in that state either.
    pub fn accept_disabled(mut self, disabled: bool) -> Self {
        self.accept_disabled = disabled;
        self
    }

    /// Force the confirmation strength independently of the facts.
    pub fn force_confirm_key(mut self, key: ConfirmKey) -> Self {
        self.confirm_key_override = Some(key);
        self
    }

    /// The verb on the action button, e.g. `Delete`, `Prune 3`, `Stop and quit`.
    pub fn action_label(mut self, label: impl Into<SharedString>) -> Self {
        self.action_label = label.into();
        self
    }

    /// The verb on the red strong button. Defaults to the action label plus ` anyway`.
    pub fn strong_label(mut self, label: impl Into<SharedString>) -> Self {
        self.strong_label = Some(label.into());
        self
    }

    /// Override the width. Defaults to 480 (compact) / 560 (expanded); prune passes 720.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width_override = Some(width);
        self
    }

    /// Replace the fact block with a multi-target body — the prune dialog's `Delete` / `Keep`
    /// sections. The stamp, consequence and key rules are unchanged.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body_override = Some(body.into_any_element());
        self
    }

    /// The footer's left side, e.g. the prune dialog's `Show kept` toggle button.
    pub fn footer_start(mut self, start: impl IntoElement) -> Self {
        self.footer_start = Some(start.into_any_element());
        self
    }

    /// A red footer line: the confirm ran and the daemon refused. The dialog stays open.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Whether this confirm renders in its compact form.
    pub fn is_compact(&self) -> bool {
        self.facts.is_compact() && self.body_override.is_none()
    }

    /// The key this confirm binds.
    pub fn confirm_key(&self) -> ConfirmKey {
        self.confirm_key_override
            .unwrap_or_else(|| self.facts.confirm_key())
    }

    /// The label the action button reads for the resolved key.
    pub fn button_label(&self) -> SharedString {
        match self.confirm_key() {
            ConfirmKey::Lower => self.action_label.clone(),
            ConfirmKey::Upper => self
                .strong_label
                .clone()
                .unwrap_or_else(|| format!("{} anyway", self.action_label).into()),
        }
    }

    /// The width this confirm resolves to, given a theme: compact (§3.8) while every decisive
    /// fact is known and benign, expanded once a risk is true or a fact is unknown.
    pub fn resolved_width(&self, theme: &Theme) -> Pixels {
        self.width_override.unwrap_or(if self.is_compact() {
            theme.metrics.confirm_compact_w
        } else {
            theme.metrics.dialog_w
        })
    }
}

/// The facts on one line: `✓ clean   ✓ merged into origin/main   ✓ no session`.
///
/// Only the compact form uses this, and the compact form is by definition all-safe, so the row
/// never has to wrap a risk sentence.
fn compact_facts(facts: &FactList, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(theme.space.lg)
        .children(facts.ordered().into_iter().map(|fact| {
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(
                    fact.icon()
                        .el()
                        .size(IconSize::Small)
                        .color(fact.tone().color(theme)),
                )
                .child(fact.sentence(theme))
        }))
        .into_any_element()
}

impl RenderOnce for ConfirmDialog {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let compact = self.is_compact();
        let key = self.confirm_key();
        let width = self.resolved_width(theme);
        let button_label = self.button_label();
        let icon = self.icon.unwrap_or(if compact {
            Icon::Trash
        } else {
            Icon::TriangleAlert
        });

        let stamp = self.stamp.map(|stamp| match self.recheck {
            Some(recheck) => stamp.trailing(
                Button::new("confirm-recheck", "Re-check")
                    .style(ButtonStyle::Ghost)
                    .size(ButtonSize::Compact)
                    .action(recheck),
            ),
            None => stamp,
        });

        let facts_block: AnyElement = match self.body_override {
            Some(body) => body,
            None if compact => compact_facts(&self.facts, theme),
            None => self.facts.into_any_element(),
        };

        let body = div()
            .flex()
            .flex_col()
            .gap(theme.space.md)
            .w_full()
            .child(facts_block)
            .children(stamp)
            .children(
                self.consequence
                    .map(|line| Text::ui(line).tone(Tone::Secondary)),
            );

        // A confirmation is the one dialog whose footer keeps its keys on the face
        // (DESIGN-SYSTEM §4): whether `y` or only `⇧Y` confirms is the point of the dialog, and
        // the dialog holds the focus for as long as it is open, so the chips never come and go.
        let cancel = self
            .dismiss
            .as_ref()
            .map(|dismiss| dismiss.cancel_button("confirm-cancel").show_kbd());
        let accept = self.accept.map(|(lower, strong)| {
            let button = Button::new("confirm-accept", button_label.clone());
            let button = match key {
                ConfirmKey::Lower => button.style(ButtonStyle::Primary).action(lower),
                ConfirmKey::Upper => button.style(ButtonStyle::Danger).action(strong),
            };
            button.show_kbd().disabled(self.accept_disabled)
        });

        let mut dialog = Dialog::new(self.title)
            .alert()
            .icon(icon)
            .width(width)
            .tone(if compact {
                Tone::Default
            } else {
                Tone::Warning
            })
            .body(body)
            .dismiss(self.dismiss);
        if let Some(target) = self.target {
            dialog = dialog.subtitle(target);
        }
        dialog = match accept {
            Some(accept) => dialog.actions(cancel.into_iter().chain([accept]).collect()),
            None => dialog.primary(format!("{}  {button_label}", key.label())),
        };
        if let Some(start) = self.footer_start {
            dialog = dialog.footer_start(start);
        }
        if let Some(error) = self.error {
            dialog = dialog.error(error);
        }
        dialog
    }
}

#[cfg(test)]
mod tests {
    use crate::components::Fact;

    use super::*;
    use gpui::px;

    #[test]
    fn all_safe_facts_give_the_compact_form_and_the_lower_key() {
        let theme = Theme::default();
        let confirm = ConfirmDialog::new(
            "Delete buk/payroll#fix-rut-validator?",
            FactList::from_facts([Fact::safe("clean"), Fact::safe("no session")]),
        );
        assert!(confirm.is_compact());
        assert_eq!(confirm.confirm_key(), ConfirmKey::Lower);
        assert!(confirm.confirm_key().accepts_enter());
        assert_eq!(
            confirm.resolved_width(&theme),
            theme.metrics.confirm_compact_w
        );
    }

    #[test]
    fn a_risk_expands_the_card_but_keeps_the_lower_key() {
        let theme = Theme::default();
        let confirm = ConfirmDialog::new(
            "Delete worktree",
            FactList::from_facts([Fact::risk("12 uncommitted files")]),
        );
        assert!(!confirm.is_compact());
        assert_eq!(confirm.confirm_key(), ConfirmKey::Lower);
        assert_eq!(confirm.resolved_width(&theme), theme.metrics.dialog_w);
    }

    #[test]
    fn an_unknown_fact_forces_the_upper_key_and_refuses_enter() {
        let confirm = ConfirmDialog::new(
            "Delete worktree",
            FactList::from_facts([
                Fact::safe("clean"),
                Fact::unknown("unique commit count unavailable (gh unavailable)"),
            ]),
        );
        assert!(!confirm.is_compact());
        assert_eq!(confirm.confirm_key(), ConfirmKey::Upper);
        assert!(!confirm.confirm_key().accepts_enter());
    }

    #[test]
    fn the_strong_key_reads_as_a_stronger_verb_on_the_button() {
        let lower = ConfirmDialog::new(
            "Delete worktree hotfix?",
            FactList::from_facts([Fact::risk("3 uncommitted files")]),
        );
        assert_eq!(lower.button_label().as_ref(), "Delete");
        let upper = ConfirmDialog::new(
            "Delete worktree hotfix?",
            FactList::from_facts([Fact::unknown("session state unknown")]),
        );
        assert_eq!(upper.button_label().as_ref(), "Delete anyway");
        let named = ConfirmDialog::new("Delete repository acme/api?", FactList::new())
            .force_confirm_key(ConfirmKey::Upper)
            .strong_label("Delete repository");
        assert_eq!(named.button_label().as_ref(), "Delete repository");
    }

    #[test]
    fn facts_still_loading_escalate_the_key() {
        let confirm = ConfirmDialog::new("Delete worktree", FactList::new().loading(true));
        assert_eq!(confirm.confirm_key(), ConfirmKey::Upper);
        assert!(!confirm.is_compact());
    }

    #[test]
    fn a_multi_target_body_is_never_compact() {
        let confirm = ConfirmDialog::new(
            "Prune buk/payroll \u{2014} 3 of 8 worktrees",
            FactList::from_facts([Fact::safe("dry run complete")]),
        )
        .body(gpui::div())
        .width(px(720.0));
        assert!(!confirm.is_compact());
        assert_eq!(confirm.resolved_width(&Theme::default()), px(720.0));
    }
}
