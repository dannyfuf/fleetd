//! `SettingsRow` — one setting: its label and a helper sentence on the left, its control at the
//! row's end.
//!
//! **Purpose.** Every editable or read-only setting in a settings dialog (Settings, Board
//! settings, Repository hooks) is one of these, so the three dialogs read as one grammar: a row
//! of at least 44 px (`row_h_comfortable`), the label in the body face, one helper sentence under
//! it, and the control — a [`super::Switch`], an inline [`super::Cycler`], a [`super::ValueBox`]
//! or a read-only value — at the end, where the eye finds every value down one column.
//!
//! **Anatomy.** `[leading] [label + badge / helper] [hover actions] [control] [trailing]`, `gap lg`
//! between the slots, `px md`, 6 px vertical padding. The label block is the only flexible slot;
//! every other slot keeps its width, so a long helper wraps instead of pushing the control.
//!
//! **API.** [`SettingsRow::new`] takes the row's id; everything else is a builder:
//! [`label`](SettingsRow::label) or [`label_spans`](SettingsRow::label_spans) (a search hit's
//! label with its matched words in the strong face), [`label_badge`](SettingsRow::label_badge),
//! [`helper`](SettingsRow::helper) / [`helper_mono`](SettingsRow::helper_mono) (a pattern),
//! [`invalid`](SettingsRow::invalid), the four slots, the three state flags and the three
//! pointer handlers.
//!
//! **States.** rest · pointer hover (`row_hover`; never over the cursor, never while disabled) ·
//! cursor (`row_selected` and the 2 px bar of [`FocusRing::cursor_row`], the row the keyboard is
//! on) · editing (the caller's business: the row stays the cursor and its [`super::ValueBox`]
//! draws the editor, so the row never changes height) · invalid (the rule **replaces** the
//! helper, in `danger`) · disabled (`dimmed_opacity`, no hover, no pointer; the helper says why).
//!
//! **Pointer (ADR 0023).** A press lands the cursor ([`SettingsRow::on_click`]); the second press
//! of a double click opens the row ([`SettingsRow::on_double_click`], the twin of `⏎`); a right
//! click opens the row's menu ([`SettingsRow::on_secondary_click`]). Landing never opens an editor.
//! [`SettingsRow::hover_actions`] are revealed while the row is hovered or is the cursor (on the
//! cursor row even while disabled), with their width reserved either way; every one of them must
//! also be in the row's menu, so nothing lives only behind hover. A press on any slot — the
//! leading control, a hover action, the control, the trailing button — is that control's, and
//! the second press of a double click on one never opens the row. The row is not focusable: the
//! dialog's list owns focus and keys.
//!
//! **Usage rule.** Use a `SettingsRow` inside a [`super::SettingsCard`] for anything a settings
//! pane lists. A list of records (worktrees, jobs, a picker) is a [`super::Row`]; a form field
//! that is always editable is a bare [`super::TextInput`].

use std::ops::Range;

use gpui::{
    AnyElement, App, ElementId, HighlightStyle, MouseButton, MouseDownEvent, SharedString,
    StyledText, Window, div, prelude::*,
};

use crate::{
    components::Badge,
    focus::FocusRing,
    text::{Text, TextRole},
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// The hover group every settings row names itself with, so its hover actions reveal while *its
/// own* row is hovered. gpui resolves a group name to the innermost enclosing element that
/// declared it, so one name serves any number of sibling rows.
const SETTINGS_ROW_GROUP: &str = "fleet-settings-row";

/// A pointer handler on a row. It receives the raw press, so a view can hand it a
/// `cx.listener(..)` directly.
type PressHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// What the row says under its label.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Helper {
    /// A sentence, in the caption face.
    Sentence(SharedString),
    /// A pattern or an identifier, in the small data face.
    Mono(SharedString),
}

/// One setting in a settings card.
#[derive(IntoElement)]
pub struct SettingsRow {
    id: ElementId,
    label: Option<SharedString>,
    label_spans: Option<Vec<(SharedString, bool)>>,
    label_badge: Option<Badge>,
    helper: Option<Helper>,
    invalid: Option<SharedString>,
    leading: Option<AnyElement>,
    control: Option<AnyElement>,
    trailing: Option<AnyElement>,
    hover_actions: Option<AnyElement>,
    cursor: bool,
    disabled: bool,
    tall: bool,
    on_click: Option<PressHandler>,
    on_double_click: Option<PressHandler>,
    on_secondary_click: Option<PressHandler>,
}

impl SettingsRow {
    /// An empty row with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            label_spans: None,
            label_badge: None,
            helper: None,
            invalid: None,
            leading: None,
            control: None,
            trailing: None,
            hover_actions: None,
            cursor: false,
            disabled: false,
            tall: false,
            on_click: None,
            on_double_click: None,
            on_secondary_click: None,
        }
    }

    /// The setting's name, in the body face. Sentence case, no trailing full stop.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The label as spans, `true` marking the ones drawn in the strong face: a search hit's
    /// label with the matched words standing out. Concatenated, the spans are the label; it
    /// replaces [`SettingsRow::label`].
    pub fn label_spans(mut self, spans: impl IntoIterator<Item = (SharedString, bool)>) -> Self {
        self.label_spans = Some(spans.into_iter().collect());
        self
    }

    /// A badge after the label: what kind of thing the row is (`command`, `port`, `started`).
    pub fn label_badge(mut self, badge: Badge) -> Self {
        self.label_badge = Some(badge);
        self
    }

    /// One sentence under the label saying what the setting does. It wraps; it ends with a full
    /// stop.
    pub fn helper(mut self, helper: impl Into<SharedString>) -> Self {
        self.helper = Some(Helper::Sentence(helper.into()));
        self
    }

    /// A pattern or an identifier under the label, in the small data face: a keep-alive rule's
    /// match, for example.
    pub fn helper_mono(mut self, helper: impl Into<SharedString>) -> Self {
        self.helper = Some(Helper::Mono(helper.into()));
        self
    }

    /// The rule the row's value breaks. It **replaces** the helper, in `danger`, so the row keeps
    /// its height and the failure names the exact rule it failed (§3.8).
    pub fn invalid(mut self, rule: impl Into<SharedString>) -> Self {
        self.invalid = Some(rule.into());
        self
    }

    /// A slot before the label block: a category glyph, a number, or a rule row's
    /// [`super::Switch`].
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// The control at the row's end: a switch, an inline cycler, a value box or a read-only
    /// value. It keeps its own width.
    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }

    /// A slot after the control: a drill-in chevron, a copy button.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// Buttons drawn only while the row is hovered or is the cursor, before the control.
    ///
    /// The slot keeps its width reserved while hidden, so revealing it never reflows the row.
    /// Every action placed here must also be reachable from the row's key and from its
    /// right-click menu (UX-SPEC §5.1).
    pub fn hover_actions(mut self, actions: impl IntoElement) -> Self {
        self.hover_actions = Some(actions.into_any_element());
        self
    }

    /// This is the row the keyboard is on: `row_selected` and the 2 px cursor bar.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// The setting does not apply here. The row dims to `dimmed_opacity`, drops hover and ignores
    /// the pointer; its helper should say why.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The row holds a multi-line control: the slots align to the top and the row pads `sm`
    /// above and below, so a growing box pushes the row down instead of centring in it.
    pub fn tall(mut self, tall: bool) -> Self {
        self.tall = tall;
        self
    }

    /// A single press of the primary button: land the cursor on this row.
    ///
    /// Fires on the press, like every list row, so the cursor is already here when the second
    /// press of a double click arrives. A handler implies the pointer cursor.
    pub fn on_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// The second press of a double click: open the row, the pointer twin of `⏎`.
    ///
    /// Read from the press's click count, so the first press of the pair has already run
    /// [`SettingsRow::on_click`].
    pub fn on_double_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_double_click = Some(Box::new(handler));
        self
    }

    /// A right click (a ctrl-click on macOS): open the row's menu at `event.position`, listing
    /// every row action, hover actions included.
    pub fn on_secondary_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary_click = Some(Box::new(handler));
        self
    }

    /// Whether the row is drawn as not applicable.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// What the row states under its label: the broken rule when invalid, else the helper.
    pub fn shown_helper(&self) -> Option<SharedString> {
        self.invalid.clone().or_else(|| {
            self.helper.as_ref().map(|helper| match helper {
                Helper::Sentence(text) | Helper::Mono(text) => text.clone(),
            })
        })
    }

    /// Whether any pointer handler is attached.
    fn is_pressable(&self) -> bool {
        self.on_click.is_some()
            || self.on_double_click.is_some()
            || self.on_secondary_click.is_some()
    }
}

impl RenderOnce for SettingsRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let live = !self.disabled;
        let cursor = self.cursor;
        // Hover is pointer feedback only: it never paints over the cursor row's fill.
        let hoverable = live && !cursor;
        let pressable = live && self.is_pressable();
        let hover_bg = theme.colors.row_hover;

        let under_label = match (self.invalid, self.helper) {
            (Some(rule), _) => Some(Text::caption(rule).tone(Tone::Danger)),
            (None, Some(Helper::Sentence(text))) => Some(Text::caption(text).muted()),
            (None, Some(Helper::Mono(text))) => Some(Text::data_small(text).muted()),
            (None, None) => None,
        };
        // A row with no text at all (a hooks command) hands the label block's width to its
        // control instead, so a `Fill` box really fills the row.
        let has_text = self.label.is_some()
            || self.label_spans.is_some()
            || self.label_badge.is_some()
            || under_label.is_some();
        let label_text: Option<AnyElement> = match (self.label_spans, self.label) {
            (Some(spans), _) => Some(strong_spans(&spans, theme)),
            (None, Some(label)) => Some(Text::ui(label).into_any_element()),
            (None, None) => None,
        };
        let label_line = div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .min_w_0()
            .children(label_text)
            .children(self.label_badge);
        let label_block = has_text.then(|| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(theme.space.xxs)
                .child(label_line)
                .children(under_label)
        });

        let hover_actions = self.hover_actions.map(|actions| {
            div()
                .flex_none()
                .flex()
                .items_center()
                // Hidden, not removed: `invisible` keeps the layout box, so revealing the
                // actions on hover never moves the control beside them. The cursor row shows
                // them even while disabled: a disabled schedule's *Run now* is still its key's
                // twin, and a row nobody can hover has nowhere else to show it.
                .when(!cursor, |el| {
                    el.invisible().when(live, |el| {
                        el.group_hover(SETTINGS_ROW_GROUP, |style| style.visible())
                    })
                })
                .child(actions)
        });
        // A press on a slot is the control's: a switch flips, a hover button runs, a box opens.
        // The second press of a quick double click on one must not open the row as well — after
        // *Move up* the row under the pointer is already another column — so the slot swallows
        // it. A single press still reaches the row, which lands the cursor as any press does.
        let slot = |child: AnyElement| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .on_mouse_down(MouseButton::Left, |event, _, cx| {
                    if event.click_count >= 2 {
                        cx.stop_propagation();
                    }
                })
                .child(child)
        };
        let control = self.control.map(|control| {
            if has_text {
                slot(control)
            } else {
                slot(control).flex_1().min_w_0()
            }
        });

        // The body carries the minimum height and the padding, so the cursor bar around it spans
        // the whole row however tall the label block or the control grows.
        let body = div()
            .flex()
            .w_full()
            .min_h(theme.metrics.row_h_comfortable)
            .px(theme.space.md)
            .gap(theme.space.lg)
            .map(|el| {
                if self.tall {
                    el.items_start().py(theme.space.sm)
                } else {
                    el.items_center().py(theme.space.xs + theme.space.xxs)
                }
            })
            .children(self.leading.map(slot))
            .children(label_block)
            .children(hover_actions.map(|actions| slot(actions.into_any_element())))
            .children(control)
            .children(self.trailing.map(slot));

        let (on_click, on_double_click, on_secondary_click) = if live {
            (self.on_click, self.on_double_click, self.on_secondary_click)
        } else {
            (None, None, None)
        };
        let primary = (on_click.is_some() || on_double_click.is_some())
            .then_some((on_click, on_double_click));

        div()
            .id(self.id)
            .w_full()
            .group(SETTINGS_ROW_GROUP)
            .when(cursor, |el| el.bg(theme.colors.row_selected))
            .when(!live, |el| el.opacity(theme.metrics.dimmed_opacity))
            .when(hoverable, |el| el.hover(move |style| style.bg(hover_bg)))
            .when(pressable, |el| el.cursor_pointer())
            .when_some(primary, |el, (on_click, on_double_click)| {
                el.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    if event.click_count >= 2
                        && let Some(open) = &on_double_click
                    {
                        open(event, window, cx);
                    } else if let Some(select) = &on_click {
                        select(event, window, cx);
                    }
                })
            })
            .when_some(on_secondary_click, |el, menu| {
                el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    menu(event, window, cx)
                })
            })
            .child(FocusRing::cursor_row(cursor).content(body))
    }
}

/// One ellipsised run of the label in the `ui` face, its strong spans in the `ui_strong`
/// weight: one text run, not one element per span, so it ellipsises as a label does.
fn strong_spans(spans: &[(SharedString, bool)], theme: &Theme) -> AnyElement {
    let (text, ranges) = join_spans(spans);
    let strong = HighlightStyle {
        color: Some(theme.colors.text),
        font_weight: Some(theme.text.ui_strong.weight),
        ..Default::default()
    };
    crate::styled_with(div(), TextRole::Ui.style(theme), theme)
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.colors.text)
        .child(
            StyledText::new(text).with_highlights(ranges.into_iter().map(|range| (range, strong))),
        )
        .into_any_element()
}

/// The spans joined back into one string, and the byte ranges of the strong ones.
fn join_spans(spans: &[(SharedString, bool)]) -> (SharedString, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut ranges = Vec::new();
    for (span, strong) in spans {
        let start = text.len();
        text.push_str(span);
        if *strong && !span.is_empty() {
            ranges.push(start..text.len());
        }
    }
    (SharedString::from(text), ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(parts: &[(&str, bool)]) -> Vec<(SharedString, bool)> {
        parts
            .iter()
            .map(|(text, strong)| (SharedString::from((*text).to_owned()), *strong))
            .collect()
    }

    #[test]
    fn joined_spans_are_the_label_and_the_strong_ranges_are_byte_ranges() {
        let (text, ranges) = join_spans(&spans(&[]));
        assert_eq!(text.as_ref(), "");
        assert!(ranges.is_empty());

        let (text, ranges) = join_spans(&spans(&[
            ("Keep finished ", false),
            ("jobs", true),
            (" for", false),
        ]));
        assert_eq!(text.as_ref(), "Keep finished jobs for");
        assert_eq!(ranges, vec![14..18]);

        let (text, ranges) = join_spans(&spans(&[
            ("Claude \u{203a} ", false),
            ("Eff", true),
            ("ort", true),
        ]));
        assert_eq!(text.as_ref(), "Claude \u{203a} Effort");
        // Byte ranges (`\u{203a}` is three bytes); adjacent strong spans keep their own range and
        // the highlighter draws them as one.
        assert_eq!(ranges, vec![11..14, 14..17]);

        let (_, ranges) = join_spans(&spans(&[("", true), ("Grace", false)]));
        assert!(ranges.is_empty(), "an empty span highlights nothing");
    }

    #[test]
    fn a_row_shows_its_helper_until_a_rule_replaces_it() {
        let row = SettingsRow::new("grace")
            .label("Grace")
            .helper("How long a terminal gets to finish before sleep closes it.");
        assert_eq!(
            row.shown_helper().as_deref(),
            Some("How long a terminal gets to finish before sleep closes it.")
        );
        let row = row.invalid("Must be at least 0 ms.");
        assert_eq!(
            row.shown_helper().as_deref(),
            Some("Must be at least 0 ms.")
        );
    }

    #[test]
    fn a_mono_helper_is_shown_like_a_sentence_one() {
        let row = SettingsRow::new("rule").helper_mono("vite|next dev");
        assert_eq!(row.shown_helper().as_deref(), Some("vite|next dev"));
    }

    #[test]
    fn a_rule_without_a_helper_is_still_shown() {
        let row = SettingsRow::new("refresh").invalid("Must be at least 500 ms.");
        assert_eq!(
            row.shown_helper().as_deref(),
            Some("Must be at least 500 ms.")
        );
        assert!(SettingsRow::new("bare").shown_helper().is_none());
    }

    #[test]
    fn a_row_is_live_until_disabled() {
        assert!(!SettingsRow::new("push").is_disabled());
        assert!(SettingsRow::new("push").disabled(true).is_disabled());
    }
}
