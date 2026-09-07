use fleet_core::config::default_config;

use super::*;

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
fn numbers_clamp_to_their_minimum() {
    let mut state = draft();
    let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
    commit_value(config, &RowId::StatusRefreshMs, "10");
    assert_eq!(config.ui.status_refresh_ms, 500);
    commit_value(config, &RowId::StatusRefreshMs, "4000");
    assert_eq!(config.ui.status_refresh_ms, 4_000);
    commit_value(config, &RowId::GraceMs, "not a number");
    assert_eq!(config.sleep.grace_ms, 0);
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
    assert_eq!(format_duration(600_000), "10 min");
    assert_eq!(format_duration(3_600_000), "1 h");
    assert_eq!(format_duration(30_000), "30 s");
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
