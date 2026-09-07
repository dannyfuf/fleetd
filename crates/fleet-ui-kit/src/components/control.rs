//! Presentation every settings control shares: the cursor-row band it sits in and the
//! clickable semantics it gains once a caller can activate it.

use gpui::{
    App, Div, InteractiveElement, StatefulInteractiveElement, Styled, Window, div, prelude::*,
};

use crate::{focus::FocusRing, theme::Theme};

/// The `row_h` band a labelled control occupies in a settings list: the cursor background and
/// the focus bar while it is the cursor, dimmed as a whole while it is disabled.
pub(super) fn cursor_row(
    theme: &Theme,
    focused: bool,
    disabled: bool,
    body: impl IntoElement,
) -> Div {
    div()
        .w_full()
        .h(theme.metrics.row_h)
        .when(focused, |el| el.bg(theme.colors.row_selected))
        .when(disabled, |el| el.opacity(theme.metrics.dimmed_opacity))
        .child(FocusRing::cursor_row(focused).content(body))
}

/// Make `element` activatable by pointer, with the role and cursor that implies.
pub(super) fn on_activate<E: InteractiveElement + StatefulInteractiveElement + Styled>(
    element: E,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> E {
    element
        .role(gpui::Role::Button)
        .cursor_pointer()
        .on_click(move |_, window, cx| activate(window, cx))
}
