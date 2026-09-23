//! §3.8.7 Help (`?`) — a guide: what you can do here, how to get a task done, and every key.
//!
//! Help opens on a search field and two tabs. **Guides** lists the six most useful actions of
//! the surface it was opened over ("Here in Worktrees") and the task guides of [`guides`];
//! **All shortcuts** is the whole action catalogue by place. Every row and every guide step's
//! button runs its action: Help closes, and the action is dispatched to the surface behind it
//! once that surface has the keyboard back, exactly as its key would be ([`run`]).
//!
//! The draft lives on the [`super::DialogHost`] for as long as Help is open; everything it shows
//! is prepared in [`seed`] and in the update paths below, never in `render`
//! (`docs/APP-CONTRACTS.md`).

mod guides;
mod model;
mod view;

use fleet_ui_kit::{InputMode, TextInput, TextInputEvent};
use gpui::{App, AppContext as _, Entity};

pub(super) use model::HelpState;

use super::{notify, with_host};
use crate::state::AppState;

/// The wire protocol this build speaks, for the Settings About section and Help's footer.
#[must_use]
pub(super) const fn protocol() -> u32 {
    fleet_proto::PROTOCOL_VERSION
}

/// The search field's placeholder: the question Help answers, and three words that find
/// something.
const PLACEHOLDER: &str = "What do you want to do?  Try \u{201c}agent\u{201d}, \
                           \u{201c}delete\u{201d}, \u{201c}copy\u{201d}";

/// Prepares Help for one opening: what runs where it was opened, and the search field.
///
/// Called on the overlay transition, when `AppState.overlay` already says Help; the surface
/// Help describes is the one under it, [`AppState::base_context_chain`].
pub(super) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let chain = state.read(cx).base_context_chain();
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_icon(Some(fleet_ui_kit::Icon::Search), cx);
        input.set_placeholder(PLACEHOLDER, cx);
        // The header row is the field's whole height: there is no status to report under it.
        input.set_hide_status_line(true, cx);
        input
    });
    // Weak, like every dialog editor's subscription: the handle lives on `DialogHost`, which
    // `AppState` owns, so a strong capture here would be a cycle holding the app alive.
    let watched = state.downgrade();
    let subscription = cx.subscribe(&input, move |input, event: &TextInputEvent, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(watched) = watched.upgrade() else {
            return;
        };
        let query = input.read(cx).text().to_owned();
        with_host(&watched, cx, |host| {
            if let Some(help) = host.help.as_mut() {
                help.set_query(query);
            }
        });
        notify(&watched, cx);
    });
    with_host(state, cx, |host| {
        host.help = Some(HelpState::new(&chain));
        host.help_input = Some(input);
        host.help_input_subscription = Some(subscription);
    });
}

/// Closes Help and runs `action` on the surface it was opened over.
///
/// The action is not dispatched here: Help's own element still holds the keyboard, and an
/// action dispatched from it would reach the Shell's listeners but none of the surface's. The
/// shell dispatches it once the frame that gave the surface its keyboard back has painted
/// (`shell/root/focus.rs`), which is where the action's key would have landed.
pub(super) fn run(state: &Entity<AppState>, action: &'static str, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.close_overlay();
        app.pending_action = Some(action);
        app.harness.set_pending_frame(true);
        cx.notify();
    });
}

pub(super) use view::render;
