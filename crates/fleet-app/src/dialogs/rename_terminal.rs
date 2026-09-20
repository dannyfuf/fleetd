//! Rename the active Workspace terminal.

use fleet_core::ids::TerminalId;
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Dialog, HarnessTargetExt, Icon, InputMode, KeyHintRow, TextInput};
use gpui::{
    AnyElement, App, AppContext, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Window,
};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{DialogHost, notify, read_host, root, with_host},
    state::AppState,
};

type RenameReply = async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>>;
type RenameRequest = std::rc::Rc<dyn Fn(RequestBody) -> RenameReply>;

/// Draft for the terminal selected when the dialog opened.
///
/// The name itself lives in `DialogHost.rename_input`; this holds only what the card states
/// around it.
#[derive(Debug, Default)]
pub struct RenameState {
    terminal: Option<TerminalId>,
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
    let name = selected
        .as_ref()
        .map_or_else(String::new, |(_, name)| name.clone());
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_label(Some("Name".into()), cx);
        input.set_hide_status_line(true, cx);
        input.set_text(name, cx);
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, _| {
        host.rename_terminal = RenameState {
            terminal: selected.map(|(terminal, _)| terminal),
            error: None,
            in_flight: false,
        };
        host.rename_input = Some(input.clone());
    });
    let weak_host = host.downgrade();
    // A refusal is about the name that was sent, so the next keystroke retires it.
    let subscription = cx.subscribe(&input, move |_, event, cx| {
        if !matches!(event, fleet_ui_kit::TextInputEvent::Changed) {
            return;
        }
        let Some(host) = weak_host.upgrade() else {
            return;
        };
        host.update(cx, |host, cx| {
            if host.rename_terminal.error.take().is_some() {
                cx.notify();
            }
        });
    });
    host.update(cx, |host, _| {
        host.rename_input_subscription = Some(subscription)
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
    let (error, in_flight) = {
        let draft = &host.read(cx).rename_terminal;
        (draft.error.clone(), draft.in_flight)
    };
    let Some(input) = read_host(state, cx, |host, _| host.rename_input.clone()) else {
        return root(focus).into_any_element();
    };
    let confirm_state = state.clone();
    let confirm_request = request.clone();
    root(focus)
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            let typed = read_host(&confirm_state, cx, |host, cx| {
                host.rename_input
                    .as_ref()
                    .map_or_else(String::new, |input| input.read(cx).text().trim().to_owned())
            });
            let request = with_host(&confirm_state, cx, |host| {
                if host.rename_terminal.in_flight {
                    return None;
                }
                host.rename_terminal.error = None;
                host.rename_terminal.in_flight = true;
                (host.rename_terminal.terminal, typed).into()
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
                .body(input.clone().harness_target_indexed("dialog.field", 0))
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
    use gpui::{Context, Render, div};

    use super::*;

    struct RenameFixture {
        state: Entity<AppState>,
        host: Entity<DialogHost>,
        focus: FocusHandle,
        request: RenameRequest,
    }

    impl Render for RenameFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // `AppFrame` is the harness target table's frame boundary, and it is what the shell
            // puts above every dialog, so the fixture wears it too.
            fleet_ui_kit::AppFrame::new().body(div().key_context("Dialog").child(
                render_with_request(
                    &self.state,
                    self.request.clone(),
                    &self.focus,
                    &self.host,
                    cx,
                ),
            ))
        }
    }

    /// Seeds the draft plus its live editor the way [`seed`] does, for a fixed terminal.
    fn seed_named(
        state: &Entity<AppState>,
        terminal: TerminalId,
        cx: &mut App,
    ) -> Entity<DialogHost> {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("renamed", cx);
            input
        });
        with_host(state, cx, |host| {
            host.rename_terminal = RenameState {
                terminal: Some(terminal),
                error: None,
                in_flight: false,
            };
            host.rename_input = Some(input);
        });
        super::super::host::host_for(state, cx)
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
        let host = cx.update(|cx| seed_named(&state, terminal, cx));
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

    /// `docs/TESTING-HARNESS.md` §3 names a dialog's inputs `dialog.field[N]`, counted in the
    /// order the tab cycle visits them. A single-field dialog therefore publishes exactly
    /// `dialog.field[0]`, and a Phase 5 scenario clicks it rather than guessing a pixel.
    #[gpui::test]
    fn the_rename_dialog_names_its_one_field(cx: &mut gpui::TestAppContext) {
        fleet_ui_kit::harness::set_recording(true);
        let terminal = TerminalId(7);
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet", Instant::now());
            state.overlay = Some(crate::state::Overlay::Dialog(
                super::super::Dialogs::RenameTerminal,
            ));
            state
        });
        let host = cx.update(|cx| seed_named(&state, terminal, cx));
        let request: RenameRequest = Rc::new(|_body| async_channel::bounded(1).1);

        cx.update(|cx| {
            cx.set_global(fleet_ui_kit::Theme::dark());
            crate::keymap::init(cx);
        });
        let window = cx.add_window(|_, cx| RenameFixture {
            state,
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
        visual.run_until_parked();

        let names: Vec<String> = visual
            .update(|window, _| fleet_ui_kit::harness::painted(window))
            .into_iter()
            .map(|target| target.name.to_string())
            .collect();
        fleet_ui_kit::harness::set_recording(false);

        assert_eq!(
            names,
            vec!["dialog.field[0]"],
            "the rename dialog's only input is `dialog.field[0]`"
        );
    }
}
