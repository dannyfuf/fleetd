use super::*;
use fleet_core::board::PropertySource;

fn draft() -> BoardSettingsState {
    BoardSettingsState {
        board_id: Some("work".parse().unwrap_or_else(|error| panic!("{error}"))),
        name: "Fleet".to_owned(),
        prefix: "FLT".to_owned(),
        backend_kind: "local".to_owned(),
        original_kind: "local".to_owned(),
        ..BoardSettingsState::default()
    }
}

fn entry(key: &str, name: &str, kind: PropertyKind) -> PropertySchema {
    PropertySchema {
        key: key.to_owned(),
        name: name.to_owned(),
        kind,
        options: Vec::new(),
        editable: true,
        source: PropertySource::Backend,
        show_on_card: false,
    }
}

/// A schema shaped like a real backend's, without naming one: every kind the dialog draws.
fn schema() -> Vec<PropertySchema> {
    vec![
        entry("project", "Project key (required)", PropertyKind::Text),
        entry("jql", "Extra filter", PropertyKind::Text),
        entry("statuses", "Columns", PropertyKind::MultiSelect),
        entry("overlapMinutes", "Overlap", PropertyKind::Number),
        entry("archived", "Include archived", PropertyKind::Bool),
    ]
}

#[test]
fn a_schema_becomes_one_row_per_key_with_the_stored_value_in_it() {
    let settings = serde_json::json!({
        "project": "SP",
        "statuses": ["To Do", "Done"],
        "overlapMinutes": 5,
        "extraFields": [{"id": "customfield_1"}]
    });
    let rows = backend_rows(&schema(), &settings);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].value, "SP");
    assert!(rows[0].required, "the marker is the only `required` signal");
    assert_eq!(
        rows[0].name, "Project key",
        "the marker is not part of the label"
    );
    assert_eq!(rows[1].value, "", "an absent optional key reads as empty");
    assert!(!rows[1].required);
    assert_eq!(rows[2].value, "To Do, Done");
    assert_eq!(rows[3].value, "5");
    assert!(!rows[4].present, "an absent flag is not `false`");
}

#[test]
fn rows_write_typed_json_and_keep_the_keys_the_schema_never_names() {
    let settings = serde_json::json!({
        "project": "SP",
        "overlapMinutes": 5,
        "extraFields": [{"id": "customfield_1"}]
    });
    let mut rows = backend_rows(&schema(), &settings);
    rows[0].value = "  PAY  ".into();
    rows[2].value = "To Do , , Done ".into();
    rows[3].value = "8".into();
    let written = rows_to_settings(&settings, &rows);
    assert_eq!(
        written,
        serde_json::json!({
            "project": "PAY",
            "statuses": ["To Do", "Done"],
            // A number is written as a number: a backend deserializing `usize` refuses
            // `"8"` and refuses `8.0` just as hard.
            "overlapMinutes": 8,
            // A setting only the CLI can write survives a pass through this dialog.
            "extraFields": [{"id": "customfield_1"}]
        })
    );
}

#[test]
fn an_emptied_row_removes_its_key_rather_than_writing_an_empty_string() {
    let settings = serde_json::json!({"project": "SP", "jql": "sprint in openSprints()"});
    let mut rows = backend_rows(&schema(), &settings);
    assert_eq!(rows[1].value, "sprint in openSprints()");
    rows[1].value = "   ".into();
    let written = rows_to_settings(&settings, &rows);
    assert_eq!(written, serde_json::json!({"project": "SP"}));
    // An absent optional key is `None` to the backend; `""` is a setting it has to honour.
    assert!(written.get("jql").is_none());
}

#[test]
fn a_flag_is_written_only_once_it_means_something() {
    let mut rows = backend_rows(&schema(), &serde_json::Value::Null);
    assert_eq!(
        rows[4].json(),
        None,
        "a flag nobody touched overrides no default"
    );
    rows[4].value = "true".into();
    rows[4].present = true;
    assert_eq!(rows[4].json(), Some(serde_json::json!(true)));
    rows[4].value = "false".into();
    assert_eq!(
        rows[4].json(),
        Some(serde_json::json!(false)),
        "a flag turned off is an opinion, not an absence"
    );
}

#[test]
fn an_empty_required_row_and_a_non_numeric_one_stop_the_save() {
    let mut rows = backend_rows(&schema(), &serde_json::Value::Null);
    assert_eq!(
        rows_error(&rows).as_deref(),
        Some("Project key is required"),
        "the message names the row, not the JSON key"
    );
    rows[0].value = "SP".into();
    assert_eq!(rows_error(&rows), None);
    rows[3].value = "many".into();
    assert_eq!(
        rows_error(&rows).as_deref(),
        Some("Overlap must be a whole number")
    );
    // A number row refuses the letter, which is what leaves `h`/`l` their cycling meaning.
    assert!(!rows[3].accepts("h"));
    assert!(rows[3].accepts("7"));
    assert!(rows[1].accepts("h"));
    assert!(!rows[4].accepts("h"), "a flag is not typed into");
}

/// `h` is off and `l` is on: a row that flipped on both made `h l` invert the setting.
#[test]
fn hl_on_a_flag_row_sets_a_side_rather_than_flipping() {
    let mut state = draft();
    state.rows = backend_rows(&schema(), &serde_json::Value::Null);
    let flag = state.rows.len() - 1;
    assert_eq!(state.rows[flag].kind, PropertyKind::Bool);
    cycle_backend_row(&mut state, flag, 1);
    assert!(state.rows[flag].flag());
    cycle_backend_row(&mut state, flag, 1);
    assert!(state.rows[flag].flag(), "`l` twice is still on");
    cycle_backend_row(&mut state, flag, -1);
    assert!(!state.rows[flag].flag());
    cycle_backend_row(&mut state, flag, -1);
    assert!(!state.rows[flag].flag(), "`h` twice is still off");
    assert!(
        state.rows[flag].present,
        "a flag the user touched is a setting the board has an opinion about"
    );
}

/// `pushNewCards` used to be reachable from no app surface at all, and it defaults to off:
/// every `c` on a linked board made a card that never became an issue, silently.
#[test]
fn the_push_new_cards_row_is_drawn_and_carried_into_the_saved_settings() {
    assert!(FIXED_ROWS.contains(&SettingRow::PushNewCards));
    assert_eq!(SettingRow::PushNewCards.label(), "Push new cards");
    let mut state = draft();
    state.row = FIXED_ROWS
        .iter()
        .position(|row| *row == SettingRow::PushNewCards)
        .unwrap_or_else(|| panic!("the row is drawn"));
    assert_eq!(state.focused(), SettingRow::PushNewCards);
    assert!(!state.push_new_cards, "the board default is off");
    // It sits with the other board-level flag, ahead of the backend's own rows.
    assert_eq!(state.row, 4);
    assert!(state.focused_backend_row().is_none());
}

/// A row the user never touched writes back exactly what it read. The `,` a multi-select
/// joins and splits on has no escape, so a stored column named `Blocked, waiting` came back
/// as two — silently rewritten by a Save that was about some other row entirely.
#[test]
fn an_untouched_row_is_written_back_unchanged_however_it_is_spelled() {
    let settings = serde_json::json!({
        "project": "SP",
        "statuses": ["To Do", "Blocked, waiting"],
    });
    let rows = backend_rows(&schema(), &settings);
    assert_eq!(rows_to_settings(&settings, &rows), settings);
    // A row the user *did* touch is written as the rows spell it.
    let mut edited = rows.clone();
    edited[2].value = "To Do, Done".to_owned();
    assert_eq!(
        rows_to_settings(&settings, &edited)["statuses"],
        serde_json::json!(["To Do", "Done"])
    );
    // A shape no row can draw is preserved rather than flattened into its own debug text.
    let odd = serde_json::json!({"project": "SP", "jql": {"saved": 42}});
    assert_eq!(rows_to_settings(&odd, &backend_rows(&schema(), &odd)), odd);
}

/// A stored repository this context no longer lists sits on no position of the cycler.
/// Drawn as if it were "none", the row said `h` would do nothing and `l` would move one
/// step, while the value on screen was the repository id itself — and `h` wrapped to the
/// *last* repository rather than clearing it.
#[test]
fn a_repository_the_context_no_longer_lists_is_off_the_cycler_s_grid() {
    let repos: Vec<RepoId> = vec!["acme/api".parse().unwrap(), "acme/web".parse().unwrap()];
    let gone: RepoId = "acme/gone".parse().unwrap();
    assert!(repo_listed(&repos, None));
    assert!(repo_listed(&repos, Some(&repos[1])));
    assert!(!repo_listed(&repos, Some(&gone)));
    // On the grid, the positions are "none" then the list in order.
    assert_eq!(repo_position(&repos, None), 0);
    assert_eq!(repo_position(&repos, Some(&repos[1])), 2);
}

/// The required rule reads what the row *writes*: separators alone write nothing.
#[test]
fn a_required_multi_select_of_separators_alone_is_refused_rather_than_dropped() {
    let mut schema = schema();
    schema[2].name = "Columns (required)".to_owned();
    let mut rows = backend_rows(&schema, &serde_json::json!({"project": "SP"}));
    rows[2].value = format!("{MULTI_SEPARATOR} {MULTI_SEPARATOR}");
    assert_eq!(rows[2].json(), None, "the row writes no value");
    assert_eq!(
        rows_error(&rows).as_deref(),
        Some("Columns is required"),
        "a save that shipped settings missing a required key would have gone out"
    );
    rows[2].value = format!("To Do{MULTI_SEPARATOR}Done");
    assert_eq!(rows_error(&rows), None);
}

#[test]
fn changing_the_kind_starts_from_empty_settings_and_coming_back_restores_them() {
    let mut state = draft();
    state.original_settings = serde_json::json!({"project": "SP"});
    state.rows = backend_rows(&schema(), &state.original_settings);
    state.select_backend("other", &schema());
    assert!(!state.keeps_kind());
    assert_eq!(
        state.rows[0].value, "",
        "settings never cross a kind change"
    );
    // Nothing configured is `null`, which is what an unconfigured board stores: an empty
    // object would differ from it and make every save a write.
    assert_eq!(state.settings_json(), serde_json::Value::Null);
    state.select_backend("local", &schema());
    assert!(state.keeps_kind());
    assert_eq!(state.rows[0].value, "SP");
    assert_eq!(state.settings_json(), serde_json::json!({"project": "SP"}));
}

#[test]
fn the_backend_rows_follow_the_fixed_ones() {
    let mut state = draft();
    assert_eq!(state.rows().len(), FIXED_ROWS.len());
    state.select_backend("other", &schema());
    assert_eq!(state.rows().len(), FIXED_ROWS.len() + 5);
    state.row = FIXED_ROWS.len();
    assert_eq!(state.focused(), SettingRow::BackendSetting(0));
    assert_eq!(
        state.focused_backend_row().map(|row| row.key.as_str()),
        Some("project")
    );
    // A shorter schema cannot leave the cursor pointing past the end.
    state.select_backend("smaller", &[entry("a", "A", PropertyKind::Text)]);
    assert!(state.row < state.rows().len());
}

#[test]
fn only_the_typing_rows_take_a_caret() {
    let mut state = draft();
    state.select_backend("other", &schema());
    assert!(state.input().is_some(), "name");
    state.row = 2;
    assert!(state.input().is_none(), "the repo cycler");
    state.row = FIXED_ROWS.len();
    assert!(state.input().is_some(), "a text setting");
    state.row = FIXED_ROWS.len() + 4;
    assert!(state.input().is_none(), "a flag");
    assert!(state.rows[3].is_text(), "a number is typed into");
    assert!(!state.rows[4].is_text());
}

#[test]
fn a_backend_row_stores_what_is_typed_into_it() {
    let mut state = draft();
    state.select_backend("other", &schema());
    state.row = FIXED_ROWS.len();
    let mut input = state.input().unwrap_or_else(|| panic!("a text row"));
    input.insert("SP");
    state.set_input(&input);
    assert_eq!(state.rows[0].value, "SP");
    assert_eq!(state.settings_json(), serde_json::json!({"project": "SP"}));
}

#[test]
fn a_schema_that_arrives_late_fills_the_rows_without_moving_the_cursor() {
    // `,` on the first frame after a reconnect seeds the dialog before the registry has
    // answered. What arrives late has to land, or the dialog shows a backend with no
    // settings for as long as it stays open.
    let mut state = draft();
    state.original_settings = serde_json::json!({"project": "SP"});
    state.rows = backend_rows(&[], &state.original_settings);
    assert!(state.rows.is_empty());
    state.row = 1;
    state.caret = 2;
    let (row, caret) = (state.row, state.caret);
    state.select_backend(&state.backend_kind.clone(), &schema());
    state.row = row;
    state.caret = caret;
    assert_eq!(
        state.rows[0].value, "SP",
        "the board's own settings, not blanks"
    );
    assert_eq!((state.row, state.caret), (1, 2));
}

#[test]
fn the_backend_cycler_always_offers_the_board_s_own_kind() {
    let state = AppState::new("/tmp/fleet-board-settings", std::time::Instant::now());
    assert_eq!(backend_kinds(&state, "remote"), vec!["remote".to_owned()]);
    assert!(schema_for(&state, "remote").is_empty());
}

#[test]
fn non_ascii_prefix_reports_charset_before_length() {
    let draft = BoardSettingsState {
        name: "Board".into(),
        prefix: "界界界界".into(),
        ..Default::default()
    };
    assert!(draft.validate().unwrap().contains("only"));
}

#[test]
fn the_prefix_rule_is_stated_exactly() {
    let mut state = draft();
    assert_eq!(state.validate(), None);
    state.prefix = "fl t".to_owned();
    assert_eq!(
        state.validate().as_deref(),
        Some("prefix must be A\u{2013}Z and 0\u{2013}9 only")
    );
    state.prefix = "TOOOOOLONG".to_owned();
    assert!(state.validate().is_some());
    state.prefix = String::new();
    assert!(state.validate().is_some());
    state.prefix = "FLT".to_owned();
    state.name = "  ".to_owned();
    assert_eq!(state.validate().as_deref(), Some("name must not be empty"));
}

#[test]
fn a_prefix_is_stored_the_way_it_is_shown() {
    let mut state = draft();
    state.row = 1;
    let mut input = state
        .input()
        .unwrap_or_else(|| panic!("prefix is a text row"));
    input.clear();
    input.insert("pay");
    state.set_input(&input);
    assert_eq!(state.prefix, "PAY");
}

#[test]
fn the_repo_cycler_puts_none_first() {
    let repos: Vec<RepoId> = ["a/one", "a/two"]
        .iter()
        .map(|id| RepoId::try_from(*id).unwrap_or_else(|error| panic!("{error}")))
        .collect();
    assert_eq!(repo_position(&repos, None), 0);
    assert_eq!(repo_position(&repos, Some(&repos[1])), 2);
}

/// `backend_element` refuses to draw an unset number as `0`, so `h` must not write one.
#[test]
fn stepping_down_an_unset_number_row_leaves_it_unset() {
    let mut draft = BoardSettingsState {
        rows: vec![BackendRow {
            key: "fullSyncEvery".into(),
            name: "Full sync every".into(),
            kind: PropertyKind::Number,
            options: Vec::new(),
            required: false,
            value: String::new(),
            present: false,
        }],
        ..Default::default()
    };
    cycle_backend_row(&mut draft, 0, -1);
    assert_eq!(draft.rows[0].value, "");
    assert_eq!(
        draft.rows[0].json(),
        None,
        "`0` here makes every pull a full one, and nothing validates it"
    );
    // Stepping up from empty is a value the user asked for, and comes back down again.
    cycle_backend_row(&mut draft, 0, 1);
    assert_eq!(draft.rows[0].value, "1");
    cycle_backend_row(&mut draft, 0, -1);
    assert_eq!(draft.rows[0].value, "0");
}
