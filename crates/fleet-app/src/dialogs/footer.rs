//! The two buttons nearly every dialog footer ends in (UX-SPEC §3.8, ADR 0023).
//!
//! `Cancel` dispatches the action `esc` runs in the dialog, so it steps back exactly as the key
//! does — a dialog that leaves an editor before it closes does the same under the pointer — and
//! the primary dispatches the action its key runs. Both chips come from the live keymap.

use fleet_ui_kit::{Button, ButtonStyle};
use gpui::{Action, SharedString};

use super::Dialogs;

/// `Cancel`, running what `esc` runs in `dialog`.
pub(crate) fn cancel(dialog: &Dialogs) -> Button {
    Button::new("dialog-cancel", "Cancel").action(dialog.dismiss_action())
}

/// The dialog's one primary button, running `action` and showing its key.
pub(crate) fn primary(
    id: &'static str,
    label: impl Into<SharedString>,
    action: Box<dyn Action>,
) -> Button {
    Button::new(id, label)
        .style(ButtonStyle::Primary)
        .action(action)
}
