use fleet_core::config::{NATIVE_BOARD, NATIVE_LAZYGIT, WindowConfig, default_config};
use gpui::AppContext;
use std::{cell::RefCell, rc::Rc};

use fleet_core::agents::PermissionMode;

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
    cycle(config, &RowId::Agent, -1, &Efforts::default());
    assert_eq!(config.agent, Agent::Claude);
    cycle(config, &RowId::Agent, 1, &Efforts::default());
    assert_eq!(config.agent, Agent::Codex);
    cycle(config, &RowId::Agent, 1, &Efforts::default());
    assert_eq!(config.agent, Agent::Codex);
}

#[test]
fn native_agent_default_rows_follow_each_harnesss_mode_vocabulary() {
    let mut state = draft();
    let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
    assert_eq!(config.native_agents.claude.mode, PermissionMode::FullAccess);
    cycle(config, &RowId::ClaudeDefaultMode, -1, &Efforts::default());
    assert_eq!(config.native_agents.claude.mode, PermissionMode::DontAsk);
    cycle(config, &RowId::CodexDefaultMode, -1, &Efforts::default());
    assert_eq!(config.native_agents.codex.mode, PermissionMode::Plan);

    assert!(commit_value(
        config,
        &RowId::ClaudeDefaultModel,
        "  fable[1m]  "
    ));
    assert_eq!(
        config.native_agents.claude.model.as_deref(),
        Some("fable[1m]")
    );
    // Effort is a closed choice now: `Default` (none configured) first, then the vocabulary.
    let efforts = Efforts::default();
    select(config, &RowId::ClaudeDefaultEffort, 3, &efforts);
    assert_eq!(config.native_agents.claude.effort.as_deref(), Some("high"));
    select(config, &RowId::ClaudeDefaultEffort, 0, &efforts);
    assert!(config.native_agents.claude.effort.is_none());

    state.section = Section::Agents.index();
    let ids = schema::rows(
        &state,
        &AppState::new("/tmp/fleet", std::time::Instant::now()),
    )
    .into_iter()
    .map(|row| row.id)
    .collect::<Vec<_>>();
    for expected in [
        RowId::ClaudeDefaultMode,
        RowId::ClaudeDefaultModel,
        RowId::ClaudeDefaultEffort,
        RowId::CodexDefaultMode,
        RowId::CodexDefaultModel,
        RowId::CodexDefaultEffort,
    ] {
        assert!(ids.contains(&expected), "missing settings row {expected:?}");
    }
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

/// The editor keeps the raw text the user typed; the draft takes only what parses, so a
/// half-typed number never rewrites the row behind it.
#[test]
fn a_numeric_edit_commits_only_what_parses() {
    let mut config = default_config("/tmp/fleet");
    assert!(commit_value(&mut config, &RowId::GraceMs, "0007"));
    assert_eq!(config.sleep.grace_ms, 7);
    assert!(!commit_value(&mut config, &RowId::GraceMs, ""));
    assert_eq!(config.sleep.grace_ms, 7);
}

#[test]
fn about_reports_app_version_and_live_link() {
    let mut app = AppState::new("/tmp/fleet", std::time::Instant::now());
    app.snapshot = Some(fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        revision: None,
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
fn durations_read_as_one_unit() {
    assert_eq!(schema::format_cycler_duration(600_000), "10 min");
    assert_eq!(schema::format_cycler_duration(3_600_000), "1 h");
    assert_eq!(schema::format_cycler_duration(30_000), "30 s");
}

#[test]
fn every_section_has_a_title_and_only_config_file_sections_end_with_a_caption() {
    for section in Section::ALL {
        assert!(!section.title().is_empty());
    }
    assert_eq!(
        Section::General.caption(),
        Some("Change these in config.json.")
    );
    assert_eq!(Section::Hosts.caption(), Section::General.caption());
    assert_eq!(
        Section::Sleep.caption(),
        Some("Rules are defined in config.json. Open it from the footer to add or change one.")
    );
    assert!(Section::About.caption().is_none());
    assert!(Section::Agents.caption().is_none());
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

/// A reserved `fleet://` command is a surface Fleet draws, not a program the user could run, so
/// every one of them reads the same way: the name, the reserved command, and "built in".
#[test]
fn every_reserved_window_command_reads_as_built_in() {
    let mut probe = draft();
    probe.section = Section::ALL
        .iter()
        .position(|section| *section == Section::General)
        .unwrap_or(0);
    let config = probe.config.as_mut().unwrap_or_else(|| panic!("no config"));
    config.windows = vec![
        WindowConfig {
            name: "nvim".to_owned(),
            command: "nvim .".to_owned(),
        },
        WindowConfig {
            name: "lg".to_owned(),
            command: NATIVE_LAZYGIT.to_owned(),
        },
        WindowConfig {
            name: "board".to_owned(),
            command: NATIVE_BOARD.to_owned(),
        },
    ];
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    let list = rows(&probe, &app);
    let values = list
        .iter()
        .map(|row| match &row.kind {
            RowKind::Fact(value) => (row.label.as_str(), value.as_str()),
            other => panic!("window rows are read-only facts: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        vec![
            ("1", "nvim \u{2014} nvim ."),
            ("2", "lg \u{2014} fleet://lazygit (built in)"),
            ("3", "board \u{2014} fleet://board (built in)"),
        ]
    );
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
            if row.kind.opens_editor() {
                let value_of = |kind: RowKind| match kind {
                    RowKind::Text(value) | RowKind::Model { value, .. } => value,
                    RowKind::Number { value, .. } => value.to_string(),
                    other => panic!("not an editable value: {other:?}"),
                };
                assert_eq!(value_of(focused.kind), value_of(row.kind));
            }
        }
    }
}

struct SettingsInput {
    state: Entity<AppState>,
    focus: FocusHandle,
}
impl gpui::Render for SettingsInput {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let focus = self.focus.clone();
        // The shell publishes `Settings` while browsing and `SettingsEditing` while a row
        // editor exists, and mounts that editor inside the dialog's focus subtree. The fixture
        // reproduces both, because that pair is exactly what this test is about.
        let editing = read_host(&self.state, cx, |host, _| host.settings_input.clone());
        let word = if editing.is_some() {
            "SettingsEditing"
        } else {
            "Settings"
        };
        div().key_context("Dialog").child(
            super::view::input_actions(
                div()
                    .key_context(word)
                    .track_focus(&self.focus)
                    .size_full()
                    .children(editing),
                &self.state,
                &self.focus,
            )
            // The dialog's own `Enter` saves after this on a row that opens nothing; the
            // fixture drives only the half that opens and closes a row's editor.
            .on_action(move |_: &dialog::Confirm, window, cx| {
                confirm_opens_editing(&state, &focus, window, cx);
            }),
        )
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
            host.settings.section = Section::Agents.index();
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
    // Browsing: `j` / `k` move rows and `space` toggles, because no editor exists yet.
    visual.simulate_keystrokes("j k");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.settings.row, 1);
            assert!(host.settings.editing.is_none());
        })
    });
    // `Enter` opens the row and hands its editor the keyboard; `j` is then a letter.
    visual.simulate_keystrokes("enter");
    visual.simulate_input("jkhl ");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(
                host.settings.config.as_ref().unwrap().agent_commands.claude,
                "claudejkhl "
            );
            assert_eq!(host.settings.row, 1);
        })
    });
    // `ctrl-n` is the container's, so it still moves the row and closes the editor.
    visual.simulate_keystrokes("ctrl-n");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.settings.row, 2);
            assert!(host.settings.editing.is_none());
            assert!(host.settings_input.is_none());
        })
    });
}

/// §3.8.6: `Enter` in an open editor keeps what was typed and closes the box in place, leaving
/// the dialog open on the same row; a value that breaks its rule keeps the box open instead.
#[gpui::test]
fn enter_in_an_open_editor_keeps_the_value_and_closes_the_box(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/settings-enter", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings = draft();
            host.settings.section = Section::Agents.index();
            host.settings.row = 1;
        });
        refresh_rows(&state, cx);
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
    visual.simulate_keystrokes("enter");
    visual.simulate_input("-x");
    visual.simulate_keystrokes("enter");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(
                host.settings.config.as_ref().unwrap().agent_commands.claude,
                "claude-x"
            );
            assert_eq!(host.settings.row, 1, "the cursor stays on the row");
            assert!(host.settings.editing.is_none(), "the box closed");
            assert!(host.settings_input.is_none());
        });
    });
    window
        .update(&mut visual, |view, window, _| {
            assert!(
                view.focus.is_focused(window),
                "the keys went back to the dialog"
            );
        })
        .expect("settings window");

    // Status › Local status refresh has a floor of 500 ms: a value under it keeps its box open.
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings.section = Section::Status.index();
            host.settings.row = 0;
        });
        refresh_rows(&state, cx);
    });
    visual.simulate_keystrokes("enter");
    // The default is four digits; a few spare backspaces on an empty buffer are harmless.
    visual.simulate_keystrokes("backspace backspace backspace backspace backspace backspace");
    visual.simulate_input("9");
    visual.simulate_keystrokes("enter");
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert!(
                host.settings.edit_rule.is_some(),
                "9 ms breaks the 500 ms floor"
            );
            assert!(
                host.settings_input.is_some(),
                "the box stays open on a broken rule"
            );
        })
    });
}

/// §3.8.6's number rows filter to ASCII digits, so a letter that would make `commit_value`
/// clamp the setting to its minimum never reaches the buffer at all.
#[gpui::test]
fn a_number_row_refuses_every_non_digit(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/settings-number", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings = draft();
            // Sleep › Grace, the first number row of §3.8.6.
            host.settings.section = Section::Sleep.index();
            host.settings.row = 1;
        });
        refresh_rows(&state, cx);
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
    visual.simulate_keystrokes("enter");
    visual.simulate_input("4x0j0");
    visual.update(|_, cx| {
        read_host(&state, cx, |host, cx| {
            let input = host
                .settings_input
                .as_ref()
                .unwrap_or_else(|| panic!("the number row materialized an editor"));
            assert_eq!(input.read(cx).text(), "2000400");
            assert_eq!(
                host.settings.config.as_ref().unwrap().sleep.grace_ms,
                2_000_400
            );
        })
    });
}

#[gpui::test]
fn editing_a_cached_number_preserves_units_and_other_rows(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/settings", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings = draft();
            host.settings.section = Section::Sleep.index();
            host.settings.row = 1;
        });
        refresh_rows(&state, cx);
        let label = with_host(&state, cx, |host| host.settings.prepared[0].label.as_ptr());
        with_host(&state, cx, |host| host.settings.editing = Some("4000".to_owned()));
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
        revision: None,
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
            identity_file: None,
            ssh_host: None,
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
            identity_file: None,
            ssh_host: None,
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

#[test]
fn a_click_on_an_option_writes_the_same_value_its_key_would() {
    let efforts = Efforts::default();
    let mut keyed = draft();
    let mut clicked = draft();
    let keyed_config = keyed.config.as_mut().unwrap_or_else(|| panic!("no config"));
    cycle(keyed_config, &RowId::KeepFinishedFor, 1, &efforts);
    let choice = choice_of(keyed_config, &RowId::KeepFinishedFor, &efforts)
        .unwrap_or_else(|| panic!("a duration is a choice"));
    let clicked_config = clicked
        .config
        .as_mut()
        .unwrap_or_else(|| panic!("no config"));
    select(
        clicked_config,
        &RowId::KeepFinishedFor,
        choice.index.unwrap_or_else(|| panic!("on the grid")),
        &efforts,
    );
    assert_eq!(keyed.config, clicked.config);
    // An index past the end is ignored rather than clamped onto some other value.
    let before = clicked.config.clone();
    select(
        clicked
            .config
            .as_mut()
            .unwrap_or_else(|| panic!("no config")),
        &RowId::HotPoolSize,
        99,
        &efforts,
    );
    assert_eq!(clicked.config, before);
}

#[test]
fn a_switch_click_asks_for_a_value_and_space_flips_it() {
    let mut state = draft();
    let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
    let on = switch_of(config, &RowId::SleepOnSwitch).unwrap_or_else(|| panic!("a switch"));
    set_switch(config, &RowId::SleepOnSwitch, on);
    assert_eq!(switch_of(config, &RowId::SleepOnSwitch), Some(on));
    toggle(config, &RowId::SleepOnSwitch);
    assert_eq!(switch_of(config, &RowId::SleepOnSwitch), Some(!on));
    assert!(state.dirty());
}

#[test]
fn effort_offers_the_default_then_the_base_vocabulary_and_keeps_an_unknown_value() {
    let mut state = draft();
    let efforts = Efforts::default();
    let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
    let choice = choice_of(config, &RowId::CodexDefaultEffort, &efforts)
        .unwrap_or_else(|| panic!("effort is a choice"));
    assert_eq!(choice.labels, ["Default", "Low", "Medium", "High"]);
    assert_eq!(choice.index, Some(0));
    config.native_agents.codex.effort = Some("xhigh".to_owned());
    let off = choice_of(config, &RowId::CodexDefaultEffort, &efforts)
        .unwrap_or_else(|| panic!("effort is a choice"));
    assert_eq!((off.index, off.current.as_str()), (None, "xhigh"));
}

#[test]
fn search_finds_rows_across_sections_by_every_word() {
    let state = draft();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    // The model row by its label, and Effort by its helper sentence ("Used with the default
    // model."), both in Claude's card, in pane order.
    let hits = schema::search_hits("claude model", &state, &app);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[1].label, "Claude \u{203a} Effort");
    let hit = &hits[0];
    assert_eq!(
        (hit.section, hit.row, hit.label.as_str()),
        (Section::Agents, 4, "Claude \u{203a} Default model")
    );
    assert!(schema::search_hits("   ", &state, &app).is_empty());
    let retention = schema::search_hits("RETENTION", &state, &app);
    assert_eq!(retention.len(), 1);
    assert_eq!(retention[0].section, Section::Jobs);
    // A hit's spans are its label with the typed words marked strong; a word matched only in
    // the helper or the section leaves the label plain.
    assert_eq!(
        retention[0].spans,
        vec![("Trash ".to_owned(), false), ("retention".to_owned(), true)]
    );
    // `model` matched Effort's helper, so only `claude` — the card prefix — is strong there.
    assert_eq!(
        hits[1].spans,
        vec![
            ("Claude".to_owned(), true),
            (" \u{203a} Effort".to_owned(), false)
        ]
    );
}

#[test]
fn label_spans_mark_every_typed_word_and_join_back_to_the_label() {
    let words =
        |query: &str| -> Vec<String> { query.split_whitespace().map(str::to_lowercase).collect() };
    let joined = |spans: &[(String, bool)]| -> String {
        spans.iter().map(|(text, _)| text.as_str()).collect()
    };
    // No match: one plain span.
    assert_eq!(
        schema::label_spans("Grace", &words("zzz")),
        vec![("Grace".to_owned(), false)]
    );
    assert_eq!(
        schema::label_spans("", &words("a")),
        vec![(String::new(), false)]
    );
    // A word in the middle, case-insensitively, with the card prefix kept plain.
    let spans = schema::label_spans("Claude \u{203a} Effort", &words("EFFORT"));
    assert_eq!(
        spans,
        vec![
            ("Claude \u{203a} ".to_owned(), false),
            ("Effort".to_owned(), true)
        ]
    );
    assert_eq!(joined(&spans), "Claude \u{203a} Effort");
    // Two words, and a word that occurs twice; overlapping matches merge.
    let spans = schema::label_spans("Keep finished jobs for", &words("keep jobs"));
    assert_eq!(
        spans,
        vec![
            ("Keep".to_owned(), true),
            (" finished ".to_owned(), false),
            ("jobs".to_owned(), true),
            (" for".to_owned(), false)
        ]
    );
    let spans = schema::label_spans("Repo cache · PR cache", &words("cache ca"));
    assert_eq!(
        spans,
        vec![
            ("Repo ".to_owned(), false),
            ("cache".to_owned(), true),
            (" \u{b7} PR ".to_owned(), false),
            ("cache".to_owned(), true)
        ]
    );
    // A character whose lowercase form is longer in bytes never splits a span mid-character.
    let spans = schema::label_spans("İstanbul host", &words("host"));
    assert_eq!(joined(&spans), "İstanbul host");
    assert_eq!(spans[1], ("host".to_owned(), true));
}

#[test]
fn a_card_of_rows_is_one_pane_child_for_scrolling() {
    let mut state = draft();
    state.section = Section::Agents.index();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    let list = rows(&state, &app);
    // The default agent, then the Claude card (rows 1–5), then the Codex card (6–10).
    assert_eq!(block_of(&list, 0), 0);
    assert_eq!(block_of(&list, 1), 1);
    assert_eq!(block_of(&list, 5), 1);
    assert_eq!(block_of(&list, 6), 2);
    assert_eq!(block_of(&list, 10), 2);
}

#[test]
fn the_cursor_row_reads_as_its_label_and_value() {
    let mut state = draft();
    state.section = Section::Agents.index();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    state.prepared = rows(&state, &app).into();
    assert_eq!(state.cursor_summary(), "Default agent = Claude");
    state.row = 4;
    assert_eq!(state.cursor_summary(), "Default model = ");
}

/// A fixture dialog root that, like the shell, closes the overlay on a `dialog::Cancel` the
/// settings handler lets through, so a test can tell "kept" from "closed".
struct SettingsEscape {
    state: Entity<AppState>,
    focus: FocusHandle,
    closed: Rc<std::cell::Cell<bool>>,
}

impl gpui::Render for SettingsEscape {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let editing = read_host(&self.state, cx, |host, _| host.settings_input.clone());
        let word = if editing.is_some() {
            "SettingsEditing"
        } else {
            "Settings"
        };
        let state = self.state.clone();
        let focus = self.focus.clone();
        let closed = self.closed.clone();
        div()
            .key_context("Dialog")
            .size_full()
            .on_action(move |_: &dialog::Cancel, _, _| closed.set(true))
            .child(
                super::view::input_actions(
                    div()
                        .key_context(word)
                        .track_focus(&self.focus)
                        .size_full()
                        .children(editing),
                    &self.state,
                    &self.focus,
                )
                .on_action({
                    let state = state.clone();
                    let focus = focus.clone();
                    move |_: &dialog::Confirm, window, cx| {
                        confirm_opens_editing(&state, &focus, window, cx);
                    }
                })
                .on_action(move |_: &dialog::Cancel, window, cx| {
                    if cancel(&state, &focus, window, cx) {
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
    }
}

fn open_escape_fixture(
    cx: &mut gpui::TestAppContext,
    section: Section,
    row: usize,
) -> (
    Entity<AppState>,
    Rc<std::cell::Cell<bool>>,
    gpui::VisualTestContext,
) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/settings-escape", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.settings = draft();
            host.settings.section = section.index();
            host.settings.row = row;
        });
        refresh_rows(&state, cx);
    });
    let closed = Rc::new(std::cell::Cell::new(false));
    let window = cx.add_window({
        let state = state.clone();
        let closed = closed.clone();
        move |_, cx| SettingsEscape {
            state,
            focus: cx.focus_handle(),
            closed,
        }
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |view, window, cx| {
            window.focus(&view.focus, cx)
        })
        .expect("focus settings");
    (state, closed, visual)
}

/// §1 Esc ladder: `Esc` in an open editor puts the row's value back, closes the box in place and
/// keeps the dialog; it no longer discards the whole draft.
#[gpui::test]
fn esc_reverts_an_open_editor_and_keeps_the_dialog(cx: &mut gpui::TestAppContext) {
    // Agents › Claude › Terminal command.
    let (state, closed, mut visual) = open_escape_fixture(cx, Section::Agents, 1);
    visual.simulate_keystrokes("enter");
    visual.simulate_input("-typed");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            let config = host.settings.config.as_ref().expect("loaded");
            assert_eq!(config.agent_commands.claude, "claude-typed");
            assert!(host.settings.dirty());
        })
    });
    visual.simulate_keystrokes("escape");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            let config = host.settings.config.as_ref().expect("loaded");
            assert_eq!(
                config.agent_commands.claude, "claude",
                "the edit is reverted"
            );
            assert!(host.settings_input.is_none(), "the editor closed in place");
            assert!(host.settings.editing.is_none());
            assert!(!host.settings.dirty());
            assert!(!host.settings.discard_armed, "a revert asks nothing");
            assert_eq!(host.settings.cursor_summary(), "Terminal command = claude");
        })
    });
    assert!(!closed.get(), "the dialog stays open");
}

/// §1 Esc ladder: the first `Esc` on a dirty draft arms and paints the amber strip; the second
/// discards, which is the shell closing the overlay.
#[gpui::test]
fn the_first_esc_on_a_dirty_draft_asks_and_the_second_discards(cx: &mut gpui::TestAppContext) {
    let (state, closed, mut visual) = open_escape_fixture(cx, Section::Sleep, 0);
    visual.simulate_keystrokes("space");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert!(host.settings.dirty());
            assert!(
                host.settings.discard_warning().is_none(),
                "dirty paints nothing"
            );
        })
    });
    visual.simulate_keystrokes("escape");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert!(host.settings.discard_armed);
            assert_eq!(
                host.settings.discard_warning(),
                Some("Unsaved changes. Press Esc again to discard them.")
            );
        })
    });
    assert!(!closed.get(), "the first Esc only asks");
    visual.simulate_keystrokes("escape");
    assert!(
        closed.get(),
        "the second Esc reaches the shell and discards"
    );
}

/// A clean draft closes at once: there is nothing to ask about.
#[test]
fn esc_on_a_clean_draft_closes_at_once() {
    let mut clean = draft();
    assert_eq!(clean.escape(), EscapeStep::Close);
    assert!(!clean.discard_armed);
}

/// Any edit after the question disarms it, so the next `Esc` asks about the draft as it now is.
#[test]
fn an_edit_disarms_the_discard_question() {
    let mut state = draft();
    state.section = Section::Jobs.index();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    state.prepared = rows(&state, &app).into();
    let config = state.config.as_mut().expect("loaded");
    toggle(config, &RowId::WarnBeforeQuit);
    assert_eq!(state.escape(), EscapeStep::Ask);
    assert!(state.discard_warning().is_some());

    let config = state.config.as_mut().expect("loaded");
    cycle(config, &RowId::KeepFinishedFor, 1, &Efforts::default());
    state.row = 1;
    state.update_selected();
    assert!(!state.discard_armed, "an edit disarms");
    assert!(state.discard_warning().is_none(), "the strip is gone again");
    assert_eq!(state.escape(), EscapeStep::Ask, "the next Esc asks again");
    assert_eq!(state.escape(), EscapeStep::Close);
}

fn catalogue() -> Models {
    Models::from_lists(
        vec![
            ModelOption {
                id: "claude-opus-5-5".to_owned(),
                name: "Opus 5.5".to_owned(),
            },
            ModelOption {
                id: "claude-sonnet-5".to_owned(),
                name: "Sonnet 5".to_owned(),
            },
        ],
        Vec::new(),
    )
}

/// §3 Model choice: a harness that reported models gets a dropdown (Harness default, then its
/// models by name); one that reported none keeps the text box, as does a row being typed into.
#[test]
fn the_model_row_draws_a_dropdown_only_when_the_harness_reported_models() {
    let mut state = draft();
    state.models = catalogue();
    state.section = Section::Agents.index();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    let list = rows(&state, &app);
    let claude = list
        .iter()
        .find(|row| row.id == RowId::ClaudeDefaultModel)
        .expect("claude's model row");
    let codex = list
        .iter()
        .find(|row| row.id == RowId::CodexDefaultModel)
        .expect("codex's model row");
    assert!(claude.kind.draws_model_dropdown(false));
    assert!(
        !claude.kind.draws_model_dropdown(true),
        "an id being typed draws the text box in place of the dropdown"
    );
    assert!(
        !codex.kind.draws_model_dropdown(false),
        "no catalogue: a text box"
    );
    assert_eq!(
        claude.detail.as_deref(),
        Some("The models claude reported. Harness default lets claude pick.")
    );
    assert_eq!(
        codex.detail.as_deref(),
        Some("Empty means the harness picks.")
    );
    assert!(matches!(&claude.kind, RowKind::Model { shown, .. } if shown == "Harness default"));

    // `→` steps Harness default → the models in order, never wrapping; the dropdown then reads
    // the model's name, and a typed id the catalogue lacks reads as itself.
    let models = catalogue();
    let config = state.config.as_mut().expect("loaded");
    cycle_model(config, AgentKind::Claude, 1, &models);
    assert_eq!(
        config.native_agents.claude.model.as_deref(),
        Some("claude-opus-5-5")
    );
    cycle_model(config, AgentKind::Claude, 1, &models);
    cycle_model(config, AgentKind::Claude, 1, &models);
    assert_eq!(
        config.native_agents.claude.model.as_deref(),
        Some("claude-sonnet-5")
    );
    assert!(matches!(
        model_of(config, AgentKind::Claude, &models),
        RowKind::Model { shown, .. } if shown == "Sonnet 5"
    ));
    select_model(config, AgentKind::Claude, None, &models);
    assert!(config.native_agents.claude.model.is_none());
    assert!(commit_value(
        config,
        &RowId::ClaudeDefaultModel,
        "claude-next"
    ));
    assert!(matches!(
        model_of(config, AgentKind::Claude, &models),
        RowKind::Model { shown, value, .. } if shown == "claude-next" && value == "claude-next"
    ));
}

/// §3 Keep-alive rule row: a rule whose pattern does not compile is drawn disabled, and its card
/// ends with the one danger sentence saying so.
#[test]
fn a_keep_alive_rule_with_a_broken_pattern_is_disabled_and_captioned() {
    let mut state = draft();
    state.section = Section::Sleep.index();
    let rules = state
        .config
        .as_ref()
        .map(|config| config.sleep.keep_alive.clone())
        .expect("loaded");
    let broken = rules
        .iter()
        .find(|rule| rule.kind == KeepAliveKind::Process)
        .expect("a process rule");
    state.matches = rules
        .iter()
        .map(|rule| KeepAliveRuleMatch {
            rule_id: rule.id.clone(),
            count: 2,
            error: (rule.id == broken.id).then(|| "regex parse error".to_owned()),
        })
        .collect();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    let list = rows(&state, &app);
    let rule_rows: Vec<&SettingRow> = list
        .iter()
        .filter(|row| matches!(row.id, RowId::KeepAliveRule(_)))
        .collect();
    assert_eq!(rule_rows.len(), rules.len());
    let broken_row = rule_rows
        .iter()
        .find(|row| row.label == broken.label)
        .expect("the broken rule's row");
    assert!(matches!(
        &broken_row.kind,
        RowKind::Rule {
            broken: true,
            running: None,
            badge: "command",
            ..
        }
    ));
    assert!(rule_rows.iter().all(|row| row.card == Some(KEEP_AWAKE)));
    assert_eq!(
        card_note(KEEP_AWAKE),
        Some("matched against running processes now")
    );
    let card: Vec<SettingRow> = list
        .iter()
        .filter(|row| row.card == Some(KEEP_AWAKE))
        .cloned()
        .collect();
    assert_eq!(
        card_captions(&card),
        vec![format!(
            "{}: the pattern does not compile, so the rule is skipped.",
            broken.label
        )]
    );
    // A healthy rule reads its live count; its switch is still the row's `Space`.
    assert!(rule_rows.iter().any(|row| matches!(
        &row.kind,
        RowKind::Rule {
            broken: false,
            running: Some(2),
            ..
        }
    )));
}

/// The search field's `shown/total` count divides by every row of every section.
#[test]
fn the_search_count_is_out_of_every_row_across_sections() {
    let state = draft();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    let every: usize = Section::ALL
        .iter()
        .map(|section| schema::section_rows(*section, &state, &app).len())
        .sum();
    let (hits, total) = schema::search("refresh", &state, &app);
    assert_eq!(total, every);
    assert_eq!(
        hits.iter()
            .map(|hit| (hit.section, hit.label.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (Section::Pool, "Refresh interval"),
            (Section::Status, "Local status refresh"),
            (Section::Status, "Remote status refresh"),
        ]
    );
}

/// Every editable row states one helper sentence (§3: "Every editable row carries a helper").
#[test]
fn every_editable_row_carries_a_helper_sentence() {
    let state = draft();
    let app = AppState::new("/tmp/fleet", std::time::Instant::now());
    for section in Section::ALL {
        for row in schema::section_rows(*section, &state, &app) {
            if row.id == RowId::ReadOnly || matches!(row.id, RowId::KeepAliveRule(_)) {
                continue;
            }
            let helper = row
                .detail
                .as_deref()
                .unwrap_or_else(|| panic!("{} has no helper", row.label));
            assert!(helper.ends_with('.'), "{helper:?} is a sentence");
        }
    }
}
