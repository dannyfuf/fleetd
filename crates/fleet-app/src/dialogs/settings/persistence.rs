use super::*;

/// Loads the effective configuration and the live keep-alive match counts.
pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let seq = with_host(state, cx, |host| {
        let seq = host.settings.seq.wrapping_add(1);
        host.settings = SettingsState {
            seq,
            ..SettingsState::default()
        };
        seq
    });
    let config_reply = bridge.request(RequestBody::GetConfig);
    let matches_reply = bridge.request(RequestBody::MatchKeepAliveRules);
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        if let Ok(Ok(ResponseBody::Config(config))) = config_reply.recv().await {
            cx.update(|cx| {
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                let live = with_host(&state, cx, |host| {
                    if host.settings.seq != seq {
                        return false;
                    }
                    host.settings.original = Some(config.clone());
                    host.settings.config = Some(config);
                    true
                });
                if live {
                    refresh_rows(&state, cx);
                    notify(&state, cx);
                }
            });
        }
        if let Ok(Ok(ResponseBody::KeepAliveRuleMatches(matches))) = matches_reply.recv().await {
            cx.update(|cx| {
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                let live = with_host(&state, cx, |host| {
                    if host.settings.seq != seq {
                        return false;
                    }
                    host.settings.matches = matches;
                    true
                });
                if live {
                    refresh_rows(&state, cx);
                    notify(&state, cx);
                }
            });
        }
    });
    crate::dialogs::retain_task(state, cx, "settings-load", task);
}

/// `Enter`: save synchronously and silently; a refused write keeps the dialog open (§3.8.6).
pub(super) fn save(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some((patch, warn_before_quit)) = read_host(state, cx, |host, _| {
        host.settings
            .config
            .as_ref()
            .map(|config| (serde_json::to_value(config), config.jobs.warn_before_quit))
    }) else {
        return;
    };
    let Ok(patch) = patch else {
        with_host(state, cx, |host| {
            host.settings.error = Some("the configuration could not be encoded".to_owned())
        });
        notify(state, cx);
        return;
    };
    let reply = bridge.request(RequestBody::SetConfig { patch });
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            match answer {
                Ok(_) => {
                    state.update(cx, |app, cx| {
                        app.warn_before_quit = warn_before_quit;
                        app.close_overlay();
                        cx.notify();
                    });
                }
                Err(failure) => {
                    with_host(&state, cx, |host| {
                        host.settings.error = Some(failure.message)
                    });
                    notify(&state, cx);
                }
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "settings-save", task);
}

/// `D`: run doctor and surface the first failing check in the sticky error slot.
/// §3.8.6 About: `E` opens `config.json` **in a new terminal tab**, not in the OS handler.
///
/// KEYMAP scopes `E` per context and gives this one the terminal tab, so that editing the file
/// happens inside Fleet, next to the daemon it configures. Without a live session there is no
/// tab strip to add to, and the system handler is the honest fallback rather than a dead key.
pub(super) fn open_config_file(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let path = state.read(cx).home.join("config.json");
    let Some(session) = state.read(cx).active_session().cloned() else {
        cx.open_with_system(&path);
        return;
    };
    bridge.send(RequestBody::NewTerminal {
        session: session.id.clone(),
        name: workspace_tabs::unique_terminal_name(&session, "config"),
        command: format!("{} {}", editor_command(), path.display()),
        cwd: session.cwd.clone(),
    });
    state.update(cx, |app, cx| {
        app.close_overlay();
        app.screen = Screen::Workspace {
            session: session.id.clone(),
        };
        cx.notify();
    });
}

/// `$EDITOR`, or `vi` — the one editor POSIX guarantees.
pub(super) fn editor_command() -> String {
    std::env::var("EDITOR")
        .ok()
        .map(|editor| editor.trim().to_owned())
        .filter(|editor| !editor.is_empty())
        .unwrap_or_else(|| "vi".to_owned())
}

/// §3.8.6 About: `D` runs doctor and shows the §3.12 `Daemon > Doctor` surface.
///
/// The same key means the same thing on the daemon-down splash, so both write the answer to
/// [`AppState::doctor`] and the shell renders it; a toast would have been a second, weaker
/// spelling of a screen that already exists.
pub(super) fn run_doctor(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let reply = bridge.request(RequestBody::Doctor);
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            match answer {
                Ok(Ok(ResponseBody::Doctor(checks))) => {
                    state.update(cx, |app, cx| {
                        app.doctor = Some(checks);
                        // The doctor table is a full surface; it replaces the dialog it was
                        // raised from, and `Esc` there comes back to the app.
                        app.close_overlay();
                        cx.notify();
                    });
                }
                // A refused request keeps the dialog open with the exact error (§3.8.6 States).
                Ok(Err(error)) => {
                    with_host(&state, cx, |host| {
                        host.settings.error = Some(error.message.clone())
                    });
                    notify(&state, cx);
                }
                Ok(Ok(_)) | Err(_) => {
                    with_host(&state, cx, |host| {
                        host.settings.error = Some("doctor: the daemon did not answer".to_owned());
                    });
                    notify(&state, cx);
                }
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "settings-doctor", task);
}
