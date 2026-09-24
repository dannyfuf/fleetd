//! `TitleBar` and `CommandField` — the one 44 px row at the top of every Fleet window.
//!
//! The title bar is the unified macOS titlebar, so it starts after the traffic lights
//! (`metrics.traffic_light_inset`) and paints the `chrome` ground. It has three named regions,
//! weighted to the left:
//!
//! ```text
//! [inset][ leading …                          ][ command field ][ trailing ]
//! ```
//!
//! - **leading**: where you are. In the Hub, the context switcher and the section nav; in the
//!   Workspace, the breadcrumb `‹ / repo / worktree ⌄` and what git knows about it. It takes all
//!   the width the other two leave, and clips rather than wraps.
//! - **command**: the [`CommandField`], right-aligned against the trailing region at its fixed
//!   `command_field_w`, so it never moves while the leading region changes.
//! - **trailing**: what needs you and what is running (each a [`super::StatusButton`] shown only
//!   while its count is non-zero), then Help and Settings as [`super::IconButton`]s. Never
//!   shrinks.
//!
//! Where you are is what a narrow window must keep longest, so only the leading region yields
//! width: a narrow window loses the end of the breadcrumb, never the field or a count.
//!
//! Use it once, as [`super::AppFrame::title_bar`]; the frame owns the height. A floating window of
//! its own (the agent popup) draws a header of its own, not a second title bar.

use gpui::{Action, AnyElement, App, ElementId, Pixels, SharedString, Window, div, prelude::*};

use super::{
    control,
    kbd::{Kbd, KbdSize},
};
use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
};

/// The window's top row. See the module doc.
#[derive(IntoElement)]
pub struct TitleBar {
    leading_inset: Option<Pixels>,
    leading: Vec<AnyElement>,
    command: Option<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl TitleBar {
    /// An empty bar: the chrome ground and its hairline, nothing in it. The first-run screen shows
    /// exactly this, because there is nowhere to go yet.
    pub fn new() -> Self {
        Self {
            leading_inset: None,
            leading: Vec::new(),
            command: None,
            trailing: Vec::new(),
        }
    }

    /// Clear this much at the left edge before the leading region: the macOS traffic lights.
    /// Defaults to the `md` gutter.
    pub fn leading_inset(mut self, inset: Pixels) -> Self {
        self.leading_inset = Some(inset);
        self
    }

    /// Append an element to the leading region, left to right.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading.push(element.into_any_element());
        self
    }

    /// The element right of the leading region, against the trailing one: normally a
    /// [`CommandField`]. It keeps its own width.
    pub fn command(mut self, element: impl IntoElement) -> Self {
        self.command = Some(element.into_any_element());
        self
    }

    /// Append an element to the trailing region, left to right; the region is right-aligned.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }
}

impl Default for TitleBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for TitleBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .size_full()
            .pl(self.leading_inset.unwrap_or(theme.space.md))
            .pr(theme.space.md)
            .bg(theme.colors.chrome)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.space.md)
                    .overflow_hidden()
                    .children(self.leading),
            )
            .children(
                self.command
                    .map(|command| div().flex_none().pl(theme.space.lg).child(command)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .pl(theme.space.sm)
                    .gap(theme.space.xs)
                    .children(self.trailing),
            )
    }
}

/// A button drawn as a search field: a magnifier, `Search or run a command`, and the key chip.
///
/// It is **not** an input: a click dispatches [`Self::action`] (the palette's open action), the
/// palette takes the keyboard, and the typing happens there. Drawing it as a field is what tells
/// a person there is somewhere to type; making it a real input would put a second, weaker palette
/// on screen. Like every [`super::Button`] it is not focusable (ADR 0023).
///
/// Its chip is one of the few a face keeps (DESIGN-SYSTEM §4): the palette reaches everything,
/// so its key is the one worth teaching. Pass it with [`Self::kbd`], read from the key table
/// rather than from focus: the palette, a popover or a dialog takes the focus away from where
/// the key is bound, and the chip must not vanish under it. The field is `command_field_w` wide
/// whatever its chip, so nothing beside it moves either way.
#[derive(IntoElement)]
pub struct CommandField {
    id: ElementId,
    placeholder: SharedString,
    action: Option<Box<dyn Action>>,
    kbd: Option<Kbd>,
}

impl CommandField {
    /// A field reading `placeholder`.
    pub fn new(id: impl Into<ElementId>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            placeholder: placeholder.into(),
            action: None,
            kbd: None,
        }
    }

    /// Dispatch `action` to the focused element on click. Without [`Self::kbd`] the chip is its
    /// live key binding in the focused context.
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(action);
        self
    }

    /// Show this key instead of the one [`Self::action`] resolves: the caller's key-table
    /// spelling, which does not depend on focus. Never a hand-typed string for a key the keymap
    /// owns.
    pub fn kbd(mut self, kbd: Kbd) -> Self {
        self.kbd = Some(kbd);
        self
    }
}

impl RenderOnce for CommandField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kbd = self.kbd.or_else(|| {
            self.action
                .as_deref()
                .and_then(|action| Kbd::for_action(action, window, cx))
        });
        let theme = cx.theme();
        let hover = theme.colors.control_hover;
        let action = self.action;
        let field = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.sm)
            .w(theme.metrics.command_field_w)
            .h(theme.metrics.button_h)
            .px(theme.space.sm)
            .rounded(theme.radii.control)
            .bg(theme.colors.control)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.control_border)
            .hover(move |style| style.bg(hover))
            .when_some(kbd.as_ref().and_then(Kbd::aria_shortcut), |el, shortcut| {
                el.aria_keyshortcuts(shortcut)
            })
            .child(
                Icon::Search
                    .el()
                    .size(IconSize::Small)
                    .color(theme.colors.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Text::ui(self.placeholder.clone()).muted().ellipsize()),
            )
            .children(kbd.map(|kbd| kbd.size(KbdSize::Small)));
        control::on_click_named(field, self.placeholder, move |_, window, cx| {
            if let Some(action) = &action {
                window.dispatch_action(action.boxed_clone(), cx);
            }
        })
    }
}
