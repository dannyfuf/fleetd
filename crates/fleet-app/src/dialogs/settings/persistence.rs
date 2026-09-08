use super::*;

pub(super) type SettingsReply =
    async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>>;

pub(super) trait SettingsRequests {
    fn request(&self, body: RequestBody) -> SettingsReply;
}

impl SettingsRequests for Bridge {
    fn request(&self, body: RequestBody) -> SettingsReply {
        Bridge::request(self, body)
    }
}

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
    request_config(state, bridge, seq, cx);
    request_matches(state, bridge, seq, cx);
}

fn request_config(
    state: &Entity<AppState>,
    requests: &dyn SettingsRequests,
    seq: u64,
    cx: &mut App,
) {
    with_host(state, cx, |host| host.settings.config_loading = true);
    let reply = requests.request(RequestBody::GetConfig);
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let live = with_host(&state, cx, |host| {
                if host.settings.seq != seq {
                    return false;
                }
                host.settings.config_loading = false;
                match answer {
                    Ok(Ok(ResponseBody::Config(config))) => {
                        host.settings.config_error = None;
                        host.settings.original = Some(config.clone());
                        host.settings.config = Some(config);
                    }
                    Ok(Err(error)) => host.settings.config_error = Some(error.message),
                    Ok(Ok(_)) | Err(_) => {
                        host.settings.config_error =
                            Some("configuration: the daemon did not answer".to_owned());
                    }
                }
                true
            });
            if live {
                refresh_rows(&state, cx);
                notify(&state, cx);
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "settings-config", task);
}

fn request_matches(
    state: &Entity<AppState>,
    requests: &dyn SettingsRequests,
    seq: u64,
    cx: &mut App,
) {
    with_host(state, cx, |host| host.settings.matches_loading = true);
    let reply = requests.request(RequestBody::MatchKeepAliveRules);
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let live = with_host(&state, cx, |host| {
                if host.settings.seq != seq {
                    return false;
                }
                host.settings.matches_loading = false;
                match answer {
                    Ok(Ok(ResponseBody::KeepAliveRuleMatches(matches))) => {
                        host.settings.matches_error = None;
                        host.settings.matches = matches;
                    }
                    Ok(Err(error)) => host.settings.matches_error = Some(error.message),
                    Ok(Ok(_)) | Err(_) => {
                        host.settings.matches_error =
                            Some("keep-alive diagnostics: the daemon did not answer".to_owned());
                    }
                }
                true
            });
            if live {
                refresh_rows(&state, cx);
                notify(&state, cx);
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "settings-matches", task);
}

fn retry_failed_loads(
    state: &Entity<AppState>,
    requests: &dyn SettingsRequests,
    cx: &mut App,
) -> bool {
    let (seq, config, matches) = with_host(state, cx, |host| {
        let (config, matches) = host.settings.take_failed_loads();
        (host.settings.seq, config, matches)
    });
    if config {
        request_config(state, requests, seq, cx);
    }
    if matches {
        request_matches(state, requests, seq, cx);
    }
    if config || matches {
        notify(state, cx);
    }
    config
}

/// `Enter`: save synchronously and silently; a refused write keeps the dialog open (§3.8.6).
pub(super) fn save(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    save_with_requests(state, bridge, cx);
}

pub(super) fn save_with_requests(
    state: &Entity<AppState>,
    requests: &dyn SettingsRequests,
    cx: &mut App,
) {
    if retry_failed_loads(state, requests, cx) {
        return;
    }
    if !read_host(state, cx, |host, _| host.settings.editing_is_valid()) {
        with_host(state, cx, |host| {
            host.settings.error = Some("finish the invalid numeric edit before saving".to_owned());
        });
        notify(state, cx);
        return;
    }
    let Some((seq, original, config, warn_before_quit)) = with_host(state, cx, |host| {
        let original = host.settings.original.clone()?;
        let config = host.settings.config.clone()?;
        let seq = host.settings.begin_save()?;
        Some((seq, original, config.clone(), config.jobs.warn_before_quit))
    }) else {
        return;
    };
    let Ok(patch) = changed_config_patch(&original, &config) else {
        with_host(state, cx, |host| {
            let _current = host.settings.finish_save(seq);
            host.settings.error = Some("the configuration could not be encoded".to_owned());
        });
        notify(state, cx);
        return;
    };
    let reply = requests.request(RequestBody::SetConfig { patch });
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            if !with_host(&state, cx, |host| host.settings.finish_save(seq)) {
                return;
            }
            match answer {
                Ok(Ok(ResponseBody::Config(_))) => {
                    state.update(cx, |app, cx| {
                        app.warn_before_quit = warn_before_quit;
                        app.close_overlay();
                        cx.notify();
                    });
                }
                Ok(Err(failure)) => {
                    with_host(&state, cx, |host| {
                        host.settings.error = Some(failure.message)
                    });
                    notify(&state, cx);
                }
                Ok(Ok(_)) | Err(_) => {
                    with_host(&state, cx, |host| {
                        host.settings.error =
                            Some("save: the daemon did not acknowledge the config".to_owned());
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
pub(crate) fn editor_command() -> String {
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
    let Some(seq) = with_host(state, cx, |host| host.settings.begin_doctor()) else {
        return;
    };
    let reply = bridge.request(RequestBody::Doctor);
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            if !with_host(&state, cx, |host| host.settings.finish_doctor(seq)) {
                return;
            }
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

pub(super) fn changed_config_patch(
    original: &Config,
    current: &Config,
) -> Result<serde_json::Value, serde_json::Error> {
    let original = serde_json::to_value(original)?;
    let current = serde_json::to_value(current)?;
    Ok(changed_value(&original, &current)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())))
}

fn changed_value(
    original: &serde_json::Value,
    current: &serde_json::Value,
) -> Option<serde_json::Value> {
    if original == current {
        return None;
    }
    match (original, current) {
        (serde_json::Value::Object(original), serde_json::Value::Object(current)) => {
            let changed = current
                .iter()
                .filter_map(|(key, value)| {
                    original
                        .get(key)
                        .and_then(|before| changed_value(before, value))
                        .or_else(|| (!original.contains_key(key)).then(|| value.clone()))
                        .map(|value| (key.clone(), value))
                })
                .collect();
            Some(serde_json::Value::Object(changed))
        }
        _ => Some(current.clone()),
    }
}
