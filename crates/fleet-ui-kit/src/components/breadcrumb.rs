//! `Breadcrumb` — where a drill-in sits and the way back: `‹ Columns  In review`.
//!
//! ## Purpose
//!
//! A settings pane that opens one item of a list in place (Board settings' column and schedule
//! forms) says so at its top: the parent list's name as a button that goes back, then the open
//! item's name. It replaces the mono hint line those forms used to carry. Use a
//! [`super::PaneHeader`] for a pane's own title, and a [`super::SwitcherButton`] for a crumb
//! that opens a menu of siblings rather than going back.
//!
//! ## Anatomy
//!
//! ```text
//! [ ‹ Columns ]  In review  [started]                        4 of 6
//! ```
//!
//! One `row_h` row, `sm` between parts: a compact ghost [`Button`] leading with
//! `chevron-left` and reading the parent's name, the current name in `ui_strong` (ellipsised
//! when the row is narrow), an optional [`Badge`], a flexible gap, and an optional muted caption
//! at the end (`4 of 6`, `next 14:05`).
//!
//! ## API
//!
//! [`Breadcrumb::new`] takes the id and the two names. [`Breadcrumb::back_action`] is what the
//! back button dispatches, [`Breadcrumb::badge`] follows the name, [`Breadcrumb::trailing`] is
//! the caption, and [`Breadcrumb::harness_back`] names the back button for the harness.
//!
//! ## States
//!
//! plain · with a badge · with a trailing caption · a long current name (ellipsised; the back
//! button and the caption keep their width).
//!
//! ## Pointer and keyboard (ADR 0023)
//!
//! The back button is wired with [`Button::action`]: a click dispatches the same action `Esc`
//! runs from the form, and the button's tooltip names that key from the live keymap. The crumb
//! decodes no keys itself.
//!
//! ## Usage rule
//!
//! The first line of a drill-in pane, above its cards, and nowhere else. The parent's name is
//! the rail section the user came from.

use gpui::{Action, App, ElementId, SharedString, Window, div, prelude::*};

use super::{Badge, Button, ButtonSize, ButtonStyle};
use crate::{
    harness::HarnessTargetExt as _, icons::Icon, text::Text, theme::ActiveTheme, tone::Tone,
};

/// A drill-in's `‹ Parent  Current` line. See the module doc.
#[derive(IntoElement)]
pub struct Breadcrumb {
    id: ElementId,
    parent: SharedString,
    current: SharedString,
    back_action: Option<Box<dyn Action>>,
    badge: Option<Badge>,
    trailing: Option<SharedString>,
    harness_back: Option<&'static str>,
}

impl Breadcrumb {
    /// A crumb from `parent` (the list the form was opened from) to `current` (the open item).
    pub fn new(
        id: impl Into<ElementId>,
        parent: impl Into<SharedString>,
        current: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            parent: parent.into(),
            current: current.into(),
            back_action: None,
            badge: None,
            trailing: None,
            harness_back: None,
        }
    }

    /// The action the `‹ Parent` button dispatches to the focused element; its live key rides in
    /// the button's tooltip. Without it the button is drawn but goes nowhere.
    pub fn back_action(mut self, action: Box<dyn Action>) -> Self {
        self.back_action = Some(action);
        self
    }

    /// A badge after the current name (`started`).
    pub fn badge(mut self, badge: Badge) -> Self {
        self.badge = Some(badge);
        self
    }

    /// A muted caption at the end of the row (`4 of 6`, `next 14:05`). An empty caption draws
    /// nothing.
    pub fn trailing(mut self, text: impl Into<SharedString>) -> Self {
        self.trailing = Some(text.into()).filter(|text: &SharedString| !text.is_empty());
        self
    }

    /// Name the back button for the harness target recorder.
    pub fn harness_back(mut self, name: &'static str) -> Self {
        self.harness_back = Some(name);
        self
    }

    /// The parent's name, as the back button reads.
    pub fn parent(&self) -> &SharedString {
        &self.parent
    }

    /// The caption drawn at the end of the row, if any.
    pub fn shown_trailing(&self) -> Option<&SharedString> {
        self.trailing.as_ref()
    }

    /// Whether a click on the back button goes anywhere.
    pub fn goes_back(&self) -> bool {
        self.back_action.is_some()
    }
}

impl RenderOnce for Breadcrumb {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let back = Button::new("breadcrumb-back", self.parent)
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .icon(Icon::ChevronLeft);
        let back = match self.back_action {
            Some(action) => back.action(action),
            None => back,
        };
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(theme.metrics.row_h)
            .gap(theme.space.sm)
            .child(
                div()
                    .flex_none()
                    .child(back.harness_target_named(self.harness_back)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.space.sm)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .child(Text::ui_strong(self.current).ellipsize()),
                    )
                    .children(self.badge.map(|badge| div().flex_none().child(badge))),
            )
            .children(
                self.trailing
                    .map(|text| Text::caption(text).tone(Tone::Muted).flex_none()),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext as _, Context, FocusHandle, Modifiers, Render, TestAppContext,
        VisualTestContext, point, px,
    };

    use super::*;

    gpui::actions!(breadcrumb_test, [Back]);

    #[test]
    fn an_empty_caption_is_no_caption() {
        let crumb = Breadcrumb::new("c", "Columns", "In review").trailing("");
        assert_eq!(crumb.shown_trailing(), None);
        let crumb = Breadcrumb::new("c", "Columns", "In review").trailing("4 of 6");
        assert_eq!(crumb.shown_trailing(), Some(&SharedString::from("4 of 6")));
    }

    #[test]
    fn the_back_button_reads_the_parent_and_goes_back_only_with_an_action() {
        let crumb = Breadcrumb::new("c", "Schedules", "Nightly review");
        assert_eq!(crumb.parent(), &SharedString::from("Schedules"));
        assert!(!crumb.goes_back());
        assert!(crumb.back_action(Box::new(Back)).goes_back());
    }

    struct Host {
        focus: FocusHandle,
        backs: usize,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            crate::harness::begin_frame(window);
            div()
                .key_context("BreadcrumbTest")
                .track_focus(&self.focus)
                .size_full()
                .on_action(cx.listener(|host, _: &Back, _, _| host.backs += 1))
                .child(
                    Breadcrumb::new("crumb", "Columns", "In review")
                        .badge(Badge::new("started"))
                        .trailing("4 of 6")
                        .back_action(Box::new(Back))
                        .harness_back("test.crumb.back"),
                )
        }
    }

    #[gpui::test]
    fn a_click_on_the_parent_dispatches_the_back_action(cx: &mut TestAppContext) {
        crate::harness::set_recording(true);
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    Host { focus, backs: 0 }
                })
            })
            .expect("test window")
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let back = visual
            .update(|window, _| crate::harness::painted(window))
            .into_iter()
            .find(|target| target.name == "test.crumb.back")
            .unwrap_or_else(|| panic!("the back button is painted"))
            .rect;
        crate::harness::set_recording(false);
        let at = point(px(back.x + back.w / 2.0), px(back.y + back.h / 2.0));
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();

        let host = window.root(&mut visual).expect("test host");
        host.read_with(&visual, |host, _| assert_eq!(host.backs, 1));
    }
}
