//! `Cycler` — a closed choice in a settings row, changed with `←` / `→` (`h` / `l`).
//!
//! The keyboard is the cycler's, unchanged; the drawing picks the form that reads at a glance.
//! Given its [`Cycler::options`], a set of up to [`SEGMENTED_MAX`] draws as a
//! [`SegmentedControl`] with the value raised, and a longer set as a compact [`Dropdown`] whose
//! list opens on a click. A cycler whose caller has not listed its options (or whose value is off
//! the configured steps) draws the dropdown field alone, stating the value.
//!
//! Zero-suppression applies to the control itself: a set with fewer than two members has no
//! choice in it, so [`Cycler::is_visible`] is false and the host cycler disappears when no
//! hosts are configured.

use std::rc::Rc;

use gpui::{App, ElementId, SharedString, Window, div, prelude::*};

use super::{
    menu::{Dropdown, MenuItem},
    segmented_control::{Segment, SegmentedControl},
};
use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// The most options a cycler draws side by side; a longer set draws as a dropdown.
pub const SEGMENTED_MAX: usize = 4;

type CyclerSelect = dyn Fn(usize, &mut Window, &mut App);

/// A left/right value cycler.
#[derive(IntoElement)]
pub struct Cycler {
    id: Option<ElementId>,
    label: Option<SharedString>,
    value: SharedString,
    options: Option<Vec<SharedString>>,
    has_prev: bool,
    has_next: bool,
    focused: bool,
    disabled: bool,
    off_grid: bool,
    label_width: Option<gpui::Pixels>,
    on_select: Option<Rc<CyclerSelect>>,
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
            has_prev: true,
            has_next: true,
            focused: false,
            disabled: false,
            off_grid: false,
            label_width: None,
            on_select: None,
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

    /// A click choosing option `ix` of [`Cycler::options`]. Point it at the same update the
    /// row's `←` / `→` makes; without it the control is drawn only.
    pub fn on_select(mut self, on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(on_select));
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
        match self.position() {
            Some(ix) if listed <= SEGMENTED_MAX => CyclerForm::Segmented(ix),
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
                let control = SegmentedControl::new(id, options.into_iter().map(Segment::new))
                    .active(Some(active))
                    .disabled(disabled);
                match on_select {
                    Some(on_select) => control
                        .on_select(move |ix, window, cx| on_select(ix, window, cx))
                        .into_any_element(),
                    None => control.into_any_element(),
                }
            }
            CyclerForm::Dropdown(listed) => {
                let dropdown = Dropdown::new(id, self.value.clone()).compact();
                match on_select.filter(|_| listed) {
                    Some(on_select) => {
                        let current = self.value.clone();
                        dropdown
                            .menu(move |menu, _, _| {
                                options.iter().enumerate().fold(menu, |menu, (ix, option)| {
                                    let on_select = on_select.clone();
                                    menu.item(
                                        MenuItem::new(option.clone())
                                            .checked(*option == current)
                                            .on_select(move |window, cx| on_select(ix, window, cx)),
                                    )
                                })
                            })
                            .into_any_element()
                    }
                    None => dropdown.into_any_element(),
                }
            }
        };

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .h_full()
            .w_full()
            .px(theme.space.md)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .children(self.label.map(|label| {
                        let text = Text::ui(label).tone(if disabled {
                            Tone::Muted
                        } else {
                            Tone::Default
                        });
                        match self.label_width {
                            Some(width) => text.w(width).flex_none(),
                            None => text.ellipsize(),
                        }
                    })),
            )
            .child(div().flex_none().child(control));

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
    fn a_single_member_set_is_zero_suppressed() {
        let one = Cycler::new("local").has_prev(false).has_next(false);
        assert!(!one.is_visible());
        assert!(Cycler::new("local").is_visible());
    }
}
