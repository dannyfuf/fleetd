use fleet_core::config::default_config;
use gpui::AppContext;
use std::{cell::RefCell, rc::Rc};

use super::*;

struct FakeSettingsRequests {
    requests: Rc<RefCell<Vec<RequestBody>>>,
    config: Config,
}

impl super::persistence::SettingsRequests for FakeSettingsRequests {
    fn request(&self, body: RequestBody) -> super::persistence::SettingsReply {
        self.requests.borrow_mut().push(body.clone());
        let result = match body {
            RequestBody::MatchKeepAliveRules => Err(fleet_proto::error::ProtoError {
                kind: fleet_proto::error::ErrorKind::Unknown,
                message: "diagnostics refused".to_owned(),
            }),
            RequestBody::GetConfig | RequestBody::SetConfig { .. } => {
                Ok(ResponseBody::Config(self.config.clone()))
            }
            other => panic!("unexpected settings request: {other:?}"),
        };
        let (sender, receiver) = async_channel::bounded(1);
        sender
            .try_send(result)
            .unwrap_or_else(|error| panic!("send settings response: {error}"));
        receiver
    }
}

fn draft() -> SettingsState {
    let config = default_config("/tmp/fleet");
    SettingsState {
        original: Some(config.clone()),
        config: Some(config),
        ..SettingsState::default()
    }
}

#[test]
fn a_text_row_takes_every_surrendered_key_as_a_character() {
    for key in ["E", "D", "j", "k", "h", "l", " "] {
        assert!(
            accepts(&RowKind::Text(String::new()), key),
            "a free-text row takes `{key}`"
        );
        assert!(
            !accepts(&RowKind::Toggle(false), key),
            "a toggle takes nothing, so `{key}` keeps its binding"
        );
    }
    let number = RowKind::Number {
        value: 1,
        min: 0,
        unit: None,
    };
    assert!(!accepts(&number, "E"), "a number row is not a text field");
    assert!(accepts(&number, "7"));
}

#[test]
fn a_fresh_draft_is_not_dirty() {
    assert!(!draft().dirty());
}

#[test]
fn toggling_a_switch_makes_the_draft_dirty() {
    let mut state = draft();
    let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
    let before = config.jobs.warn_before_quit;
    toggle(config, &RowId::WarnBeforeQuit);
    assert_ne!(config.jobs.warn_before_quit, before);
    assert!(state.dirty());
}

#[test]
fn cycling_never_wraps_past_either_end() {
    let mut state = draft();
    let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
    cycle(config, &RowId::Agent, -1);
    assert_eq!(config.agent, Agent::Claude);
    cycle(config, &RowId::Agent, 1);
    assert_eq!(config.agent, Agent::Opencode);
    cycle(config, &RowId::Agent, 1);
    assert_eq!(config.agent, Agent::Opencode);
}

#[test]
fn invalid_numbers_do_not_replace_the_last_valid_value() {
    let mut state = draft();
    let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
    assert!(!commit_value(config, &RowId::StatusRefreshMs, "10"));
    assert_ne!(config.ui.status_refresh_ms, 10);
    assert!(commit_value(config, &RowId::StatusRefreshMs, "4000"));
    assert_eq!(config.ui.status_refresh_ms, 4_000);
    let grace = config.sleep.grace_ms;
    assert!(!commit_value(config, &RowId::GraceMs, "not a number"));
    assert_eq!(config.sleep.grace_ms, grace);
}

#[test]
fn late_or_duplicate_save_cannot_close_new_opening() {
    let mut opening = draft();
    opening.seq = 7;
    assert_eq!(opening.begin_save(), Some(7));
    assert_eq!(opening.begin_save(), None, "a second save is single-flight");
    assert_eq!(opening.begin_doctor(), Some(7));
    assert_eq!(
        opening.begin_doctor(),
        None,
        "doctor is independently single-flight"
    );

    let mut newer = SettingsState { seq: 8, ..draft() };
    assert!(!newer.finish_save(7));
    assert!(!newer.finish_doctor(7));
}

#[test]
fn save_patch_excludes_unchanged_concurrent_fields() {
    let original = default_config("/tmp/fleet");
    let mut edited = original.clone();
    edited.jobs.warn_before_quit = !original.jobs.warn_before_quit;
    let patch = super::persistence::changed_config_patch(&original, &edited)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        patch,
        serde_json::json!({
            "jobs": { "warnBeforeQuit": edited.jobs.warn_before_quit }
        })
    );
}

#[gpui::test]
fn config_and_diagnostics_failures_are_retryable(cx: &mut gpui::TestAppContext) {
    let mut failed = draft();
    failed.config_error = Some("config refused".to_owned());
    failed.matches_error = Some("diagnostics refused".to_owned());
    assert!(failed.load_error().is_some());
    assert_eq!(failed.take_failed_loads(), (true, true));
    assert!(failed.config_error.is_none());
    assert!(failed.matches_error.is_none());
    failed.config_loading = true;
    failed.matches_loading = true;
    assert_eq!(failed.take_failed_loads(), (false, false));

    let config = default_config("/tmp/fleet");
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet", std::time::Instant::now());
        state.overlay = Some(crate::state::Overlay::Dialog(
            super::super::Dialogs::Settings,
        ));
        state
    });
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings = draft();
            host.settings.matches_error = Some("diagnostics refused".to_owned());
            let settings = host
                .settings
                .config
                .as_mut()
                .unwrap_or_else(|| panic!("loaded settings"));
            settings.jobs.warn_before_quit = !settings.jobs.warn_before_quit;
        });
    });
    let requests = Rc::new(RefCell::new(Vec::new()));
    let fake = FakeSettingsRequests {
        requests: requests.clone(),
        config,
    };
    cx.update(|cx| super::persistence::save_with_requests(&state, &fake, cx));
    cx.run_until_parked();

    let requests = requests.borrow();
    assert!(
        requests
            .iter()
            .any(|request| matches!(request, RequestBody::MatchKeepAliveRules))
    );
    assert!(
        requests
            .iter()
            .any(|request| matches!(request, RequestBody::SetConfig { .. })),
        "a diagnostics retry must not consume the save action"
    );
    cx.update(|cx| {
        assert!(state.read(cx).overlay.is_none());
        with_host(&state, cx, |host| {
            assert_eq!(
                host.settings.matches_error.as_deref(),
                Some("diagnostics refused")
            );
        });
    });
}

#[test]
fn numeric_edit_preserves_raw_buffer_and_caret() {
    let mut config = default_config("/tmp/fleet");
    let mut input = TextFieldState::from_text("0007");
    assert!(input.move_left());
    let caret = input.caret_chars();
    assert!(commit_value(&mut config, &RowId::GraceMs, input.text()));
    assert_eq!(input.text(), "0007");
    assert_eq!(input.caret_chars(), caret);
    assert_eq!(config.sleep.grace_ms, 7);

    input.clear();
    assert!(!commit_value(&mut config, &RowId::GraceMs, input.text()));
    assert_eq!(input.text(), "");
    assert_eq!(config.sleep.grace_ms, 7);
}

#[test]
fn text_row_visual_focus_matches_key_ownership() {
    let editing = TextFieldState::from_text("claude");
    assert!(!super::view::input_is_focused(true, None));
    assert!(!super::view::input_is_focused(false, Some(&editing)));
    assert!(super::view::input_is_focused(true, Some(&editing)));
}

#[test]
fn about_reports_app_version_and_live_link() {
    let mut app = AppState::new("/tmp/fleet", std::time::Instant::now());
    app.snapshot = Some(fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "99.0.0-daemon".to_owned(),
            pid: 42,
            started_at: "2026-09-04T09:00:00Z".to_owned(),
            home: "/tmp/fleet".to_owned(),
        },
    });
    let rows = super::schema::about_rows(&app);
    assert!(matches!(
        &rows[0].kind,
        RowKind::Fact(version)
            if version == crate::presentation::bare_version(env!("CARGO_PKG_VERSION"))
    ));
    assert!(matches!(&rows[1].kind, RowKind::Fact(status) if status == "not connected"));

    app.daemon = crate::state::DaemonLink::Connected;
    let rows = super::schema::about_rows(&app);
    assert!(
        matches!(&rows[1].kind, RowKind::Fact(status) if status.starts_with("running · pid 42"))
    );
}

#[test]
fn a_number_row_refuses_every_non_digit() {
    let number = number_row(RowId::GraceMs, "Grace", 2_000, 0, "ms");
    // `j` / `k` / `h` / `l` / space are bound to navigation: none may reach the buffer,
    // where `commit_value` would clamp `2000j` down to the minimum.
    for literal in ["j", "k", "h", "l", " ", "-", "x"] {
        assert!(
            !accepts(&number.kind, literal),
            "`{literal}` must never type into a number row"
        );
    }
    assert!(accepts(&number.kind, "7"));
    assert!(!accepts(&number.kind, ""));

    // A free-text row still takes every printable key, including those letters.
    let text = text_row(RowId::ClaudeCommand, "Claude command", "claude");
    for literal in ["j", "k", "h", "l", " ", "7"] {
        assert!(accepts(&text.kind, literal));
    }

    // Rows with no input never take typing at all.
    let toggle = SettingRow {
        id: RowId::WarnBeforeQuit,
        label: "Warn".to_owned(),
        kind: RowKind::Toggle(true),
        detail: None,
        invalid: None,
    };
    assert!(!accepts(&toggle.kind, "j"));
}

#[test]
fn durations_read_as_one_unit() {
    assert_eq!(schema::format_cycler_duration(600_000), "10 min");
    assert_eq!(schema::format_cycler_duration(3_600_000), "1 h");
    assert_eq!(schema::format_cycler_duration(30_000), "30 s");
}

#[test]
fn every_section_has_a_title_and_only_data_sections_are_editable() {
    for section in Section::ALL {
        assert!(!section.title().is_empty());
    }
    assert!(Section::General.editable());
    assert!(!Section::Windows.editable());
    assert!(!Section::About.editable());
}

#[test]
fn the_sleep_section_lists_one_row_per_keep_alive_rule() {
    let state = draft();
    let mut probe = state;
    probe.section = Section::ALL
        .iter()
        .position(|section| *section == Section::Sleep)
        .unwrap_or(0);
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    let list = rows(&probe, &app);
    let rules = probe
        .config
        .as_ref()
        .map_or(0, |config| config.sleep.keep_alive.len());
    assert_eq!(list.len(), 2 + rules);
}

#[test]
fn stable_addresses_match_rendered_editable_rows() {
    let mut draft = draft();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    for section in 0..Section::ALL.len() {
        draft.section = section;
        for (index, row) in rows(&draft, &app).into_iter().enumerate() {
            draft.row = index;
            let focused = draft.focused_row().expect("loaded row");
            assert_eq!(focused.id, row.id);
            if matches!(row.kind, RowKind::Text(_) | RowKind::Number { .. }) {
                assert_eq!(accepts(&focused.kind, "7"), accepts(&row.kind, "7"));
                let rendered_value = match row.kind {
                    RowKind::Text(value) => value,
                    RowKind::Number { value, .. } => value.to_string(),
                    _ => unreachable!(),
                };
                let focused_value = match focused.kind {
                    RowKind::Text(value) => value,
                    RowKind::Number { value, .. } => value.to_string(),
                    _ => unreachable!(),
                };
                assert_eq!(focused_value, rendered_value);
            }
        }
    }
}

struct SettingsInput {
    state: Entity<AppState>,
    focus: FocusHandle,
}
impl gpui::Render for SettingsInput {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .key_context("Dialog")
            .child(super::view::input_actions(
                div()
                    .key_context("Settings")
                    .track_focus(&self.focus)
                    .size_full(),
                &self.state,
            ))
    }
}

#[gpui::test]
fn dispatched_edits_update_the_selected_setting_and_keep_navigation_available(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/settings", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings = draft();
            host.settings.row = 1;
        })
    });
    let window = cx.add_window(|_, cx| SettingsInput {
        state: state.clone(),
        focus: cx.focus_handle(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |view, window, cx| {
            window.focus(&view.focus, cx)
        })
        .expect("focus settings");
    visual.simulate_keystrokes("backspace j k h l space");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(
                host.settings.config.as_ref().unwrap().agent_commands.claude,
                "claudjkhl "
            );
            assert_eq!(host.settings.row, 1);
        })
    });
    visual.simulate_keystrokes("down");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.settings.row, 2);
            assert!(host.settings.editing.is_none());
        })
    });
}

#[gpui::test]
fn editing_a_cached_number_preserves_units_and_other_rows(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/settings", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings = draft();
            host.settings.section = 1;
            host.settings.row = 1;
        });
        refresh_rows(&state, cx);
        let label = with_host(&state, cx, |host| host.settings.prepared[0].label.as_ptr());
        with_host(&state, cx, |host| host.settings.editing = Some(TextFieldState::from_text("4000")));
        flush(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.settings.prepared[0].label.as_ptr(), label);
            assert!(matches!(&host.settings.prepared[1].kind, RowKind::Number { value: 4000, unit: Some(unit), .. } if unit == "ms"));
        });
    });
}

fn app_with_host_status(status: fleet_proto::snapshot::HostStatus) -> AppState {
    let mut app = AppState::new("/tmp/fleet", std::time::Instant::now());
    app.snapshot = Some(fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: vec![status],
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: String::new(),
            pid: 1,
            started_at: String::new(),
            home: "/tmp/fleet".to_owned(),
        },
    });
    app
}

fn host_id(id: &str) -> fleet_core::ids::HostId {
    fleet_core::ids::HostId::try_from(id).unwrap_or_else(|error| panic!("{error}"))
}

fn fact_value(row: &SettingRow) -> &str {
    match &row.kind {
        RowKind::Fact(value) => value.as_str(),
        other => panic!("expected a read-only fact, got {other:?}"),
    }
}

#[test]
fn the_hosts_section_renders_the_configured_shape_and_the_live_link() {
    let mut config = default_config("/tmp/fleet");
    config.hosts.insert(
        host_id("devbox"),
        fleet_core::model::HostConfigEntry::Tailscale {
            node: "dev-box".to_owned(),
            user: Some("df".to_owned()),
            ssh_options: Vec::new(),
            fleetd: "fleetd".to_owned(),
            fleet_home: Some("~/.fleet".to_owned()),
        },
    );
    config.default_host = "devbox".to_owned();
    let app = app_with_host_status(fleet_proto::snapshot::HostStatus {
        id: host_id("devbox"),
        provider: "tailscale".to_owned(),
        version: Some("0.4.0".to_owned()),
        link: fleet_proto::snapshot::LinkState::Ready,
        address: Some("100.64.0.2".to_owned()),
        agent_binaries: None,
        reachable: true,
        checked_at: "2026-09-04T12:00:00Z".to_owned(),
        error: None,
    });

    let rows = super::schema::host_rows(&config, &app);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].label, "default");
    assert_eq!(fact_value(&rows[0]), "devbox");
    assert_eq!(rows[1].label, "devbox");
    assert_eq!(
        fact_value(&rows[1]),
        "tailscale \u{00b7} node dev-box \u{00b7} user df \u{2014} ready \u{00b7} fleetd 0.4.0"
    );
    assert!(
        rows.iter().all(|row| row.id == RowId::ReadOnly),
        "the hosts section stays read-only (§3.8.6)"
    );
}

#[test]
fn a_legacy_host_is_named_as_one_and_told_to_migrate() {
    let mut config = default_config("/tmp/fleet");
    config.hosts.insert(
        host_id("archdev"),
        fleet_core::model::HostConfigEntry::Legacy {
            ssh: "arch-dev".to_owned(),
            swarm_command: "swarm".to_owned(),
        },
    );
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    let rows = super::schema::host_rows(&config, &app);
    assert_eq!(fact_value(&rows[0]), "local");
    assert_eq!(
        fact_value(&rows[1]),
        "legacy \u{00b7} ssh arch-dev \u{00b7} swarm \u{2014} legacy entry \u{2014} migrate it to a tailscale host"
    );
}

#[test]
fn an_unreachable_host_reports_the_probe_error_and_a_command_host_its_label() {
    let mut config = default_config("/tmp/fleet");
    config.hosts.insert(
        host_id("devbox"),
        fleet_core::model::HostConfigEntry::Tailscale {
            node: "dev-box".to_owned(),
            user: None,
            ssh_options: Vec::new(),
            fleetd: "fleetd".to_owned(),
            fleet_home: None,
        },
    );
    config.hosts.insert(
        host_id("loopback"),
        fleet_core::model::HostConfigEntry::Command {
            run: vec!["sh".to_owned()],
            fleetd: "/tmp/fleetd".to_owned(),
            fleet_home: Some("/tmp/remote".to_owned()),
            display: Some("loopback".to_owned()),
        },
    );
    let app = app_with_host_status(fleet_proto::snapshot::HostStatus {
        id: host_id("devbox"),
        provider: "tailscale".to_owned(),
        version: None,
        link: fleet_proto::snapshot::LinkState::Down,
        address: None,
        agent_binaries: None,
        reachable: false,
        checked_at: "2026-09-04T12:00:00Z".to_owned(),
        error: Some("ssh: connect timed out after 5s".to_owned()),
    });
    let rows = super::schema::host_rows(&config, &app);
    assert_eq!(
        fact_value(&rows[1]),
        "tailscale \u{00b7} node dev-box \u{2014} down \u{00b7} ssh: connect timed out after 5s"
    );
    assert_eq!(
        fact_value(&rows[2]),
        "command \u{00b7} loopback \u{2014} no status yet",
        "a configured host the daemon has not probed says so rather than claiming failure"
    );
}
