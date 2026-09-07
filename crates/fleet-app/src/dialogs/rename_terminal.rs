//! Rename the active Workspace terminal.

use fleet_core::ids::TerminalId;
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Dialog, Icon, KeyHintRow, TextFieldState};
use gpui::{
    AnyElement, App, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement, Window,
};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{DialogHost, field, notify, root, typed_char, with_host},
    state::AppState,
};

/// Draft for the terminal selected when the dialog opened.
#[derive(Debug, Default)]
pub struct RenameState {
    terminal: Option<TerminalId>,
    input: TextFieldState,
}

pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let selected = state.read(cx).active_session().and_then(|session| {
        let active = session.active_terminal?;
        session
            .terminals
            .iter()
            .find(|terminal| terminal.id == active)
            .map(|terminal| (terminal.id, terminal.name.clone()))
    });
    with_host(state, cx, |host| {
        host.rename_terminal =
            selected.map_or_else(RenameState::default, |(terminal, name)| RenameState {
                terminal: Some(terminal),
                input: TextFieldState::from_text(name),
            });
    });
}

pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let draft = &host.read(cx).rename_terminal;
    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    super::input::actions(
        root(focus),
        state,
        |host| &mut host.rename_terminal.input,
        notify,
    )
    .on_key_down({
        let state = state.clone();
        move |event, _window, cx| {
            if let Some(text) = typed_char(event) {
                with_host(&state, cx, |host| host.rename_terminal.input.insert(text));
                notify(&state, cx);
            }
        }
    })
    .on_action(move |_: &dialog::Confirm, _window, cx| {
        let (terminal, name) = with_host(&confirm_state, cx, |host| {
            (
                host.rename_terminal.terminal,
                host.rename_terminal.input.text().trim().to_owned(),
            )
        });
        if let Some(terminal) = terminal
            && !name.is_empty()
        {
            confirm_bridge.send(RequestBody::RenameTerminal { terminal, name });
            confirm_state.update(cx, |app, cx| {
                // §3.6: from now on this tab keeps the user's name, whatever the program
                // sets its OSC title to.
                app.mark_renamed(terminal);
                app.close_overlay();
                cx.notify();
            });
        }
    })
    .child(
        Dialog::new("Rename terminal")
            .icon(Icon::FilePen)
            .body(
                field(&draft.input)
                    .label("Name")
                    .focused(true)
                    .hide_status_line(true),
            )
            .hint_row(KeyHintRow::new().key("esc", "cancel"))
            .primary("enter  rename"),
    )
    .into_any_element()
}
