//! Rename the active Workspace terminal.

use fleet_core::ids::TerminalId;
use fleet_proto::{request::RequestBody, response::ResponseBody};
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

type RenameReply = async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>>;
type RenameRequest = std::rc::Rc<dyn Fn(RequestBody) -> RenameReply>;

/// Draft for the terminal selected when the dialog opened.
#[derive(Debug, Default)]
pub struct RenameState {
    terminal: Option<TerminalId>,
    input: TextFieldState,
    error: Option<String>,
    in_flight: bool,
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
                error: None,
                in_flight: false,
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
    let bridge = bridge.clone();
    render_with_request(
        state,
        std::rc::Rc::new(move |body| bridge.request(body)),
        focus,
        host,
        cx,
    )
}

fn render_with_request(
    state: &Entity<AppState>,
    request: RenameRequest,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    cx: &mut App,
) -> AnyElement {
    let (input, error, in_flight) = {
        let draft = &host.read(cx).rename_terminal;
        (draft.input.clone(), draft.error.clone(), draft.in_flight)
    };
    let confirm_state = state.clone();
    let confirm_request = request.clone();
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
        let request = with_host(&confirm_state, cx, |host| {
            if host.rename_terminal.in_flight {
                return None;
            }
            host.rename_terminal.error = None;
            host.rename_terminal.in_flight = true;
            (
                host.rename_terminal.terminal,
                host.rename_terminal.input.text().trim().to_owned(),
            )
                .into()
        });
        let Some((Some(terminal), name)) = request else {
            return;
        };
        if name.is_empty() {
            with_host(&confirm_state, cx, |host| {
                host.rename_terminal.in_flight = false
            });
            return;
        }
        let reply = confirm_request(RequestBody::RenameTerminal { terminal, name });
        let weak_state = confirm_state.downgrade();
        let task = cx.spawn(async move |cx| {
            let answer = reply.recv().await;
            cx.update(|cx| {
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                match answer {
                    Ok(Ok(body)) if rename_accepted(terminal, &body) => {
                        state.update(cx, |app, cx| {
                            app.mark_renamed(terminal);
                            app.close_overlay();
                            cx.notify();
                        });
                    }
                    Ok(Err(error)) => rename_failed(&state, error.message, cx),
                    Ok(Ok(_)) | Err(_) => rename_failed(
                        &state,
                        "rename: the daemon did not acknowledge the terminal".to_owned(),
                        cx,
                    ),
                }
            });
        });
        crate::dialogs::retain_task(&confirm_state, cx, "rename-terminal", task);
    })
    .child({
        let mut dialog = Dialog::new("Rename terminal")
            .icon(Icon::FilePen)
            .body(
                field(&input)
                    .label("Name")
                    .focused(true)
                    .hide_status_line(true),
            )
            .hint_row(KeyHintRow::new().key("esc", "cancel"))
            .primary(if in_flight {
                "renaming…"
            } else {
                "enter  rename"
            });
        if let Some(error) = error {
            dialog = dialog.error(error);
        }
        dialog
    })
    .into_any_element()
}

fn rename_accepted(expected: TerminalId, response: &ResponseBody) -> bool {
    matches!(response, ResponseBody::Terminal(terminal) if terminal.id == expected)
}

fn rename_failed(state: &Entity<AppState>, message: String, cx: &mut App) {
    with_host(state, cx, |host| {
        host.rename_terminal.in_flight = false;
        host.rename_terminal.error = Some(message);
    });
    notify(state, cx);
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Instant};

    use fleet_core::sessions::{Terminal, TerminalKind, TerminalStatus};
    use fleet_proto::error::{ErrorKind, ProtoError};
    use gpui::{AppContext, Context, Render, div};

    use super::*;

    struct RenameFixture {
        state: Entity<AppState>,
        host: Entity<DialogHost>,
        focus: FocusHandle,
        request: RenameRequest,
    }

    impl Render for RenameFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().key_context("Dialog").child(render_with_request(
                &self.state,
                self.request.clone(),
                &self.focus,
                &self.host,
                cx,
            ))
        }
    }

    fn terminal_record(id: TerminalId) -> Terminal {
        Terminal {
            id,
            name: "renamed".to_owned(),
            command: "zsh".to_owned(),
            cwd: "/tmp".to_owned(),
            shell_pid: None,
            foreground_command: None,
            status: TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
            agent_attention: None,
            kind: TerminalKind::Pty,
        }
    }

    #[gpui::test]
    fn failed_rename_keeps_osc_titles_enabled(cx: &mut gpui::TestAppContext) {
        let terminal = TerminalId(7);
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet", Instant::now());
            state.overlay = Some(crate::state::Overlay::Dialog(
                super::super::Dialogs::RenameTerminal,
            ));
            state
        });
        let host = cx.update(|cx| {
            with_host(&state, cx, |host| {
                host.rename_terminal = RenameState {
                    terminal: Some(terminal),
                    input: TextFieldState::from_text("renamed"),
                    error: None,
                    in_flight: false,
                };
            });
            super::super::host::host_for(&state, cx)
        });
        let responses = Rc::new(RefCell::new(VecDeque::from([
            Err(ProtoError {
                kind: ErrorKind::Conflict,
                message: "rename refused".to_owned(),
            }),
            Ok(ResponseBody::Ack),
            Ok(ResponseBody::Terminal(terminal_record(terminal))),
        ])));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let request: RenameRequest = Rc::new({
            let responses = responses.clone();
            let seen = seen.clone();
            move |body| {
                seen.borrow_mut().push(body);
                let response = responses
                    .borrow_mut()
                    .pop_front()
                    .unwrap_or_else(|| panic!("missing rename response"));
                let (sender, receiver) = async_channel::bounded(1);
                sender
                    .try_send(response)
                    .unwrap_or_else(|error| panic!("send response: {error}"));
                receiver
            }
        });

        cx.update(|cx| {
            cx.set_global(fleet_ui_kit::Theme::dark());
            crate::keymap::init(cx);
        });
        let window = cx.add_window(|_, cx| RenameFixture {
            state: state.clone(),
            host,
            focus: cx.focus_handle(),
            request,
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(&mut visual, |view, window, cx| {
                window.focus(&view.focus, cx)
            })
            .unwrap_or_else(|error| panic!("focus rename: {error}"));

        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.update(|_, cx| {
            assert!(!state.read(cx).renamed_terminals.contains(&terminal));
            with_host(&state, cx, |host| {
                assert_eq!(
                    host.rename_terminal.error.as_deref(),
                    Some("rename refused")
                );
            });
        });

        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.update(|_, cx| {
            assert!(!state.read(cx).renamed_terminals.contains(&terminal));
            with_host(&state, cx, |host| {
                assert_eq!(
                    host.rename_terminal.error.as_deref(),
                    Some("rename: the daemon did not acknowledge the terminal")
                );
            });
        });

        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.update(|_, cx| {
            assert!(state.read(cx).renamed_terminals.contains(&terminal));
            assert!(state.read(cx).overlay.is_none());
        });
        assert_eq!(seen.borrow().len(), 3);
    }
}
