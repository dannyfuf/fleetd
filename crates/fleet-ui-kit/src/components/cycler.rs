//! `Cycler` — a closed choice in a settings row, changed with `←` / `→` (`h` / `l`).
//!
//! The keyboard is the cycler's, unchanged; the drawing picks the form that reads at a glance.
//! Given its [`Cycler::options`], a set of up to [`SEGMENTED_MAX`] short options (at most
//! [`SEGMENTED_MAX_CHARS`] characters together) draws as a [`SegmentedControl`] with the value
//! raised, and any other set as a compact [`Dropdown`] whose list opens on a click. A cycler whose caller has not listed its options (or whose value is off
//! the configured steps) draws the dropdown field alone, stating the value. A caller whose set
//! reads as a list whatever its size — a repository id, a model catalogue that ends in `Other
//! model id…`, where a segment would read as a value — asks for the dropdown with
//! [`Cycler::dropdown`].
//!
//! Zero-suppression applies to the control itself: a set with fewer than two members has no
//! choice in it, so [`Cycler::is_visible`] is false and the host cycler disappears when no
//! hosts are configured.
//!
//! ## Two chromes
//!
//! - **Row** (the default): the cycler draws its own `row_h` band — the label, the control at
//!   the end, the cursor fill and bar. Create worktree's form keeps this one.
//! - **Inline** ([`Cycler::inline`]): the cycler draws the control alone — no label, no cursor
//!   band, no label column — for a [`super::SettingsRow`] that owns all of that and takes the
//!   cycler as its `control`. The form rule, [`Cycler::harness`], [`Cycler::on_select`],
//!   [`Cycler::unavailable`], [`Cycler::disabled`] and [`Cycler::off_grid`] work the same in
//!   both. Every settings row uses this one.
//!
//! A dropdown's items can name what each option resolves to ([`Cycler::details`], e.g. a model
//! id beside `Sonnet`), drawn as the menu item's trailing detail.

use std::rc::Rc;

use gpui::{App, ElementId, SharedString, Window, div, prelude::*};

use super::{
    menu::{Dropdown, MenuItem},
    segmented_control::{Segment, SegmentedControl},
};
use crate::{harness::HarnessTargetExt as _, text::Text, theme::ActiveTheme, tone::Tone};

/// The most options a cycler draws side by side; a longer set draws as a dropdown.
pub const SEGMENTED_MAX: usize = 4;

/// The most characters, summed over every option, a cycler draws side by side. Four short words
/// (`Low` `Medium` `High` `Max`) fit a settings row beside its label; four phrases
/// (`asks before edits` …) do not, and draw as a dropdown instead of pushing the label out.
pub const SEGMENTED_MAX_CHARS: usize = 32;

type CyclerSelect = dyn Fn(usize, &mut Window, &mut App);

/// A left/right value cycler.
#[derive(IntoElement)]
pub struct Cycler {
    id: Option<ElementId>,
    label: Option<SharedString>,
    value: SharedString,
    options: Option<Vec<SharedString>>,
    details: Vec<SharedString>,
    inline: bool,
    dropdown: bool,
    has_prev: bool,
    has_next: bool,
    focused: bool,
    disabled: bool,
    off_grid: bool,
    label_width: Option<gpui::Pixels>,
    on_select: Option<Rc<CyclerSelect>>,
    unavailable: Vec<usize>,
    harness_segments: Option<&'static str>,
    harness_dropdown: Option<&'static str>,
}

/// How a cycler draws, decided from what its caller told it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CyclerForm {
    /// Side by side, the value raised at this index.
    Segmented(usize),
    /// A dropdown field; `true` when it lists the options on a click.
    Dropdown(bool),
}

impl Cycler {
    /// A cycler showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            label: None,
            value: value.into(),
            options: None,
            details: Vec::new(),
            inline: false,
            dropdown: false,
            has_prev: true,
            has_next: true,
            focused: false,
            disabled: false,
            off_grid: false,
            label_width: None,
            on_select: None,
            unavailable: Vec::new(),
            harness_segments: None,
            harness_dropdown: None,
        }
    }

    /// A labelled cycler.
    pub fn labeled(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self::new(value).label(label)
    }

    /// Set the label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The control's element id. Defaults to the label, which is unique within a settings list.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Every option, in cycling order, as the user reads them. The value is raised where it
    /// matches one; a value matching none (off the grid) draws as a dropdown field.
    pub fn options<T: Into<SharedString>>(mut self, options: impl IntoIterator<Item = T>) -> Self {
        self.options = Some(options.into_iter().map(Into::into).collect());
        self
    }

    /// A muted detail for each option, in the order of [`Cycler::options`], shown beside it in
    /// the dropdown's list (a model id beside its name). An empty detail draws none. A cycler
    /// drawn side by side shows no details.
    pub fn details<T: Into<SharedString>>(mut self, details: impl IntoIterator<Item = T>) -> Self {
        self.details = details.into_iter().map(Into::into).collect();
        self
    }

    /// Draw the control alone (segmented or dropdown, by [`Cycler::form`]): no label, no cursor
    /// band, no label column. For a [`super::SettingsRow`]'s `control` slot, where the row owns
    /// the label, the cursor and the dimming. See the module doc. An inline cycler has no label
    /// to take its id from, so give it one with [`Cycler::id`].
    pub fn inline(mut self, inline: bool) -> Self {
        self.inline = inline;
        self
    }

    /// Whether [`Cycler::inline`] is set.
    pub fn is_inline(&self) -> bool {
        self.inline
    }

    /// Draw the compact dropdown whatever the option count: for a set that reads as a list (a
    /// repository id, a model catalogue ending in `Other model id…`), where a segment would read
    /// as a value. The keys, the list and the harness names are the dropdown's as usual.
    pub fn dropdown(mut self, dropdown: bool) -> Self {
        self.dropdown = dropdown;
        self
    }

    /// A click choosing option `ix` of [`Cycler::options`]. Point it at the same update the
    /// row's `←` / `→` makes; without it the control is drawn only.
    pub fn on_select(mut self, on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// The options (indices into [`Cycler::options`]) that exist but cannot be chosen now: they
    /// draw dimmed and ignore a click. The keys still reach them, so the caller states why each
    /// one is unavailable next to the control.
    pub fn unavailable(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        self.unavailable = indices.into_iter().collect();
        self
    }

    /// Name each option `<part>[<index>]` for the harness target recorder while the cycler
    /// draws side by side (`dialog.segment`). A dropdown's options are `menu.item[N]` instead.
    pub fn harness_segments(mut self, part: &'static str) -> Self {
        self.harness_segments = Some(part);
        self
    }

    /// Name the controls for the harness target recorder: each segment `<options>[N]` when the
    /// cycler draws side by side, and the field `<dropdown>` when it draws as a dropdown (its
    /// open list's items are the menu's own `menu.item[N]`).
    pub fn harness(mut self, options: &'static str, dropdown: &'static str) -> Self {
        self.harness_segments = Some(options);
        self.harness_dropdown = Some(dropdown);
        self
    }

    /// Fix the label column so a stack of settings rows aligns on one gutter.
    pub fn label_width(mut self, width: gpui::Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// Whether `←` does anything.
    pub fn has_prev(mut self, has_prev: bool) -> Self {
        self.has_prev = has_prev;
        self
    }

    /// Whether `→` does anything.
    pub fn has_next(mut self, has_next: bool) -> Self {
        self.has_next = has_next;
        self
    }

    /// Whether the row carries the cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether the choice can be changed at all (a locked setting, a single-host config).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mark a persisted value that is outside the configured steps.
    ///
    /// Both arrows remain available so the next move returns to a known step.
    pub fn off_grid(mut self, off_grid: bool) -> Self {
        self.off_grid = off_grid;
        self
    }

    /// Whether the control has anything to cycle. A set with one member renders nothing.
    pub fn is_visible(&self) -> bool {
        self.off_grid || self.has_prev || self.has_next
    }

    /// The index of the value among the options, unless it is off the grid.
    fn position(&self) -> Option<usize> {
        if self.off_grid {
            return None;
        }
        self.options
            .as_ref()?
            .iter()
            .position(|option| *option == self.value)
    }

    /// The form this cycler draws in.
    pub fn form(&self) -> CyclerForm {
        let listed = self.options.as_ref().map_or(0, Vec::len);
        let chars: usize = self
            .options
            .iter()
            .flatten()
            .map(|option| option.chars().count())
            .sum();
        match self.position() {
            Some(ix)
                if !self.dropdown && listed <= SEGMENTED_MAX && chars <= SEGMENTED_MAX_CHARS =>
            {
                CyclerForm::Segmented(ix)
            }
            Some(_) => CyclerForm::Dropdown(self.on_select.is_some() && !self.disabled),
            // An off-grid value is not one of the segments, and a dropdown field can show it.
            None => CyclerForm::Dropdown(listed > 0 && self.on_select.is_some() && !self.disabled),
        }
    }
}

impl RenderOnce for Cycler {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        if !self.is_visible() && self.label.is_none() {
            return div().into_any_element();
        }

        let disabled = self.disabled;
        let form = self.form();
        let id = self
            .id
            .clone()
            .or_else(|| self.label.clone().map(ElementId::Name))
            .unwrap_or_else(|| ElementId::Name(SharedString::new_static("cycler")));
        let on_select = self.on_select.clone().filter(|_| !disabled);
        let options = self.options.unwrap_or_default();

        let control = match form {
            CyclerForm::Segmented(active) => {
                let unavailable = self.unavailable.clone();
                let segments = options
                    .into_iter()
                    .enumerate()
                    .map(|(ix, option)| Segment::new(option).disabled(unavailable.contains(&ix)));
                let mut control = SegmentedControl::new(id, segments)
                    .active(Some(active))
                    .disabled(disabled);
                if let Some(part) = self.harness_segments {
                    control = control.harness_segments(part);
                }
                match on_select {
                    Some(on_select) => control
                        .on_select(move |ix, window, cx| on_select(ix, window, cx))
                        .into_any_element(),
                    None => control.into_any_element(),
                }
            }
            CyclerForm::Dropdown(listed) => {
                let dropdown = Dropdown::new(id, self.value.clone()).compact();
                let harness = self.harness_dropdown;
                match on_select.filter(|_| listed) {
                    Some(on_select) => {
                        let current = self.value.clone();
                        let unavailable = self.unavailable.clone();
                        let details = self.details.clone();
                        dropdown
                            .menu(move |menu, _, _| {
                                options.iter().enumerate().fold(menu, |menu, (ix, option)| {
                                    // A dropdown lists only what a click can choose.
                                    if unavailable.contains(&ix) {
                                        return menu;
                                    }
                                    let on_select = on_select.clone();
                                    let item = MenuItem::new(option.clone())
                                        .checked(*option == current)
                                        .on_select(move |window, cx| on_select(ix, window, cx));
                                    let item = match details.get(ix).filter(|d| !d.is_empty()) {
                                        Some(detail) => item.detail(detail.clone()),
                                        None => item,
                                    };
                                    menu.item(item)
                                })
                            })
                            .harness_target_named(harness)
                            .into_any_element()
                    }
                    None => dropdown.harness_target_named(harness).into_any_element(),
                }
            }
        };

        if self.inline {
            // The row that hosts an inline cycler owns the label, the cursor and its own
            // dimming; the control still says it is disabled, as a segmented control does.
            let dim = disabled && matches!(form, CyclerForm::Dropdown(_));
            return div()
                .flex()
                .flex_none()
                .items_center()
                .when(dim, |el| el.opacity(theme.metrics.dimmed_opacity))
                .child(control)
                .into_any_element();
        }

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .h_full()
            .w_full()
            .px(theme.space.md)
            // The label keeps its width; the control takes what is left and clips, so a value
            // never pushes the name of the setting out of its own row.
            .children(self.label.map(|label| {
                let text = Text::ui(label)
                    .tone(if disabled { Tone::Muted } else { Tone::Default })
                    .flex_none();
                match self.label_width {
                    Some(width) => text.w(width),
                    None => text,
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .justify_end()
                    .overflow_hidden()
                    .child(control),
            );

        super::control::cursor_row(theme, self.focused && !disabled, disabled, body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_listed_set_draws_side_by_side() {
        let cycler = Cycler::new("codex").options(["claude", "codex"]);
        assert_eq!(cycler.form(), CyclerForm::Segmented(1));
    }

    #[test]
    fn four_long_phrases_draw_as_a_dropdown() {
        let modes = [
            "asks before edits",
            "accepts edits",
            "plans before editing",
            "full access",
        ];
        let cycler = Cycler::new("full access").options(modes);
        assert_eq!(cycler.form(), CyclerForm::Dropdown(false));
        let effort = Cycler::new("High").options(["Low", "Medium", "High", "Max"]);
        assert_eq!(effort.form(), CyclerForm::Segmented(2));
    }

    #[test]
    fn a_long_set_draws_as_a_dropdown_that_lists_only_when_clickable() {
        let steps = ["1 min", "5 min", "10 min", "30 min", "1 h"];
        let drawn = Cycler::new("10 min").options(steps);
        assert_eq!(drawn.form(), CyclerForm::Dropdown(false));
        let clickable = Cycler::new("10 min").options(steps).on_select(|_, _, _| {});
        assert_eq!(clickable.form(), CyclerForm::Dropdown(true));
    }

    #[test]
    fn an_unlisted_or_off_grid_value_draws_as_a_field() {
        assert_eq!(Cycler::new("local").form(), CyclerForm::Dropdown(false));
        let off = Cycler::new("7 min")
            .options(["1 min", "5 min"])
            .off_grid(true);
        assert_eq!(off.form(), CyclerForm::Dropdown(false));
    }

    #[test]
    fn inline_changes_the_chrome_not_the_form() {
        let effort = || Cycler::new("High").options(["Low", "Medium", "High", "Max"]);
        assert_eq!(effort().inline(true).form(), effort().form());
        assert_eq!(effort().inline(true).form(), CyclerForm::Segmented(2));
        let steps = ["1 min", "5 min", "10 min", "30 min", "1 h"];
        let long = || Cycler::new("10 min").options(steps).on_select(|_, _, _| {});
        assert_eq!(long().inline(true).form(), long().form());
        assert_eq!(long().inline(true).form(), CyclerForm::Dropdown(true));
        let off = || {
            Cycler::new("7 min")
                .options(["1 min", "5 min"])
                .off_grid(true)
        };
        assert_eq!(off().inline(true).form(), off().form());
        assert!(effort().inline(true).is_inline());
        assert!(!effort().is_inline());
    }

    /// A set that reads as a list draws as a dropdown even when it is short enough to segment: a
    /// two-repository board's `none │ acme/api` would read as two states, and an `Other model
    /// id…` segment as a model.
    #[test]
    fn a_forced_dropdown_ignores_the_segment_rule() {
        let repos = || Cycler::new("acme/api").options(["none", "acme/api"]);
        assert_eq!(repos().form(), CyclerForm::Segmented(1));
        assert_eq!(repos().dropdown(true).form(), CyclerForm::Dropdown(false));
        let listed = repos().dropdown(true).on_select(|_, _, _| {});
        assert_eq!(listed.form(), CyclerForm::Dropdown(true));
        assert_eq!(
            repos().dropdown(true).inline(true).form(),
            CyclerForm::Dropdown(false)
        );
    }

    #[test]
    fn a_disabled_inline_dropdown_lists_nothing() {
        let steps = ["1 min", "5 min", "10 min", "30 min", "1 h"];
        let cycler = Cycler::new("10 min")
            .options(steps)
            .on_select(|_, _, _| {})
            .disabled(true)
            .inline(true);
        assert_eq!(cycler.form(), CyclerForm::Dropdown(false));
    }

    #[test]
    fn a_single_member_set_is_zero_suppressed() {
        let one = Cycler::new("local").has_prev(false).has_next(false);
        assert!(!one.is_visible());
        assert!(Cycler::new("local").is_visible());
    }
}
