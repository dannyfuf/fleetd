//! Rename the active Workspace terminal.

use fleet_core::ids::TerminalId;
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Dialog, Icon, KeyHintRow, TextField};
use gpui::{
    AnyElement, App, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement, Window,
};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{TextInput, notify, root, typed_char, with_host},
    state::AppState,
};

/// Draft for the terminal selected when the dialog opened.
#[derive(Debug, Clone, Default)]
pub struct RenameState {
    terminal: Option<TerminalId>,
    input: TextInput,
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
    with_host(cx, |host| {
        host.rename_terminal =
            selected.map_or_else(RenameState::default, |(terminal, name)| RenameState {
                terminal: Some(terminal),
                input: TextInput::new(name),
            });
    });
}

pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let draft = with_host(cx, |host| host.rename_terminal.clone());
    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                if let Some(text) = typed_char(event) {
                    with_host(cx, |host| host.rename_terminal.input.insert(&text));
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                with_host(cx, |host| host.rename_terminal.input.backspace());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                with_host(cx, |host| host.rename_terminal.input.delete_word());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                with_host(cx, |host| host.rename_terminal.input.clear());
                notify(&state, cx);
            }
        })
        // §KEYMAP "Dialogs and text inputs": the caret keys are bound on the shared `Dialog`
        // context, so without these listeners they are consumed and do nothing.
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                with_host(cx, |host| host.rename_terminal.input.home());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                with_host(cx, |host| host.rename_terminal.input.end());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                with_host(cx, |host| host.rename_terminal.input.left());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                with_host(cx, |host| host.rename_terminal.input.right());
                notify(&state, cx);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            let (terminal, name) = with_host(cx, |host| {
                (
                    host.rename_terminal.terminal,
                    host.rename_terminal.input.value().trim().to_owned(),
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
                    TextField::new(draft.input.value().to_owned())
                        .label("Name")
                        .caret(draft.input.caret())
                        .focused(true)
                        .hide_status_line(true),
                )
                .hint_row(KeyHintRow::new().key("esc", "cancel"))
                .primary("enter  rename"),
        )
        .into_any_element()
}
