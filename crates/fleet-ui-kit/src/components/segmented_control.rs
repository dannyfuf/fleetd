//! `SegmentedControl` — two to four options side by side in a recessed trough, the chosen one
//! raised. The Hub's `Worktrees | Pull requests | Board`, the agent popup's `Claude | Codex`, and
//! every short closed choice in a settings list (drawn for it by [`super::Cycler`]).
//!
//! Use a [`super::Dropdown`] once the set outgrows four options or its labels stop fitting side
//! by side, and [`super::SegmentedTabs`] for the underlined sub-tabs inside a pane.
//!
//! The control carries no keyboard of its own: the surface that owns it binds the keys (`h`/`l`,
//! `Tab`, a prefix key), and a click goes through [`SegmentedControl::on_select`], which a
//! surface points at the same action its key dispatches. A segment may show that key as a
//! [`Kbd`] chip.

use std::rc::Rc;

use gpui::{App, ElementId, MouseButton, SharedString, Toggled, Window, div, prelude::*};

use super::kbd::{Kbd, KbdSize};
use crate::{
    harness::HarnessTargetExt as _,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// One option of a [`SegmentedControl`].
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    label: SharedString,
    icon: Option<Icon>,
    count: Option<usize>,
    loading: bool,
    kbd: Option<Kbd>,
}

impl Segment {
    /// A segment reading `label`, in sentence case as given.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            count: None,
            loading: false,
            kbd: None,
        }
    }

    /// A leading glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A trailing count. `Some(0)` renders `0`: like a tab, a segment that is empty must say
    /// so, or the missing number reads as "not loaded yet".
    pub fn count(mut self, count: Option<usize>) -> Self {
        self.count = count;
        self
    }

    /// Show `…` in place of the count while a refresh is in flight.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// The key that selects this segment, as a chip after the label. Resolve it from the live
    /// keymap ([`Kbd::for_action`]); never spell it by hand.
    pub fn kbd(mut self, kbd: Option<Kbd>) -> Self {
        self.kbd = kbd;
        self
    }

    /// The label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// What the count slot renders, if anything.
    pub fn count_text(&self) -> Option<SharedString> {
        if self.loading {
            return Some(SharedString::new_static("\u{2026}"));
        }
        self.count
            .map(|count| SharedString::from(count.to_string()))
    }
}

type SegmentSelect = dyn Fn(usize, &mut Window, &mut App);

/// A row of segments with one raised. See the module docs for when to use it.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    segments: Vec<Segment>,
    active: Option<usize>,
    disabled: bool,
    full_width: bool,
    harness_segments: Option<&'static str>,
    on_select: Option<Rc<SegmentSelect>>,
}

impl SegmentedControl {
    /// A control over `segments`, with the first one active.
    pub fn new(id: impl Into<ElementId>, segments: impl IntoIterator<Item = Segment>) -> Self {
        Self {
            id: id.into(),
            segments: segments.into_iter().collect(),
            active: Some(0),
            disabled: false,
            full_width: false,
            harness_segments: None,
            on_select: None,
        }
    }

    /// Which segment is raised. `None` raises none: a persisted value outside the set.
    pub fn active(mut self, active: Option<usize>) -> Self {
        self.active = active;
        self
    }

    /// Draw the whole control dimmed and ignore the pointer.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Stretch across the container, the segments sharing the width equally.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// A click on segment `ix`. Clicking the active segment calls it too; a surface for which
    /// that would do something (a toggle key) ignores the index it already shows.
    pub fn on_select(mut self, on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Name each segment `<part>[<index>]` for the harness target recorder (`hub.tab`,
    /// `agents.popup.agent`). A control no scenario addresses leaves this unset.
    pub fn harness_segments(mut self, part: &'static str) -> Self {
        self.harness_segments = Some(part);
        self
    }

    /// How many segments there are.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether the control has no segments.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let active = self.active;
        let clickable = self.on_select.is_some() && !self.disabled;
        let full_width = self.full_width;
        let harness = self.harness_segments;
        let hover = colors.row_hover;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xxs)
            .p(theme.space.xxs)
            .rounded(theme.radii.control)
            .bg(colors.chrome)
            .border(theme.metrics.hairline)
            .border_color(colors.border)
            .when(full_width, |el| el.w_full())
            .when(self.disabled, |el| el.opacity(theme.metrics.dimmed_opacity))
            .role(gpui::Role::RadioGroup)
            .children(self.segments.into_iter().enumerate().map(|(ix, segment)| {
                let is_active = active == Some(ix);
                let count = segment.count_text();
                let tone = if is_active {
                    Tone::Default
                } else {
                    Tone::Secondary
                };
                let count_tone = if segment.loading {
                    Tone::Warning
                } else {
                    Tone::Muted
                };
                let on_select = self.on_select.clone().filter(|_| clickable);
                let name = segment.label.clone();
                div()
                    .id(("segment", ix))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(theme.space.sm)
                    .h(theme.metrics.segment_h)
                    .px(theme.space.md)
                    .when(full_width, |el| el.flex_1().min_w_0())
                    .rounded(theme.radii.md)
                    // Every segment carries the hairline, clear unless raised, so raising one
                    // never shifts its neighbours.
                    .border(theme.metrics.hairline)
                    .border_color(if is_active {
                        colors.control_border
                    } else {
                        gpui::transparent_black()
                    })
                    .when(is_active, |el| el.bg(colors.control))
                    .role(gpui::Role::RadioButton)
                    .aria_label(name)
                    .aria_toggled(if is_active {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .when_some(on_select, |el, on_select| {
                        el.cursor_pointer()
                            .when(!is_active, |el| el.hover(move |style| style.bg(hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, window, cx| {
                                on_select(ix, window, cx);
                                cx.stop_propagation();
                            })
                    })
                    .children(segment.icon.map(|icon| {
                        icon.el().size(IconSize::Small).tone(if is_active {
                            Tone::Default
                        } else {
                            Tone::Secondary
                        })
                    }))
                    .child(
                        Text::ui(segment.label)
                            .tone(tone)
                            .weight(theme.text.ui_strong.weight)
                            .ellipsize(),
                    )
                    .children(count.map(|count| Text::caption(count).tone(count_tone)))
                    .children(segment.kbd.map(|kbd| kbd.size(KbdSize::Small)))
                    .harness_target_optional(harness.map(|part| (part, ix)))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_segment_still_says_zero() {
        assert_eq!(
            Segment::new("Board").count(Some(0)).count_text().as_deref(),
            Some("0")
        );
        assert_eq!(Segment::new("Worktrees").count_text(), None);
    }

    #[test]
    fn loading_replaces_the_count_with_an_ellipsis() {
        let segment = Segment::new("Board").count(Some(4)).loading(true);
        assert_eq!(segment.count_text().as_deref(), Some("\u{2026}"));
    }
}
