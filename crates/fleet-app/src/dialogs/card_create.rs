//! New card (BOARD §8) — *a title is enough; everything else has a key of its own.*
//!
//! Two fields and two ways out: `Enter` creates and closes, `ctrl-Enter` creates and opens the
//! card it made. Nothing else is asked for here — status, priority, labels and the rest all
//! have a one-letter picker on the board, and a create dialog that asks for them up front is a
//! form, not a keystroke.

use fleet_core::{board::CardDraft, ids::BoardId};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{board as board_actions, dialog},
    bridge::Bridge,
    dialogs::{DialogHost, Dialogs, host::complete_request, notify, read_host, root, with_host},
    screens::board,
    state::AppState,
};

/// Which field of the create dialog owns the keyboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Field {
    /// The one-line title.
    #[default]
    Title,
    /// The optional markdown description.
    Description,
}

/// Draft for a new card on the current board.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardCreateState {
    /// Identity of this opening, used to ignore replies to discarded drafts.
    pub(super) generation: u64,
    /// A request is in flight; retain and freeze the draft until it answers.
    pub(super) saving: bool,
    /// Destination board.
    pub(super) board_id: Option<BoardId>,
    /// Which field owns the keyboard.
    pub(super) field: Field,
    /// The exact message from a refused create.
    pub(super) error: Option<String>,
}

impl CardCreateState {
    /// Completes only the request belonging to this opening.
    pub(super) fn finish_save(&mut self, generation: u64, error: Option<String>) -> bool {
        if self.generation != generation || !self.saving {
            return false;
        }
        self.saving = false;
        self.error = error;
        self.error.is_none()
    }

    /// Whether `Enter` may create: a card is its title, so a blank one is not a card.
    #[must_use]
    pub(super) fn can_submit(&self, title: &str) -> bool {
        !self.saving && self.board_id.is_some() && !title.trim().is_empty()
    }
}

/// Opens an empty draft against the current board.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let board_id = state.read(cx).board().map(|view| view.board.id.clone());
    let title = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_label(Some("Title".into()), cx);
        input.set_placeholder("Fix the login redirect", cx);
        input
    });
    let description = cx.new(|cx| {
        let mut input = TextInput::new(
            InputMode::Multiline {
                min_rows: 6,
                max_rows: 6,
            },
            cx,
        );
        input.set_label(Some("Description".into()), cx);
        input.set_placeholder("Markdown. Optional.", cx);
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    let weak_host = host.downgrade();
    let weak_state = state.downgrade();
    let title_subscription = cx.subscribe(&title, move |input, event, cx| {
        if matches!(event, TextInputEvent::Focused) {
            if let Some(state) = weak_state.upgrade() {
                claim_field(&state, Field::Title, cx);
            }
            return;
        }
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let input = input.clone();
        // `Changed` is emitted from inside the editor's own update. Clear validation on the next
        // update turn so the subscriber never attempts to update the entity already on the stack.
        cx.defer(move |cx| input.update(cx, |input, cx| input.set_invalid(None, cx)));
        if let Some(host) = weak_host.upgrade() {
            host.update(cx, |host, cx| {
                if host.card_create.error.take().is_some() {
                    cx.notify();
                }
            });
        }
    });
    let weak_host = host.downgrade();
    let weak_state = state.downgrade();
    let description_subscription = cx.subscribe(&description, move |_, event, cx| {
        if matches!(event, TextInputEvent::Focused) {
            if let Some(state) = weak_state.upgrade() {
                claim_field(&state, Field::Description, cx);
            }
            return;
        }
        if matches!(event, TextInputEvent::Changed)
            && let Some(host) = weak_host.upgrade()
        {
            host.update(cx, |host, cx| {
                if host.card_create.error.take().is_some() {
                    cx.notify();
                }
            });
        }
    });
    host.update(cx, |host, _| {
        host.card_create = CardCreateState {
            generation: host.card_create.generation.wrapping_add(1),
            board_id,
            ..Default::default()
        };
        host.card_create_title = Some(title);
        host.card_create_description = Some(description);
        host.card_create_input_subscriptions = vec![title_subscription, description_subscription];
    });
}

/// Renders the two fields and their two ways out.
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let (draft, title, description) = read_host(state, cx, |host, _| {
        (
            host.card_create.clone(),
            host.card_create_title.clone(),
            host.card_create_description.clone(),
        )
    });
    let (Some(title), Some(description)) = (title, description) else {
        return root(focus).into_any_element();
    };
    // The marker mirrors focus (`claim_field`), so it is the one answer to "which field owns
    // the keyboard" — the hint row and the shell's reconciliation read the same value.
    let field = draft.field;
    let board_name = state
        .read(cx)
        .board()
        .map(|view| format!("\u{00b7} {}", view.board.name));

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(title.clone().harness_target_indexed("dialog.field", 0))
        .child(
            description
                .clone()
                .harness_target_indexed("dialog.field", 1),
        );

    let mut card = Dialog::new("New card")
        .dismiss_action(crate::dialogs::Dialogs::CardCreate.dismiss_action())
        .icon(Icon::Plus)
        .width(Dialogs::CardCreate.width(cx))
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key(
                    "\u{21e5}",
                    if field == Field::Description {
                        "title"
                    } else {
                        "description"
                    },
                )
                .key(
                    "\u{21e7}tab",
                    if field == Field::Description {
                        "title"
                    } else {
                        "description"
                    },
                )
                .key("\u{2303}\u{23ce}", "create & open")
                .key("esc", "cancel"),
        )
        .primary(if field == Field::Description {
            "\u{2303}\u{21b5} Create & open"
        } else {
            "\u{21b5} Create"
        });
    if let Some(name) = board_name {
        card = card.subtitle(name);
    }
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let submit_state = state.clone();
    let submit_bridge = bridge.clone();
    let open_state = state.clone();
    let open_bridge = bridge.clone();

    root(focus)
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, window, cx| cycle_field(&state, window, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, window, cx| cycle_field(&state, window, cx)
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            submit(false, &submit_state, &submit_bridge, cx);
            cx.stop_propagation();
        })
        .on_action(move |_: &board_actions::CreateAndOpen, _window, cx| {
            submit(true, &open_state, &open_bridge, cx);
            cx.stop_propagation();
        })
        .child(card)
        .into_any_element()
}

/// Mirrors the editor that just took focus into the marker the shell reconciles against.
///
/// The field marker is what `dialogs::focused_input` names, and a click focuses an editor
/// without asking this dialog, so without this the next `AppState` notify would move the caret
/// back to the marked field.
fn claim_field(state: &Entity<AppState>, field: Field, cx: &mut App) {
    let changed = with_host(state, cx, |host| {
        let changed = host.card_create.field != field;
        host.card_create.field = field;
        changed
    });
    if changed {
        notify(state, cx);
    }
}

/// `Tab` / `S-Tab`: two fields, so both keys do the same thing.
fn cycle_field(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let title_focused = read_host(state, cx, |host, _| host.card_create_title.clone())
        .is_some_and(|input| input.read(cx).focus_handle().is_focused(window));
    let input = with_host(state, cx, |host| {
        host.card_create.field = if title_focused {
            Field::Description
        } else {
            Field::Title
        };
        match host.card_create.field {
            Field::Title => host.card_create_title.clone(),
            Field::Description => host.card_create_description.clone(),
        }
    });
    if let Some(input) = input {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    notify(state, cx);
    cx.stop_propagation();
}

/// Creates the card; `open_after` also opens its detail once the daemon answers.
fn submit(open_after: bool, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let current_board = state.read(cx).board().map(|view| view.board.id.clone());
    let (title_input, description_input) = read_host(state, cx, |host, _| {
        (
            host.card_create_title.clone(),
            host.card_create_description.clone(),
        )
    });
    let (Some(title_input), Some(description_input)) = (title_input, description_input) else {
        return;
    };
    let title = title_input.read(cx).text().to_owned();
    let description = description_input.read(cx).text().to_owned();
    let Some((board_id, draft, generation)) = with_host(state, cx, |host| {
        let draft = &mut host.card_create;
        if draft.saving || draft.board_id.is_none() || draft.board_id != current_board {
            return None;
        }
        if !draft.can_submit(&title) {
            return None;
        }
        draft.saving = true;
        draft.error = None;
        let fields = CardDraft {
            title: title.trim().to_owned(),
            description,
            ..CardDraft::default()
        };
        Some((draft.board_id.clone()?, fields, draft.generation))
    }) else {
        title_input.update(cx, |input, cx| {
            input.set_invalid(Some("A card needs a title.".into()), cx)
        });
        notify(state, cx);
        return;
    };
    title_input.update(cx, |input, cx| input.set_read_only(true, cx));
    description_input.update(cx, |input, cx| input.set_read_only(true, cx));
    let reply = bridge.request(RequestBody::CreateCard {
        board_id: board_id.clone(),
        draft,
    });
    notify(state, cx);
    complete_request(state, cx, async move |state, cx| {
        let answer = match reply.recv().await {
            Ok(Ok(ResponseBody::Card(card))) => Ok(card),
            Ok(Err(error)) => Err(error.message),
            Ok(Ok(_)) => Err("Unexpected create response".into()),
            Err(_) => Err("Daemon disconnected before replying".into()),
        };
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            if state.read(cx).overlay != Some(crate::state::Overlay::Dialog(Dialogs::CardCreate))
                || !state
                    .read(cx)
                    .board()
                    .is_some_and(|view| view.board.id == board_id)
            {
                return;
            }
            let (current, completed) = with_host(&state, cx, |host| {
                let current = host.card_create.generation == generation && host.card_create.saving;
                let completed = host
                    .card_create
                    .finish_save(generation, answer.as_ref().err().cloned());
                (current, completed)
            });
            if current && !completed {
                let inputs = read_host(&state, cx, |host, _| {
                    (
                        host.card_create_title.clone(),
                        host.card_create_description.clone(),
                    )
                });
                for input in [inputs.0, inputs.1].into_iter().flatten() {
                    input.update(cx, |input, cx| input.set_read_only(false, cx));
                }
            }
            if completed && let Ok(card) = answer {
                let id = card.id.clone();
                state.update(cx, |app, cx| {
                    app.apply_card(card);
                    board::focus_card(app, &id);
                    app.close_overlay();
                    app.toast_short(
                        "\u{2713} card created",
                        Icon::Boxes,
                        std::time::Instant::now(),
                    );
                    cx.notify();
                });
                if open_after {
                    board::open_dialog(&state, Dialogs::CardDetail, cx);
                }
            }
            notify(&state, cx);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InputHarness {
        inputs: Vec<Entity<TextInput>>,
    }

    impl gpui::Render for InputHarness {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div().children(self.inputs.clone())
        }
    }

    fn drive_input(
        cx: &mut gpui::TestAppContext,
        inputs: Vec<Entity<TextInput>>,
    ) -> gpui::VisualTestContext {
        let input = inputs
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("at least one input"));
        let window = cx.add_window(|_, _| InputHarness { inputs });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
        visual
    }

    fn draft() -> CardCreateState {
        CardCreateState {
            board_id: Some("work".parse().unwrap_or_else(|error| panic!("{error}"))),
            ..CardCreateState::default()
        }
    }

    #[test]
    fn a_card_needs_a_title_and_a_board() {
        let mut state = draft();
        assert!(!state.can_submit(""));
        assert!(!state.can_submit("   "), "whitespace is not a title");
        assert!(state.can_submit("Fix login"));
        state.board_id = None;
        assert!(!state.can_submit("Fix login"));
    }

    #[gpui::test]
    fn live_card_create_inputs_own_word_selection_undo_and_multiline_edits(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(fleet_ui_kit::Theme::dark());
            crate::keymap::init(cx);
        });
        let state = cx.new(|_| AppState::new("/tmp/card-create-input", std::time::Instant::now()));
        cx.update(|cx| seed(&state, cx));
        let (title, description) = cx.update(|cx| {
            read_host(&state, cx, |host, _| {
                (
                    host.card_create_title
                        .clone()
                        .unwrap_or_else(|| panic!("title input")),
                    host.card_create_description
                        .clone()
                        .unwrap_or_else(|| panic!("description input")),
                )
            })
        });

        let mut visual = drive_input(cx, vec![title.clone(), description.clone()]);
        visual.simulate_input("alpha beta");
        visual.simulate_keystrokes("alt-backspace");
        title.read_with(&visual, |input, _| assert_eq!(input.text(), "alpha "));
        visual.simulate_input("beta");
        visual.simulate_keystrokes("shift-left shift-left");
        visual.simulate_input("X");
        title.read_with(&visual, |input, _| assert_eq!(input.text(), "alpha beX"));
        visual.simulate_keystrokes("cmd-z");
        title.read_with(&visual, |input, _| assert_eq!(input.text(), "alpha beta"));

        visual.update(|window, cx| {
            description.update(cx, |input, cx| input.focus(window, cx));
        });
        visual.simulate_input("first");
        visual.simulate_keystrokes("enter");
        visual.simulate_input("second");
        visual.simulate_keystrokes("cmd-backspace");
        description.read_with(&visual, |input, _| assert_eq!(input.text(), "first\n"));
    }
}
