use super::*;

/// §3.8.6: landing on a text row never opens its editor, so `j` moves off it like any other
/// row; `⏎` opens the box in place, and only then does a bare `j` type. `esc` puts the value
/// the box opened with back.
#[gpui::test]
fn j_moves_off_a_text_row_until_enter_opens_its_box(cx: &mut gpui::TestAppContext) {
    struct SettingsInputHarness {
        state: Entity<AppState>,
        focus: FocusHandle,
    }

    impl gpui::Render for SettingsInputHarness {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let input = read_host(&self.state, cx, |host, _| host.board_settings_input.clone());
            let context = if input.is_some() {
                "BoardSettingsEditing"
            } else {
                "BoardSettings"
            };
            let state = self.state.clone();
            div().key_context("Dialog").child(
                div()
                    .key_context(context)
                    .track_focus(&self.focus)
                    .on_action(move |_: &settings_actions::MoveDown, _, cx| move_row(&state, 1, cx))
                    .children(input),
            )
        }
    }

    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/board-settings-input", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| host.board_settings = draft());
        materialize_input(&state, None, None, cx);
        read_host(&state, cx, |host, _| {
            assert!(
                host.board_settings_input.is_none(),
                "arriving on Name opens nothing"
            );
        });
    });
    let window = cx.add_window(|_, cx| SettingsInputHarness {
        state: state.clone(),
        focus: cx.focus_handle(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let root = window.root(&mut visual).expect("settings input harness");
    let root_focus = root.read_with(&visual, |view, _| view.focus.clone());
    visual.update(|window, cx| window.focus(&root_focus, cx));

    visual.simulate_keystrokes("j");
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.name, "Fleet", "nothing was typed");
            assert_eq!(host.board_settings.row, 1, "`j` moved off the text row");
        });
    });

    visual.update(|window, cx| {
        with_host(&state, cx, |host| host.board_settings.row = 0);
        assert!(
            confirm_row(&state, window, &root_focus, cx),
            "`⏎` is Name's"
        );
    });
    visual.simulate_keystrokes("j");
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.name, "Fleetj", "the open box types");
            assert_eq!(host.board_settings.row, 0);
        });
    });

    visual.update(|window, cx| {
        assert!(
            cancel(&state, window, &root_focus, cx),
            "`esc` stays in the dialog"
        );
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.name, "Fleet", "`esc` reverted the row");
            assert!(!host.board_settings.editing);
            assert!(
                host.board_settings_input.is_none(),
                "the box closed in place"
            );
        });
    });
}

use fleet_core::board::PropertySource;

fn draft() -> BoardSettingsState {
    BoardSettingsState {
        board_id: Some("work".parse().unwrap_or_else(|error| panic!("{error}"))),
        name: "Fleet".to_owned(),
        prefix: "FLT".to_owned(),
        backend_kind: "local".to_owned(),
        original_kind: "local".to_owned(),
        // Never the remembered section: `BoardSection::default()` reads a value this same
        // thread's other tests can have moved, and a draft that opened on Columns has a
        // different row vector entirely.
        section: BoardSection::General,
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
    assert!(GENERAL_ROWS.contains(&SettingRow::PushNewCards));
    assert_eq!(SettingRow::PushNewCards.label(), "Push new cards");
    let mut state = draft();
    state.row = GENERAL_ROWS
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
    let repos: Vec<RepoId> = vec![
        "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
        "acme/web".parse().unwrap_or_else(|error| panic!("{error}")),
    ];
    let gone: RepoId = "acme/gone"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
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

/// The Backend pane is the kind cycler and, under it, whatever that kind's schema declares.
#[test]
fn the_backend_rows_follow_the_kind_cycler() {
    let mut state = draft();
    state.section = BoardSection::Backend;
    assert_eq!(state.rows().len(), 1, "the kind cycler alone");
    state.select_backend("other", &schema());
    assert_eq!(state.rows().len(), 1 + 5);
    state.row = 1;
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
fn only_the_typing_rows_open_an_editor_and_only_on_enter() {
    let mut state = draft();
    state.select_backend("other", &schema());
    assert!(
        state.focused_text().is_none(),
        "arriving on Name opens nothing"
    );
    state.open_editor();
    assert_eq!(
        state.focused_text().as_deref(),
        Some("Fleet"),
        "`⏎` on Name"
    );
    state.editing = false;
    state.row = 2;
    state.open_editor();
    assert!(!state.editing, "the repo cycler has no box");
    let runs = GENERAL_ROWS
        .iter()
        .position(|row| *row == SettingRow::MaxLiveRuns)
        .unwrap_or_else(|| panic!("max live runs row"));
    state.row = runs;
    state.open_editor();
    assert_eq!(state.focused_text().as_deref(), Some("1"), "a number box");
    state.editing = false;
    state.section = BoardSection::Backend;
    state.row = 1;
    state.open_editor();
    assert!(state.focused_text().is_some(), "a text setting");
    state.editing = false;
    state.row = 1 + 4;
    state.open_editor();
    assert!(state.focused_text().is_none(), "a flag");
    assert!(state.rows[3].is_text(), "a number is typed into");
    assert!(!state.rows[4].is_text());
}

/// `Max live runs` is a number box now: typed digits land in the draft, and a number outside
/// 1–8 is kept so the row can state the rule it breaks, in the kit's words.
#[test]
fn a_typed_throttle_outside_the_range_states_its_rule_on_the_row() {
    let mut state = draft();
    state.row = GENERAL_ROWS
        .iter()
        .position(|row| *row == SettingRow::MaxLiveRuns)
        .unwrap_or_else(|| panic!("max live runs row"));
    state.open_editor();
    state.set_focused_text("3");
    assert_eq!(state.live_run_limit(), 3);
    assert_eq!(state.live_runs_rule(), None);
    state.set_focused_text("12");
    assert_eq!(
        state.live_runs_rule().as_deref(),
        Some("Must be between 1 and 8.")
    );
    state.set_focused_text("");
    assert_eq!(
        state.live_run_limit(),
        12,
        "an emptied box keeps the last number"
    );
    assert_eq!(state.escape(), EscapeStep::Editor);
    assert_eq!(state.live_run_limit(), 1, "`esc` put the opened value back");
}

#[test]
fn a_backend_row_stores_what_is_typed_into_it() {
    let mut state = draft();
    state.section = BoardSection::Backend;
    state.select_backend("other", &schema());
    state.row = 1;
    state.set_focused_text("SP");
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
    let row = state.row;
    state.select_backend(&state.backend_kind.clone(), &schema());
    state.row = row;
    assert_eq!(
        state.rows[0].value, "SP",
        "the board's own settings, not blanks"
    );
    assert_eq!(state.row, 1);
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
    assert!(
        draft
            .validate()
            .unwrap_or_else(|| panic!("invalid prefix should explain itself"))
            .contains("only")
    );
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
    state.set_focused_text("pay");
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

// ---------------------------------------------------------------------------------------
// The rail, the throttle and the Columns pane (contracts §5.4).
// ---------------------------------------------------------------------------------------

/// The board the column rules are asked about.
///
/// `validate_automation` and `apply_workflow_preset` take a whole `Board`, and `Board` has no
/// `Default` — every id on it is validated. The dialog keeps the board it was seeded from for
/// exactly this reason, and so does every test here.
#[track_caller]
fn board() -> Board {
    Board {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        context_id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        worktree_id: None,
        name: "Fleet".to_owned(),
        prefix: "FLT".to_owned(),
        next_number: 1,
        backend: BackendRef::default(),
        statuses: Vec::new(),
        labels: Vec::new(),
        properties: Vec::new(),
        default_repo_id: None,
        // Exactly what a `BoardSettingsState::default()` carries, so a draft nobody has typed
        // into is not "unsaved": `start_on_worktree` defaults to *true* on the model and to
        // `false` on a bare draft, and the two have to agree for `dirty()` to mean anything.
        settings: BoardSettings {
            start_on_worktree: false,
            push_new_cards: false,
            conflict_policy: ConflictPolicy::default(),
            max_live_runs: None,
            ..BoardSettings::default()
        },
        sync: fleet_core::board::SyncState::default(),
        created_at: String::new(),
        updated_at: String::new(),
        kind: Default::default(),
    }
}

/// A column with nothing on it but a name.
#[track_caller]
fn column(id: &str, name: &str) -> ColumnDraft {
    ColumnDraft::load(&Status {
        id: StatusId::try_from(id.to_owned()).unwrap_or_else(|error| panic!("{error}")),
        name: name.to_owned(),
        category: StatusCategory::Unstarted,
        color: None,
        automation: None,
    })
}

/// A three-column draft with the cursor in the Columns pane.
fn columns_draft() -> BoardSettingsState {
    let mut state = draft();
    state.section = BoardSection::Columns;
    state.columns = vec![
        column("todo", "Todo"),
        column("in-progress", "In Progress"),
        column("done", "Done"),
    ];
    state.original_columns = state.columns.clone();
    state.board = Some(std::rc::Rc::new(board()));
    state.prepare();
    state
}

/// The names the Columns list is showing, in order.
fn listed(state: &BoardSettingsState) -> Vec<String> {
    state.prepared.iter().map(|row| row.label.clone()).collect()
}

/// The fields the drilled-into column is showing, in order.
fn fields(state: &BoardSettingsState) -> Vec<&'static str> {
    state
        .prepared
        .iter()
        .filter_map(|row| match row.row {
            SettingRow::ColumnField(field) => Some(field.label()),
            _ => None,
        })
        .collect()
}

#[test]
fn each_section_draws_its_own_rows_and_nothing_else() {
    let mut state = columns_draft();
    assert_eq!(listed(&state).len(), 3);
    state.section = BoardSection::General;
    assert_eq!(state.rows(), GENERAL_ROWS.to_vec());
    state.section = BoardSection::Backend;
    assert_eq!(state.rows(), vec![SettingRow::Backend]);
}

/// The throttle clamps: `h` on one run must not wrap round to eight.
#[test]
fn max_live_runs_clamps_at_both_ends() {
    let mut state = draft();
    assert_eq!(state.live_run_limit(), 1, "unset is one run");
    state.step_live_runs(-1);
    assert_eq!(state.live_run_limit(), 1);
    for _ in 0..20 {
        state.step_live_runs(1);
    }
    assert_eq!(state.live_run_limit(), MAX_LIVE_RUNS_PER_BOARD);
}

/// The sentence is `fleet-core`'s, so a value an older CLI wrote is refused in its words.
#[test]
fn a_throttle_outside_the_range_is_refused_in_the_contract_s_words() {
    let mut state = columns_draft();
    state.max_live_runs = Some(9);
    assert_eq!(state.validate().as_deref(), Some("must be between 1 and 8"));
}

#[test]
fn the_columns_list_is_the_board_order_and_marks_the_automated_ones() {
    let mut state = columns_draft();
    assert_eq!(listed(&state), ["Todo", "In Progress", "Done"]);
    state.columns[1].on_enter = "prompt".to_owned();
    state.prepare();
    let marks: Vec<bool> = state
        .prepared
        .iter()
        .map(|row| {
            matches!(
                row.value,
                ColumnValue::Column {
                    has_action: true,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(marks, [false, true, false], "only the column that runs one");
}

/// `P` adds by id and never touches a column that is already there, so pressing it twice is
/// the same board as pressing it once.
#[test]
fn the_preset_is_idempotent_and_keeps_the_columns_already_present() {
    let mut columns = vec![column("todo", "My todo")];
    assert!(
        apply_preset(&board(), &mut columns),
        "the missing preset columns"
    );
    let after_once: Vec<String> = columns
        .iter()
        .map(|column| column.status.id.to_string())
        .collect();
    assert!(after_once.len() > 1);
    assert!(!apply_preset(&board(), &mut columns), "nothing left to add");
    let after_twice: Vec<String> = columns
        .iter()
        .map(|column| column.status.id.to_string())
        .collect();
    assert_eq!(after_once, after_twice);
    assert_eq!(
        columns
            .iter()
            .find(|column| column.status.id.as_str() == "todo")
            .map(|column| column.status.name.as_str()),
        Some("My todo"),
        "a column already on the board keeps its own name"
    );
}

#[test]
fn reordering_moves_the_column_and_nothing_else() {
    let mut state = columns_draft();
    let before = state.original_columns.clone();
    assert_eq!(reorder(&mut state.columns, 0, 1), Some(1));
    assert_eq!(
        state
            .columns
            .iter()
            .map(|column| column.status.name.as_str())
            .collect::<Vec<_>>(),
        ["In Progress", "Todo", "Done"]
    );
    assert_eq!(reorder(&mut state.columns, 2, 1), None, "past the end");
    assert_eq!(state.original_columns, before, "the board is untouched");
    assert!(state.dirty());
}

#[test]
fn the_seven_action_rows_come_and_go_with_on_enter() {
    let mut state = columns_draft();
    state.opened_column = Some(0);
    state.prepare();
    assert_eq!(
        fields(&state),
        [
            "Name",
            "Category",
            "On enter",
            "On success",
            "When unblocked"
        ]
    );
    state.columns[0].on_enter = "prompt".to_owned();
    state.prepare();
    assert_eq!(
        fields(&state),
        [
            "Name",
            "Category",
            "On enter",
            "Provider",
            "Model",
            "Effort",
            "Mode",
            "Instructions",
            "Expect",
            "Env",
            "On success",
            "When unblocked"
        ]
    );
    state.columns[0].on_enter = "none".to_owned();
    state.prepare();
    assert_eq!(fields(&state).len(), 5);
}

/// `h` / `l` step the three legal spellings, clamped like every other cycler here, and keep
/// whatever skill name was typed, so cycling an action off and back on does not lose it.
#[test]
fn the_on_enter_row_cycles_the_three_spellings_and_keeps_the_name() {
    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "skill:deep-review".to_owned();
    // The stored action is what remembers the name while the row does not say it.
    columns[0].status = columns[0]
        .status()
        .unwrap_or_else(|message| panic!("{message}"));
    let mut step = |delta| {
        assert!(cycle_field(
            &mut columns,
            0,
            ColumnField::OnEnter,
            delta,
            false,
            &Catalogue::default(),
        ));
        columns[0].on_enter.clone()
    };
    assert_eq!(step(-1), "prompt");
    assert_eq!(step(-1), "none");
    assert_eq!(step(-1), "none", "clamped, never wrapped");
    assert_eq!(step(1), "prompt");
    assert_eq!(step(1), "skill:deep-review", "the name survived");
    assert_eq!(step(1), "skill:deep-review", "clamped at the far end too");
}

/// A context or Jira board keeps its order and its names; everything that describes a run is
/// folded away under one callout rather than drawn disabled row by row (§5.4), so the cursor
/// only ever lands on the two rows the board can change.
#[test]
fn a_board_that_cannot_run_anything_folds_the_automation_rows_away() {
    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "prompt".to_owned();
    let labels = |locked| -> Vec<String> {
        prepare(&columns, Some(0), locked, &Catalogue::default())
            .into_iter()
            .map(|row| row.label)
            .collect()
    };
    assert_eq!(
        labels(true),
        ["Name", "Category"],
        "the seven rows are folded"
    );
    assert_eq!(
        labels(false).len(),
        12,
        "a worktree board draws every row of a column that runs something"
    );
}

/// The folded rows are the callout's, not the cursor's: `j` stops at Category and `⏎` opens
/// nothing there, whatever the column runs.
#[test]
fn the_cursor_never_reaches_a_folded_automation_row() {
    let mut state = columns_draft();
    state.automation_locked = true;
    state.columns[0].on_enter = "prompt".to_owned();
    state.opened_column = Some(0);
    state.prepare();
    assert_eq!(
        state.rows(),
        vec![
            SettingRow::ColumnField(ColumnField::Name),
            SettingRow::ColumnField(ColumnField::Category)
        ]
    );
    state.row = 1;
    state.open_editor();
    assert!(!state.editing, "Category is a choice, not a box");
}

/// A locked row refuses the arrows and the keyboard, not just the styling.
#[test]
fn a_locked_automation_row_cannot_be_cycled_or_typed_into() {
    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "prompt".to_owned();
    assert!(!cycle_field(
        &mut columns,
        0,
        ColumnField::Provider,
        1,
        true,
        &Catalogue::default(),
    ));
    set_field_text(&mut columns, 0, ColumnField::Expect, "anything", true);
    assert_eq!(
        columns[0].status().map(|status| status
            .automation
            .and_then(|automation| automation.on_enter)
            .map(|action| action.expect)),
        Ok(Some(String::new()))
    );
    set_field_text(&mut columns, 0, ColumnField::Name, "Review", true);
    assert_eq!(columns[0].status.name, "Review", "the name stays editable");
}

#[test]
fn every_routing_refusal_is_stated_in_the_contract_s_words() {
    let route = |from: usize, to: Option<&str>| {
        let mut columns = vec![
            column("todo", "Todo"),
            column("review", "In review"),
            column("done", "Done"),
        ];
        columns[from].status.automation = Some(ColumnAutomation {
            on_success: to.map(|id| {
                StatusId::try_from(id.to_owned()).unwrap_or_else(|error| panic!("{error}"))
            }),
            ..ColumnAutomation::default()
        });
        validate(&board(), &columns, 1)
    };
    // Every sentence names the column the route is on: the refusal is usually raised by an edit
    // to some *other* column — removing the target, or moving it — and the reader has to be told
    // which column carries the rule (`fleet-core::board::ops::validation`).
    assert_eq!(
        route(1, Some("nowhere")).as_deref(),
        Some("In review routes to nowhere, which is not a column on this board")
    );
    assert_eq!(
        route(1, Some("review")).as_deref(),
        Some("In review may not route to itself")
    );
    assert_eq!(
        route(1, Some("todo")).as_deref(),
        Some("In review routes to todo, which is not a later column")
    );
    assert_eq!(
        route(1, Some("done")),
        None,
        "forward is the legal direction"
    );
}

#[test]
fn every_action_refusal_is_stated_in_the_contract_s_words() {
    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "skill:".to_owned();
    assert_eq!(
        validate(&board(), &columns, 1).as_deref(),
        Some("a skill action needs a name")
    );

    columns[0].on_enter = "skill:deep-review".to_owned();
    set_field_text(&mut columns, 0, ColumnField::Expect, "reviewed", false);
    cycle_field(
        &mut columns,
        0,
        ColumnField::Provider,
        2,
        false,
        &Catalogue::default(),
    );
    assert_eq!(
        validate(&board(), &columns, 1).as_deref(),
        Some(
            "skill actions run on claude only; put the invocation in the column's instructions for codex"
        )
    );

    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "prompt".to_owned();
    columns[0].env = "PATH=/usr/bin".to_owned();
    assert!(
        validate(&board(), &columns, 1)
            .is_some_and(|message| message.starts_with("PATH is refused")),
        "the delegation's own env rule"
    );

    columns[0].env = "FLEET_CARD=x".to_owned();
    assert!(
        validate(&board(), &columns, 1).is_some_and(|message| message.contains("is refused")),
        "a reserved name"
    );

    columns[0].env = "OK=1".to_owned();
    assert_eq!(validate(&board(), &columns, 1), None);
}

/// The one refusal this pane owns: a spelling `--on-enter` would not take either.
#[test]
fn an_on_enter_spelling_that_is_none_of_the_three_is_refused() {
    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "run the thing".to_owned();
    assert_eq!(
        validate(&board(), &columns, 1).as_deref(),
        Some("on enter must be none, prompt, or skill:<name>[:<args>]")
    );
}

#[test]
fn a_column_without_a_name_stops_the_save() {
    let mut columns = vec![column("todo", "Todo")];
    columns[0].status.name = "   ".to_owned();
    assert_eq!(
        validate(&board(), &columns, 1).as_deref(),
        Some("a column needs a name")
    );
}

/// Everything the form typed into a column comes back out as the `Status` the patch carries.
#[test]
fn a_column_round_trips_through_the_status_the_patch_carries() {
    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "skill:deep-review:--fast".to_owned();
    set_field_text(
        &mut columns,
        0,
        ColumnField::Instructions,
        "Read it\nall",
        false,
    );
    set_field_text(
        &mut columns,
        0,
        ColumnField::Expect,
        "no blocking issue",
        false,
    );
    set_field_text(&mut columns, 0, ColumnField::Model, "opus", false);
    columns[0].env = "A=1\n\nB=2\n".to_owned();
    let status = columns[0]
        .status()
        .unwrap_or_else(|message| panic!("{message}"));
    let action = status
        .automation
        .and_then(|automation| automation.on_enter)
        .unwrap_or_else(|| panic!("the action"));
    assert_eq!(
        action.kind,
        ActionKind::Skill {
            name: "deep-review".to_owned(),
            args: "--fast".to_owned()
        }
    );
    assert_eq!(action.instructions, "Read it\nall");
    assert_eq!(action.expect, "no blocking issue");
    assert_eq!(action.agent.model.as_deref(), Some("opus"));
    assert_eq!(action.env, ["A=1", "B=2"], "blank lines are not entries");

    // A column the user emptied stops being automation at all, rather than holding the
    // document at version 2 with a block that asks for nothing.
    columns[0].on_enter = "none".to_owned();
    columns[0].status.automation = Some(ColumnAutomation::default());
    let status = columns[0]
        .status()
        .unwrap_or_else(|message| panic!("{message}"));
    assert_eq!(status.automation, None);
}

/// `esc` climbs out one level at a time and asks exactly once on the way (§5.4).
#[test]
fn escape_leaves_the_editor_then_the_column_then_asks_once() {
    let mut state = columns_draft();
    state.opened_column = Some(1);
    state.columns[1].status.name = "Doing".to_owned();
    state.prepare();
    state.open_editor();
    assert_eq!(state.escape(), EscapeStep::Editor);
    assert_eq!(state.escape(), EscapeStep::Column);
    assert_eq!(state.row, 1, "the cursor lands back on the column it left");
    assert_eq!(state.escape(), EscapeStep::Ask);
    assert_eq!(
        state.footer_strip(),
        Some(FooterStrip::Warning(UNSAVED_DRAFT.to_owned())),
        "the question is the amber strip"
    );
    assert_eq!(state.error, None, "and nothing went red");
    assert_eq!(state.escape(), EscapeStep::Close, "asked once, not twice");
}

/// Regression (§3.8.6): `esc` in an open editor reverts that row and stays in the column form;
/// before, it closed the editor keeping what was typed, and a second `esc` left the column.
#[test]
fn esc_in_an_open_editor_reverts_the_row_and_stays_in_the_column() {
    let mut state = columns_draft();
    state.opened_column = Some(1);
    state.prepare();
    state.row = 0;
    state.open_editor();
    state.set_focused_text("Doing");
    assert_eq!(
        state.columns[1].status.name, "Doing",
        "the box mirrors each keystroke"
    );
    assert_eq!(state.escape(), EscapeStep::Editor);
    assert_eq!(
        state.columns[1].status.name, "In Progress",
        "the row is reverted"
    );
    assert_eq!(state.opened_column, Some(1), "still in the column's form");
    assert!(!state.editing);
    assert!(!state.dirty(), "a reverted row leaves nothing unsaved");
    assert_eq!(state.escape(), EscapeStep::Column);
}

/// `⏎` keeps what was typed: the box closes in place over the new value, and only the draft's
/// own `esc` question stands between it and a discard.
#[test]
fn enter_in_an_open_editor_keeps_the_value() {
    let mut state = columns_draft();
    state.opened_column = Some(1);
    state.prepare();
    state.open_editor();
    state.set_focused_text("Doing");
    state.editing = false;
    assert_eq!(state.columns[1].status.name, "Doing");
    assert!(state.dirty());
}

/// Regression (§3.8.6): the permanent amber `Esc discards` line is gone. A dirty draft paints
/// no strip until the first `esc`, which asks in amber in these words; any edit takes it back.
#[test]
fn the_amber_strip_asks_only_after_the_first_esc_on_a_dirty_draft() {
    let mut state = columns_draft();
    assert_eq!(state.footer_strip(), None, "clean");
    state.columns[0].status.name = "Doing".to_owned();
    state.prepare();
    assert!(state.dirty());
    assert_eq!(state.footer_strip(), None, "merely dirty paints nothing");
    assert_eq!(state.escape(), EscapeStep::Ask);
    assert_eq!(
        state.footer_strip(),
        Some(FooterStrip::Warning(
            "Unsaved changes. Press Esc again to discard them.".to_owned()
        ))
    );
    state.opened_column = Some(0);
    state.prepare();
    state.row = 0;
    state.open_editor();
    state.set_focused_text("Later");
    assert_eq!(state.footer_strip(), None, "an edit disarmed the question");
}

/// A refusal outranks the question: the strip is red and says the rule verbatim.
#[test]
fn a_broken_rule_paints_the_strip_red_before_any_question() {
    let mut state = columns_draft();
    state.prefix = "fl t".to_owned();
    assert_eq!(
        state.footer_strip(),
        Some(FooterStrip::Error(
            "prefix must be A\u{2013}Z and 0\u{2013}9 only".to_owned()
        ))
    );
}

#[test]
fn a_clean_draft_closes_without_a_question() {
    let mut state = columns_draft();
    assert!(!state.dirty());
    assert_eq!(state.escape(), EscapeStep::Close);
}

#[gpui::test]
fn the_rail_cycles_and_remembers_where_it_was_left(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/board-settings-rail", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| host.board_settings = columns_draft());
        open_on_section(&state, BoardSection::General, cx);
        cycle_section(&state, 1, cx);
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.section, BoardSection::Backend);
        });
        cycle_section(&state, 1, cx);
        cycle_section(&state, 1, cx);
        read_host(&state, cx, |host, _| {
            assert_eq!(
                host.board_settings.section,
                BoardSection::Columns,
                "the rail clamps at its last section, as the global dialog's does"
            );
        });
        // A fresh draft opens where this session left the rail, which is what `,` does.
        assert_eq!(BoardSection::default(), BoardSection::Columns);
        // Left as it was found: the memory is this thread's, and every other test here pins
        // its own section rather than inheriting one.
        open_on_section(&state, BoardSection::General, cx);
    });
}

/// `docs/TESTING-HARNESS.md` §3: Board settings reports one non-editor field, `section`.
///
/// It is the only way a scenario can read which rail section is open — the section lives in the
/// dialog host's draft, which no `AppState` projection can reach.
#[gpui::test]
fn the_harness_reads_the_open_rail_section_as_a_dialog_field(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/board-settings-section", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| host.board_settings = columns_draft());
        state.update(cx, |app, _| {
            app.overlay = Some(crate::state::Overlay::Dialog(Dialogs::BoardSettings));
        });
        for section in [
            BoardSection::General,
            BoardSection::Backend,
            BoardSection::Columns,
        ] {
            open_on_section(&state, section, cx);
            let fields = crate::dialogs::dialog_fields(&state, cx);
            let first = fields.first().unwrap_or_else(|| panic!("a section field"));
            assert_eq!(first.name, "section");
            assert_eq!(first.value, section.title());
            assert!(!first.focused, "a rail section is not an editor");
        }
        // Board settings paints no `dialog.field[N]` target, so the leading non-editor cannot
        // put the documented field-to-target numbering out of step. With no row-scoped editor
        // mounted, the section is the whole of what this dialog reports.
        assert_eq!(crate::dialogs::dialog_fields(&state, cx).len(), 1);
        open_on_section(&state, BoardSection::General, cx);
    });
}

/// `docs/TESTING-HARNESS.md` §11: the one row-scoped editor rides beside `section`, named after
/// the row it belongs to.
///
/// Every other row of this dialog is a cycler whose value some projection already carries, so
/// the live editor is the only place a value typed into Board settings can be read back — which
/// is what lets `scenarios/board/workflow-columns.scenario` prove an `Effort` survived a save.
#[gpui::test]
fn the_harness_reads_the_open_row_editor_as_a_dialog_field(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let state = cx.new(|_| AppState::new("/tmp/board-settings-editor", std::time::Instant::now()));
    cx.update(|cx| {
        let mut draft = columns_draft();
        draft.columns[1].on_enter = "prompt".to_owned();
        set_field_text(
            &mut draft.columns,
            1,
            ColumnField::Effort,
            "blistering",
            false,
        );
        draft.opened_column = Some(1);
        draft.prepare();
        draft.row = draft
            .rows()
            .iter()
            .position(|row| *row == SettingRow::ColumnField(ColumnField::Effort))
            .unwrap_or_else(|| panic!("an Effort row on a column that runs something"));
        with_host(&state, cx, |host| host.board_settings = draft);
        state.update(cx, |app, _| {
            app.overlay = Some(crate::state::Overlay::Dialog(Dialogs::BoardSettings));
        });

        // A column row the cursor is merely *on* owns no editor: the Columns pane opens one on
        // `\u{23ce}`, which is what leaves `n`, `d`, `J`/`K` and `P` meaning themselves.
        materialize_input(&state, None, None, cx);
        assert_eq!(crate::dialogs::dialog_fields(&state, cx).len(), 1);

        with_host(&state, cx, |host| host.board_settings.editing = true);
        materialize_input(&state, None, None, cx);
        let fields = crate::dialogs::dialog_fields(&state, cx);
        assert_eq!(fields.len(), 2, "the section, then the editor beside it");
        assert_eq!(
            fields[1].name, "effort",
            "named after the row it belongs to"
        );
        assert_eq!(fields[1].value, "blistering");

        // A board that may not carry automation opens no editor over an automation row, so the
        // field goes away with it rather than reporting a value nobody can change.
        with_host(&state, cx, |host| {
            host.board_settings.automation_locked = true;
        });
        materialize_input(&state, None, None, cx);
        assert_eq!(crate::dialogs::dialog_fields(&state, cx).len(), 1);
    });
}

#[gpui::test]
fn the_list_keys_act_on_the_draft_and_send_nothing(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/board-settings-list", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| host.board_settings = columns_draft());
        add_column(&state, cx);
        read_host(&state, cx, |host, _| {
            let draft = &host.board_settings;
            assert_eq!(draft.columns.len(), 4);
            assert_eq!(draft.row, 3, "the cursor follows the new column");
            assert!(draft.dirty());
        });
        move_column(&state, -1, cx);
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.row, 2);
            assert_eq!(host.board_settings.columns[2].status.name, "New column");
        });
        preset(&state, cx);
        read_host(&state, cx, |host, _| {
            assert!(host.board_settings.notice.is_some(), "`P` says what it did");
        });
    });
}

/// A column nothing sits in leaves at once; one holding cards arms instead and names the
/// count, because §5.4 will not delete a card by deleting its column.
#[gpui::test]
fn deleting_a_column_with_cards_asks_where_they_go_first(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/board-settings-delete", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            let mut draft = columns_draft();
            draft.cards_by_column = std::rc::Rc::new(vec![(
                StatusId::try_from("in-progress".to_owned())
                    .unwrap_or_else(|error| panic!("{error}")),
                vec!["card-1".parse().unwrap_or_else(|error| panic!("{error}"))],
            )]);
            draft.row = 1;
            host.board_settings = draft;
        });
        arm_delete(&state, cx);
        read_host(&state, cx, |host, _| {
            let draft = &host.board_settings;
            assert_eq!(draft.pending_delete, Some(1), "armed, not deleted");
            assert_eq!(draft.columns.len(), 3);
            assert!(
                draft
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.contains("1 card")),
                "the count is named"
            );
        });
        // `esc` cancels the arming without touching the draft.
        with_host(&state, cx, |host| {
            assert_eq!(host.board_settings.escape(), EscapeStep::Delete);
        });
        // An empty column needs no target at all.
        with_host(&state, cx, |host| host.board_settings.row = 2);
        arm_delete(&state, cx);
        read_host(&state, cx, |host, _| {
            let draft = &host.board_settings;
            assert_eq!(draft.columns.len(), 2);
            assert_eq!(draft.pending_delete, None);
        });
    });
}

// ---------------------------------------------------------------------------------------
// The pointer (ADR 0023): every click is the key that would reach the same place.
// ---------------------------------------------------------------------------------------

struct PointerHarness {
    focus: FocusHandle,
}

impl gpui::Render for PointerHarness {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        div().track_focus(&self.focus)
    }
}

/// A window the pointer verbs can focus into, over `draft`.
fn pointer_harness(
    cx: &mut gpui::TestAppContext,
    draft: BoardSettingsState,
) -> (Entity<AppState>, FocusHandle, gpui::VisualTestContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let state = cx.new(|_| AppState::new("/tmp/board-settings-pointer", std::time::Instant::now()));
    cx.update(|cx| with_host(&state, cx, |host| host.board_settings = draft));
    let window = cx.add_window(|_, cx| PointerHarness {
        focus: cx.focus_handle(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let root = window.root(&mut visual).expect("pointer harness");
    let focus = root.read_with(&visual, |view, _| view.focus.clone());
    (state, focus, visual)
}

#[gpui::test]
fn a_click_on_an_option_lands_where_the_arrows_would(cx: &mut gpui::TestAppContext) {
    let (state, focus, mut visual) = pointer_harness(cx, draft());
    let runs = GENERAL_ROWS
        .iter()
        .position(|row| *row == SettingRow::MaxLiveRuns)
        .unwrap_or_else(|| panic!("max live runs row"));
    let start = GENERAL_ROWS
        .iter()
        .position(|row| *row == SettingRow::StartOnWorktree)
        .unwrap_or_else(|| panic!("start on worktree row"));

    visual.update(|window, cx| pick(&state, runs, 4, Some(0), &focus, window, cx));
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert_eq!(
                host.board_settings.row, runs,
                "the click moved the cursor there"
            );
            assert_eq!(
                host.board_settings.live_run_limit(),
                5,
                "option 4 is five runs"
            );
        });
    });
    visual.update(|window, cx| pick(&state, runs, 0, Some(4), &focus, window, cx));
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.live_run_limit(), 1)
        });
    });

    visual.update(|window, cx| switch(&state, start, true, &focus, window, cx));
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert!(host.board_settings.start_on_worktree);
            assert_eq!(host.board_settings.row, start);
        });
    });

    visual.update(|window, cx| {
        select_section(
            &state,
            &crate::bridge::Bridge::closed(),
            2,
            &focus,
            window,
            cx,
        )
    });
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.section, BoardSection::Columns);
        });
    });
}

#[gpui::test]
fn a_column_opens_on_double_click_and_its_choices_take_a_click(cx: &mut gpui::TestAppContext) {
    let (state, focus, mut visual) = pointer_harness(cx, columns_draft());

    visual.update(|window, cx| open_row(&state, 1, &focus, window, cx));
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.opened_column, Some(1));
            assert_eq!(fields(&host.board_settings)[2], "On enter");
        });
    });

    // On enter: `none` → `prompt`, which brings the seven action rows with it.
    visual.update(|window, cx| pick(&state, 2, 1, Some(0), &focus, window, cx));
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            let draft = &host.board_settings;
            assert_eq!(draft.columns[1].on_enter, "prompt");
            assert!(fields(draft).contains(&"Provider"));
        });
    });

    // Category: `unstarted` is option 1; `completed` is option 3.
    visual.update(|window, cx| pick(&state, 1, 3, Some(1), &focus, window, cx));
    visual.update(|_, cx| {
        read_host(&state, cx, |host, _| {
            assert_eq!(
                host.board_settings.columns[1].status.category,
                StatusCategory::Completed
            );
        });
    });
}

/// Seeds the dialog over a board shaped by `shape`, with one column that runs a prompt.
fn seeded_over(
    cx: &mut gpui::TestAppContext,
    shape: impl FnOnce(&mut Board),
) -> BoardSettingsState {
    let state = seeded_state(cx, shape);
    cx.update(|cx| read_host(&state, cx, |host, _| host.board_settings.clone()))
}

/// The app the dialog is seeded in, over a board shaped by `shape`, opened on the first
/// column's form: for the key functions, which act on the app rather than on a draft.
fn seeded_state(cx: &mut gpui::TestAppContext, shape: impl FnOnce(&mut Board)) -> Entity<AppState> {
    let mut shaped = board();
    let mut status = column("review", "Review").status;
    status.automation = Some(ColumnAutomation {
        on_enter: Some(Action {
            kind: ActionKind::Prompt,
            instructions: String::new(),
            expect: String::new(),
            env: Vec::new(),
            agent: ColumnAgentPrefs::default(),
        }),
        ..ColumnAutomation::default()
    });
    shaped.statuses = vec![status];
    shape(&mut shaped);
    let mut app = AppState::new(
        "/tmp/board-settings-run-location",
        std::time::Instant::now(),
    );
    app.board.view = Some(fleet_core::board::BoardView {
        board: shaped,
        cards: Vec::new(),
        live_runs: Vec::new(),
    });
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let state = cx.new(|_| app);
    cx.update(|cx| {
        open_on_section(&state, BoardSection::General, cx);
        seed(&state, cx);
        with_host(&state, cx, |host| {
            let draft = &mut host.board_settings;
            draft.section = BoardSection::Columns;
            draft.opened_column = Some(0);
            draft.prepare();
        });
    });
    state
}

/// Regression (§3.8.6, "a revert asks nothing"): on a board stored with no run limit the editor
/// opens on `1`, and `esc` wrote that `1` back as `Some(1)` — a value that means the same as the
/// stored `None` and read as unsaved, so the next `esc` asked and a save wrote a no-op.
#[gpui::test]
fn esc_in_the_max_live_runs_editor_leaves_an_unset_limit_clean(cx: &mut gpui::TestAppContext) {
    let mut state = seeded_over(cx, |board| board.settings.max_live_runs = None);
    state.section = BoardSection::General;
    state.opened_column = None;
    state.prepare();
    assert!(!state.dirty(), "opened clean");
    state.row = GENERAL_ROWS
        .iter()
        .position(|row| *row == SettingRow::MaxLiveRuns)
        .unwrap_or_else(|| panic!("max live runs row"));
    state.open_editor();
    state.set_focused_text("3");
    assert!(state.dirty(), "a typed limit is an edit");
    assert_eq!(state.escape(), EscapeStep::Editor);
    assert_eq!(state.live_run_limit(), 1, "`esc` put the opened value back");
    assert!(!state.dirty(), "a reverted row leaves nothing unsaved");
    assert!(!state.step_live_runs(-1), "`h` at one run changes nothing");
    assert!(
        !state.dirty(),
        "a step that lands where it started writes nothing"
    );
    assert!(state.step_live_runs(1));
    assert!(state.dirty());
}

/// Regression (§3.8.6): `h` / `l` cleared the question `esc` asked before knowing whether the
/// row could change, so a stray `l` on a column's Name made the second `esc` ask again instead of
/// discarding. Only a branch that changed the draft disarms it.
#[gpui::test]
fn a_stray_cycle_on_a_row_it_cannot_change_keeps_the_esc_question_armed(
    cx: &mut gpui::TestAppContext,
) {
    let state = seeded_state(cx, |_| {});
    let row_of = |draft: &BoardSettingsState, field: ColumnField| {
        draft
            .rows()
            .iter()
            .position(|row| *row == SettingRow::ColumnField(field))
            .unwrap_or_else(|| panic!("{field:?} row"))
    };
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            let draft = &mut host.board_settings;
            draft.columns[0].status.name = "Reviewing".to_owned();
            draft.prepare();
            assert!(draft.dirty());
            draft.discard_armed = true;
            draft.row = row_of(draft, ColumnField::Name);
        });
        cycle(&state, 1, cx);
        read_host(&state, cx, |host, _| {
            assert!(
                host.board_settings.discard_armed,
                "a row `l` cannot change is not an edit"
            );
        });
        with_host(&state, cx, |host| {
            let draft = &mut host.board_settings;
            draft.row = row_of(draft, ColumnField::Category);
        });
        cycle(&state, 1, cx);
        read_host(&state, cx, |host, _| {
            assert!(
                !host.board_settings.discard_armed,
                "a cycled category is an edit and takes the question back"
            );
        });
    });
}

/// A context board with no worktree runs in the context, and its header says so: `this
/// worktree` above a callout saying the board runs in the context contradicted itself.
#[gpui::test]
fn a_context_board_with_no_worktree_says_it_runs_in_the_context(cx: &mut gpui::TestAppContext) {
    let plain = seeded_over(cx, |_| {});
    assert_eq!(plain.runs_in(), "the context");
    assert_eq!(plain.subtitle(), "Fleet \u{b7} FLT \u{b7} the context");
    let reviews = seeded_over(cx, |board| {
        board.settings.run_location = RunLocation::CardWorktree;
    });
    assert_eq!(reviews.runs_in(), "each card's worktree");
}

/// The reveal scrolls the pane the least that shows the cursor's row, and it is the row it
/// shows: a card taller than the pane (a column's *When a card enters*) is "revealed" at its top
/// with its last rows still hidden, which is where `j` used to leave the cursor.
#[test]
fn the_reveal_moves_the_least_that_shows_the_row() {
    // Test geometry, not dialog geometry: the named-const rule (`dialogs::tests`) reads `px(`.
    let pixels = |value: f32| gpui::px(value);
    let rect = |top: f32, height: f32| {
        gpui::Bounds::new(
            gpui::point(gpui::Pixels::ZERO, pixels(top)),
            gpui::size(pixels(500.0), pixels(height)),
        )
    };
    let viewport = rect(100.0, 400.0);
    assert_eq!(
        reveal_delta(viewport, rect(200.0, 44.0)),
        gpui::Pixels::ZERO,
        "in view"
    );
    assert_eq!(
        reveal_delta(viewport, rect(480.0, 44.0)),
        pixels(-24.0),
        "below: up by what is hidden"
    );
    assert_eq!(
        reveal_delta(viewport, rect(80.0, 44.0)),
        pixels(20.0),
        "above: down by what is hidden"
    );
    assert_eq!(
        reveal_delta(viewport, rect(300.0, 600.0)),
        pixels(-200.0),
        "taller than the pane: its top shows"
    );
}

/// Whether the open column's automation rows are folded away (§5.4).
fn automation_folded(draft: &BoardSettingsState) -> bool {
    !draft
        .prepared
        .iter()
        .any(|row| matches!(row.row, SettingRow::ColumnField(field) if field.is_automation()))
}

/// BOARD §11.10: a Reviews board has no worktree of its own, but each of its runs executes in
/// its card's worktree, so its columns carry automation.
#[gpui::test]
fn the_columns_pane_is_editable_on_a_reviews_board(cx: &mut gpui::TestAppContext) {
    let draft = seeded_over(cx, |board| {
        board.kind = fleet_core::board::BoardKind::Reviews;
        board.settings.run_location = RunLocation::CardWorktree;
    });
    assert!(!draft.automation_locked);
    assert!(!automation_folded(&draft));
    assert!(
        draft.prepared.len() > 3,
        "a column that runs a prompt draws its action rows"
    );
}

/// A context board that runs in its own worktree has none to run in.
#[gpui::test]
fn the_columns_pane_is_locked_on_a_plain_context_board(cx: &mut gpui::TestAppContext) {
    let draft = seeded_over(cx, |_| {});
    assert!(draft.automation_locked);
    assert!(automation_folded(&draft));
}

/// General states where runs execute, read-only, from `settings.run_location`.
#[gpui::test]
fn the_general_pane_says_where_runs_execute(cx: &mut gpui::TestAppContext) {
    let reviews = seeded_over(cx, |board| {
        board.settings.run_location = RunLocation::CardWorktree;
    });
    assert_eq!(RUNS_IN_LABEL, "Runs in");
    assert_eq!(
        run_location_label(reviews.run_location),
        "each card's worktree"
    );
    let plain = seeded_over(cx, |_| {});
    assert_eq!(run_location_label(plain.run_location), "this worktree");
    assert!(
        !GENERAL_ROWS.contains(&SettingRow::NoRow),
        "the fact is not a cursor row: nothing on it can change"
    );
}

/// Regression (§3.8.6): the Columns list's footer is *New column* and *Apply preset*; Delete left
/// it for each row's hover action and menu item, and a drill-in's footer offers no verb at all.
#[test]
fn the_columns_footer_offers_new_column_and_apply_preset_only() {
    let mut state = columns_draft();
    let labels = |state: &BoardSettingsState| -> Vec<&'static str> {
        view::footer_verbs(state)
            .iter()
            .map(|verb| verb.label)
            .collect()
    };
    assert_eq!(labels(&state), ["New column", "Apply preset"]);
    assert!(
        view::COLUMN_VERBS
            .iter()
            .any(|verb| verb.label == "Delete column" && verb.harness.is_some()),
        "Delete is the row's hover action, and so its menu item"
    );
    state.opened_column = Some(0);
    state.prepare();
    assert!(
        labels(&state).is_empty(),
        "the breadcrumb is a form's way back"
    );
    state.opened_column = None;
    state.section = BoardSection::General;
    assert!(labels(&state).is_empty());
}

/// Every column verb that is a hover action is also a menu item, so nothing lives only behind
/// hover (ADR 0023); `Open` is the menu's twin of the double-click.
#[test]
fn a_column_row_s_hover_actions_are_all_in_its_menu() {
    let menu: Vec<&str> = view::COLUMN_VERBS.iter().map(|verb| verb.label).collect();
    assert_eq!(menu, ["Open", "Move up", "Move down", "Delete column"]);
    let hover: Vec<&str> = view::COLUMN_VERBS
        .iter()
        .filter(|verb| verb.harness.is_some())
        .map(|verb| verb.label)
        .collect();
    assert_eq!(hover, ["Move up", "Move down", "Delete column"]);
}

/// Regression (§5.4): the column form's breadcrumb replaces the mono `Columns › name` hint, and
/// its trailing caption says where the column sits in the board.
#[test]
fn the_column_breadcrumb_says_where_the_column_sits() {
    let state = columns_draft();
    let crumb = view::column_breadcrumb(&state.columns[1], 1, state.columns.len());
    assert_eq!(crumb.parent().as_ref(), "Columns");
    assert_eq!(
        crumb.shown_trailing().map(|trailing| trailing.as_ref()),
        Some("2 of 3")
    );
    assert!(crumb.goes_back(), "the crumb is the way back");
}

/// The list row says what entering the column runs and where a success sends the card, in
/// the helper under its name.
#[test]
fn a_column_list_row_states_its_category_its_run_and_its_route() {
    let mut state = columns_draft();
    state.columns[1].on_enter = "skill:deep-review".to_owned();
    state.columns[1].status.automation = Some(ColumnAutomation {
        on_success: Some(
            StatusId::try_from("done".to_owned()).unwrap_or_else(|error| panic!("{error}")),
        ),
        ..ColumnAutomation::default()
    });
    state.prepare();
    let helpers: Vec<String> = state
        .prepared
        .iter()
        .filter_map(|row| match &row.value {
            ColumnValue::Column { helper, .. } => Some(helper.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        helpers,
        [
            "unstarted",
            "unstarted \u{b7} \u{26a1} agent runs skill deep-review \u{b7} then \u{2192} Done",
            "unstarted",
        ]
    );
}

/// With a catalogue the Model row is a dropdown ending in `Other model id…`, and `h` / `l` walk
/// the declared models; without one it stays a text box, as before.
#[test]
fn the_model_row_offers_the_catalogue_and_types_what_it_lacks() {
    let catalogue =
        Catalogue::with_models(AgentKind::Claude, &[("opus", "Opus"), ("sonnet", "sonnet")]);
    let mut columns = vec![column("review", "In review")];
    columns[0].on_enter = "prompt".to_owned();
    cycle_field(&mut columns, 0, ColumnField::Provider, 1, false, &catalogue);
    let model = |columns: &[ColumnDraft], catalogue: &Catalogue| {
        prepare(columns, Some(0), false, catalogue)
            .into_iter()
            .find(|row| row.row == SettingRow::ColumnField(ColumnField::Model))
            .map(|row| row.value)
    };
    match model(&columns, &catalogue) {
        Some(ColumnValue::Choice {
            options,
            details,
            at,
            other,
            ..
        }) => {
            assert_eq!(
                options,
                ["column default", "Opus", "sonnet", catalogue::OTHER_MODEL]
            );
            assert_eq!(details[1], "opus", "a name shows its id beside it");
            assert_eq!(at, Some(0));
            assert!(other);
        }
        other => panic!("a dropdown, not {other:?}"),
    }
    assert!(cycle_field(
        &mut columns,
        0,
        ColumnField::Model,
        1,
        false,
        &catalogue
    ));
    assert!(matches!(
        model(&columns, &catalogue),
        Some(ColumnValue::Choice { at: Some(1), .. })
    ));
    set_field_text(&mut columns, 0, ColumnField::Model, "opus-next", false);
    assert!(matches!(
        model(&columns, &catalogue),
        Some(ColumnValue::Choice { at: None, ref value, .. }) if value == "opus-next"
    ));
    assert!(matches!(
        model(&columns, &Catalogue::default()),
        Some(ColumnValue::Text { .. })
    ));
}
