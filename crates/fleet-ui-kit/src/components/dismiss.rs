//! What closing a floating surface does: the one path a [`super::Dialog`], a [`super::Sheet`]
//! and an [`super::Overlay`] share between their close ✕, a click on their scrim, and the key.
//!
//! ADR 0023: "clicking outside a dialog or sheet closes it through the same cancel action as
//! `esc`". So the normal wiring is [`Dismiss::Action`]: the pointer dispatches the surface's
//! existing cancel action to the focused element, exactly as the key would, and the ✕ shows the
//! key the live keymap gives that action. A dialog that cancels in stages (leave the editor
//! first, then close) therefore steps the same way under the pointer as under `esc`.
//! [`Dismiss::Handler`] is for a surface whose close is not an action, and for tests.

use std::rc::Rc;

use gpui::{Action, App, ElementId, Window};

use super::button::{Button, IconButton};
use crate::icons::Icon;

/// A dismiss callback. `Rc` because one surface hands it to two elements, its ✕ and its scrim.
pub(super) type DismissFn = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// How a floating surface closes when the pointer asks it to.
pub(super) enum Dismiss {
    /// Dispatch this action to the focused element, as its key does.
    Action(Box<dyn Action>),
    /// Run this handler.
    Handler(DismissFn),
}

impl Dismiss {
    /// The callback a scrim click runs.
    pub(super) fn callback(&self) -> DismissFn {
        match self {
            Dismiss::Action(action) => {
                let action = action.boxed_clone();
                Rc::new(move |window, cx| window.dispatch_action(action.boxed_clone(), cx))
            }
            Dismiss::Handler(handler) => handler.clone(),
        }
    }

    /// The header's close ✕. An action-backed one shows the action's live key in its tooltip.
    pub(super) fn close_button(&self, id: impl Into<ElementId>) -> IconButton {
        let button = IconButton::new(id, Icon::X, "Close");
        match self {
            Dismiss::Action(action) => button.action(action.boxed_clone()),
            Dismiss::Handler(handler) => {
                let handler = handler.clone();
                button.on_click(move |_, window, cx| handler(window, cx))
            }
        }
    }
}

impl Dismiss {
    /// A footer `Cancel` button that closes the way the ✕ does, showing the action's live key.
    pub(super) fn cancel_button(&self, id: impl Into<ElementId>) -> Button {
        let button = Button::new(id, "Cancel");
        match self {
            Dismiss::Action(action) => button.action(action.boxed_clone()),
            Dismiss::Handler(handler) => {
                let handler = handler.clone();
                button.on_click(move |_, window, cx| handler(window, cx))
            }
        }
    }
}

/// The two builders every dismissable surface exposes, spelled once. The surface keeps a
/// `dismiss: Option<Dismiss>` field.
macro_rules! dismiss_builders {
    () => {
        /// Close through `action`: the ✕ and a click outside dispatch it to the focused element
        /// exactly as its key does, and the ✕'s tooltip shows that key from the live keymap.
        /// Pass the action `esc` runs on this surface, so key and pointer share one path.
        pub fn dismiss_action(mut self, action: Box<dyn gpui::Action>) -> Self {
            self.dismiss = Some(super::dismiss::Dismiss::Action(action));
            self
        }

        /// Close through `handler`, for a surface whose close is not an action. Setting either
        /// this or [`Self::dismiss_action`] draws the close ✕ and arms the click outside.
        pub fn on_dismiss(
            mut self,
            handler: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
        ) -> Self {
            self.dismiss = Some(super::dismiss::Dismiss::Handler(std::rc::Rc::new(handler)));
            self
        }
    };
}

pub(super) use dismiss_builders;
