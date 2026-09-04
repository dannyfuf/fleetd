//! `ConfirmDialog` — compact or expanded, decided by the facts.
//!
//! §1.7 made literal, and §3.8.3's escalation rule [D-10] enforced in one place:
//!
//! * every decisive fact known and benign -> the **compact** form, 480 px, facts on one line,
//!   `y` confirms and `Enter` is accepted;
//! * any risk true, or any decisive fact unknown -> the **expanded** form, 560 px, one line
//!   per fact, `⚠` title tone, and `Y` is required with `Enter` **not** accepted.
//!
//! The freshness stamp is mandatory on every facts confirm: this is the one surface where the
//! age of a fact decides an outcome. There is no "don't ask again", no second confirmation, no
//! countdown and no typed-name confirmation — the compact form *is* the answer to confirm
//! fatigue.
//!
//! The bound keys are exactly `y` / `Y` / `Enter` (confirm), `n` / `Esc` / `q` (cancel), plus
//! whatever the caller adds through [`ConfirmDialog::hints`] (`I` re-check, `s` toggle the KEEP
//! list). Nothing else is bound, so muscle memory cannot misfire.

use gpui::{AnyElement, App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{
    components::{ConfirmKey, Dialog, FactList, FreshnessStamp, KeyHint, KeyHintRow},
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// The compact card width (§3.8): every decisive fact known and benign.
const COMPACT_W: Pixels = px(480.0);

/// The expanded card width: a risk is true or a decisive fact is unknown.
const EXPANDED_W: Pixels = px(560.0);

/// A destructive confirm.
#[derive(IntoElement)]
pub struct ConfirmDialog {
    title: SharedString,
    target: Option<SharedString>,
    facts: FactList,
    consequence: Option<SharedString>,
    stamp: Option<FreshnessStamp>,
    icon: Option<Icon>,
    extra_hints: Option<KeyHintRow>,
    action_label: SharedString,
    width_override: Option<Pixels>,
    body_override: Option<AnyElement>,
    error: Option<SharedString>,
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
            extra_hints: None,
            action_label: SharedString::new_static("Delete"),
            width_override: None,
            body_override: None,
            error: None,
        }
    }

    /// The full id of what is being acted on. Shown on its own line in the expanded form.
    pub fn target(mut self, target: impl Into<SharedString>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// The consequence sentence, in plain future tense. Users confirm the sentence.
    pub fn consequence(mut self, consequence: impl Into<SharedString>) -> Self {
        self.consequence = Some(consequence.into());
        self
    }

    /// The mandatory freshness stamp.
    pub fn stamp(mut self, stamp: FreshnessStamp) -> Self {
        self.stamp = Some(stamp);
        self
    }

    /// The header glyph: `trash`, `scissors`, `power`, `x`, or `triangle-alert` when expanded.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Extra footer keys, e.g. `I re-check` or `s toggle the KEEP list`. They are prepended to
    /// the cancel hint, never in place of it.
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.extra_hints = Some(hints);
        self
    }

    /// The verb on the primary action, e.g. `Delete`, `Prune 3`, `Stop and quit`.
    pub fn action_label(mut self, label: impl Into<SharedString>) -> Self {
        self.action_label = label.into();
        self
    }

    /// Override the width. Defaults to 480 (compact) / 560 (expanded); prune passes 720.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width_override = Some(width);
        self
    }

    /// Replace the fact block with a multi-target body — the prune dialog's `DELETE` / `KEEP`
    /// lists. The stamp, consequence and key rules are unchanged.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body_override = Some(body.into_any_element());
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
        self.facts.confirm_key()
    }

    /// The width this confirm will render at.
    pub fn resolved_width(&self) -> Pixels {
        self.width_override.unwrap_or(if self.is_compact() {
            COMPACT_W
        } else {
            EXPANDED_W
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
                .child(Text::ui(fact.text.clone()))
        }))
        .into_any_element()
}

impl RenderOnce for ConfirmDialog {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let compact = self.is_compact();
        let key = self.confirm_key();
        let width = self.resolved_width();
        let icon = self.icon.unwrap_or(if compact {
            Icon::Trash
        } else {
            Icon::TriangleAlert
        });

        // `Enter` is accepted exactly where `y` is; where `Y` is required it is not, and the
        // footer must not imply otherwise.
        let mut standard = KeyHintRow::new();
        if key.accepts_enter() {
            standard = standard.key("\u{23ce}", "confirm");
        }
        standard = standard.hint(KeyHint::labeled("n / esc", "cancel"));

        // TODO(INTEGRATION `KeyHintRow::merge`): `key_hint.rs` keeps its hints private, so the
        // caller's extra keys are joined here with the same `·` the row uses internally.
        let hints: AnyElement = match self.extra_hints {
            Some(extra) => div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(extra)
                .child(Text::hint("\u{b7}").faint())
                .child(standard)
                .into_any_element(),
            None => standard.into_any_element(),
        };

        let facts_block: AnyElement = match self.body_override {
            Some(body) => body,
            None if compact => compact_facts(&self.facts, &theme),
            None => self.facts.clone().into_any_element(),
        };

        let body = div()
            .flex()
            .flex_col()
            .gap(theme.space.md)
            .w_full()
            .py(theme.space.lg)
            // Expanded: the full target on its own line, because deleting the wrong copy is
            // the top failure mode. Compact: the title already carries it.
            .children(
                self.target
                    .filter(|_| !compact)
                    .map(|target| Text::data(target).ellipsize()),
            )
            .child(facts_block)
            .children(self.stamp)
            .children(
                self.consequence
                    .map(|line| Text::ui(line).tone(Tone::Secondary)),
            );

        let mut dialog = Dialog::new(self.title)
            .icon(icon)
            .width(width)
            .tone(if compact {
                Tone::Default
            } else {
                Tone::Warning
            })
            .body(body)
            .hints(hints)
            .primary(format!("{}  {}", key.label(), self.action_label));
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

    #[test]
    fn all_safe_facts_give_the_compact_form_and_the_lower_key() {
        let confirm = ConfirmDialog::new(
            "Delete buk/payroll#fix-rut-validator?",
            FactList::from_facts([Fact::safe("clean"), Fact::safe("no session")]),
        );
        assert!(confirm.is_compact());
        assert_eq!(confirm.confirm_key(), ConfirmKey::Lower);
        assert!(confirm.confirm_key().accepts_enter());
        assert_eq!(confirm.resolved_width(), COMPACT_W);
    }

    #[test]
    fn a_risk_expands_the_card_but_keeps_the_lower_key() {
        let confirm = ConfirmDialog::new(
            "Delete worktree",
            FactList::from_facts([Fact::risk("12 uncommitted files")]),
        );
        assert!(!confirm.is_compact());
        assert_eq!(confirm.confirm_key(), ConfirmKey::Lower);
        assert_eq!(confirm.resolved_width(), EXPANDED_W);
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
        assert_eq!(confirm.resolved_width(), px(720.0));
    }
}
