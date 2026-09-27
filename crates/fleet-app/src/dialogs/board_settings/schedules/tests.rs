use super::*;
use crate::{bridge::Bridge, state::Overlay};
use fleet_core::{
    board::{BoardKind, BoardView},
    schedule::ScheduleRun,
};
use fleet_proto::response::SCHEDULES_CAPABILITY;

#[track_caller]
fn board_id() -> BoardId {
    "work".parse().unwrap_or_else(|error| panic!("{error}"))
}

#[track_caller]
fn board(kind: BoardKind) -> Board {
    Board {
        id: board_id(),
        context_id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        worktree_id: None,
        name: "Reviews".to_owned(),
        prefix: "REV".to_owned(),
        next_number: 1,
        backend: BackendRef::default(),
        statuses: Vec::new(),
        labels: Vec::new(),
        properties: Vec::new(),
        default_repo_id: None,
        settings: BoardSettings::default(),
        sync: fleet_core::board::SyncState::default(),
        created_at: String::new(),
        updated_at: String::new(),
        kind,
    }
}

#[track_caller]
fn schedule(id: &str, name: &str) -> Schedule {
    Schedule {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        board_id: board_id(),
        name: name.to_owned(),
        prompt: "look for reviews".to_owned(),
        cadence: Cadence::Every { minutes: 15 },
        agent: ScheduleAgent::default(),
        enabled: true,
        timeout_minutes: 20,
        created_at: "2026-09-23T08:00:00Z".to_owned(),
        updated_at: "2026-09-23T08:00:00Z".to_owned(),
        runs: Vec::new(),
        next_run_at: None,
    }
}

/// An app showing one board of `kind`, on a daemon with or without `schedules`, holding
/// `schedules` in the mirror and Board settings as its overlay.
fn app(kind: BoardKind, supported: bool, schedules: Vec<Schedule>) -> AppState {
    let mut app = AppState::new("/tmp/board-settings-schedules", std::time::Instant::now());
    if supported {
        app.daemon_capabilities
            .insert(SCHEDULES_CAPABILITY.to_owned());
    }
    app.board.view = Some(BoardView {
        board: board(kind),
        cards: Vec::new(),
        live_runs: Vec::new(),
    });
    app.schedules.apply(&board_id(), schedules);
    app.overlay = Some(Overlay::Dialog(Dialogs::BoardSettings));
    app
}

/// Seeds the dialog over `app` and puts it on the Schedules list.
fn open(cx: &mut gpui::TestAppContext, app: AppState) -> Entity<AppState> {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let state = cx.new(|_| app);
    cx.update(|cx| {
        seed(&state, cx);
        with_host(&state, cx, |host| {
            host.open = Some(Dialogs::BoardSettings);
            let draft = &mut host.board_settings;
            if draft.schedules_supported {
                draft.open_section(false, Local::now());
            }
        });
    });
    state
}

fn draft(state: &Entity<AppState>, cx: &mut gpui::TestAppContext) -> BoardSettingsState {
    cx.update(|cx| read_host(state, cx, |host, _| host.board_settings.clone()))
}

#[gpui::test]
fn the_section_is_hidden_without_the_capability(cx: &mut gpui::TestAppContext) {
    let state = open(cx, app(BoardKind::Reviews, false, Vec::new()));
    let seeded = draft(&state, cx);
    assert!(!seeded.sections().contains(&BoardSection::Schedules));
    assert_eq!(seeded.sections(), BoardSection::WITHOUT_SCHEDULES);
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.board_settings.section = BoardSection::Columns;
        });
        cycle_section(&state, 1, cx);
        read_host(&state, cx, |host, _| {
            assert_eq!(
                host.board_settings.section,
                BoardSection::Columns,
                "`⇥` never lands on a hidden section"
            );
        });
        // `T` on such a daemon says why instead of opening an empty pane.
        open_schedules_section(&state, false, cx);
        read_host(&state, cx, |host, _| {
            assert_eq!(host.board_settings.section, BoardSection::Columns);
            assert_eq!(
                host.board_settings.notice.as_deref(),
                Some(SCHEDULES_UNSUPPORTED)
            );
        });
        open_on_section(&state, BoardSection::General, cx);
    });

    let state = open(cx, app(BoardKind::Reviews, true, Vec::new()));
    assert!(
        draft(&state, cx)
            .sections()
            .contains(&BoardSection::Schedules)
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[test]
fn the_list_states_each_schedule_s_facts() {
    let now = Local::now();
    let next = (now + chrono::Duration::minutes(5)).with_timezone(&Utc);
    let mut first = schedule("sch-00000001", "GitHub reviews");
    first.next_run_at = Some(next.to_rfc3339());
    first.runs.push(ScheduleRun {
        job_id: None,
        started_at: "2026-09-23T08:00:00Z".to_owned(),
        ended_at: Some("2026-09-23T08:01:00Z".to_owned()),
        outcome: Some(ScheduleOutcome::Succeeded),
        summary: Some("3 created, 5 existing".to_owned()),
        cost_usd: None,
        log_path: Some("/tmp/log".to_owned()),
    });
    let mut second = schedule("sch-00000002", "Chat");
    second.enabled = false;
    second.cadence = Cadence::Every { minutes: 120 };
    second.runs.push(ScheduleRun {
        job_id: None,
        started_at: "2026-09-23T08:00:00Z".to_owned(),
        ended_at: None,
        outcome: Some(ScheduleOutcome::TimedOut),
        summary: None,
        cost_usd: None,
        log_path: None,
    });

    let rows = prepare_list(&[first.clone(), second.clone()], now);
    let agent = ScheduleAgent::default();
    let who = format!(
        "{} \u{b7} {}",
        provider_word(agent.provider),
        mode_label(agent.mode)
    );
    assert_eq!(rows[0].helper, format!("every 15 min \u{b7} {who}"));
    let next_line = format!("next {}", next.with_timezone(&Local).format("%H:%M"));
    assert_eq!(rows[0].next.as_deref(), Some(next_line.as_str()));
    assert_eq!(
        rows[0].status, next_line,
        "an enabled schedule says when it fires"
    );
    let (outcome, summary) = rows[0].last.clone().unwrap_or_else(|| panic!("a last run"));
    assert_eq!(outcome.icon, Icon::CircleCheck);
    assert_eq!(outcome.tone, Tone::Success);
    assert_eq!(summary, "3 created, 5 existing");

    assert_eq!(rows[1].helper, format!("every 2 h \u{b7} {who}"));
    assert_eq!(
        rows[1].status, "disabled",
        "a disabled schedule says so first"
    );
    assert_eq!(rows[1].next, None);
    let (outcome, summary) = rows[1].last.clone().unwrap_or_else(|| panic!("a last run"));
    assert_eq!(outcome.icon, Icon::CircleX);
    assert_eq!(outcome.tone, Tone::Danger);
    assert_eq!(
        summary, "timed out",
        "no summary: the outcome says what happened"
    );

    let skipped = Outcome::of(Some(ScheduleOutcome::Skipped));
    assert_eq!(skipped.icon, Icon::CircleSlash);

    assert_eq!(
        list_note(&[first, second], now),
        format!("2 \u{b7} {next_line}"),
        "the card's note counts them and names the soonest enabled fire"
    );
    assert_eq!(list_note(&[], now), "0");
}

/// A live run takes the status line; the outcome line keeps the last run that finished.
#[test]
fn a_running_schedule_says_for_how_long() {
    let now = Local::now();
    let mut live = schedule("sch-00000001", "Nightly triage");
    live.runs
        .push(run(ScheduleOutcome::Skipped, "previous run was still live"));
    live.runs.push(ScheduleRun {
        job_id: None,
        started_at: (now - chrono::Duration::minutes(2))
            .with_timezone(&Utc)
            .to_rfc3339(),
        ended_at: None,
        outcome: None,
        summary: None,
        cost_usd: None,
        log_path: None,
    });
    let rows = prepare_list(&[live], now);
    assert_eq!(rows[0].status, "running \u{b7} 2 min");
    let (outcome, summary) = rows[0]
        .last
        .clone()
        .unwrap_or_else(|| panic!("a finished run"));
    assert_eq!(outcome.icon, Icon::CircleSlash);
    assert_eq!(summary, "previous run was still live");
}

#[test]
fn a_one_off_cadence_reads_as_its_moment() {
    let now = Local
        .with_ymd_and_hms(2026, 9, 25, 12, 0, 0)
        .earliest()
        .unwrap_or_else(|| panic!("a local time"));
    let at = Local
        .with_ymd_and_hms(2026, 9, 26, 9, 0, 0)
        .earliest()
        .unwrap_or_else(|| panic!("a local time"))
        .with_timezone(&Utc)
        .to_rfc3339();
    assert_eq!(
        cadence_text(&Cadence::Once { at: at.clone() }, now),
        "once \u{b7} Sep 26 09:00"
    );
    assert_eq!(run_time(&at, now).as_deref(), Some("Sep 26 09:00"));
    let earlier = now - chrono::Duration::minutes(10);
    assert_eq!(
        run_time(&earlier.with_timezone(&Utc).to_rfc3339(), now).as_deref(),
        Some("11:50 today")
    );
}

/// Regression (§5.4): *Run now* left the footer for each schedule row, where it is both the
/// hover action and a menu item; the footer keeps *New schedule* and *Delete*.
#[gpui::test]
fn a_schedule_row_offers_run_now_as_a_hover_action_and_a_menu_item(cx: &mut gpui::TestAppContext) {
    let hover: Vec<&str> = SCHEDULE_VERBS
        .iter()
        .filter(|verb| verb.harness.is_some())
        .map(|verb| verb.label)
        .collect();
    assert_eq!(hover, ["Run now"]);
    let menu: Vec<&str> = SCHEDULE_VERBS.iter().map(|verb| verb.label).collect();
    assert_eq!(menu, ["Open", "Run now", "Delete"]);
    assert_eq!(
        SCHEDULE_VERBS[1].harness,
        Some("board_settings.run"),
        "the cursor row's hover action is the harness's `board_settings.run`"
    );

    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let seeded = draft(&state, cx);
    let footer: Vec<&str> = crate::dialogs::board_settings::view::footer_verbs(&seeded)
        .iter()
        .map(|verb| verb.label)
        .collect();
    assert_eq!(footer, ["New schedule", "Delete"]);
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

/// The form groups its rows card by card: name and switch, cadence, agent, prompt (§5.4).
#[test]
fn the_schedule_form_draws_its_rows_in_card_order() {
    let fields = ScheduleFields::new(true, Local::now());
    let form = ScheduleForm {
        id: None,
        baseline: fields.clone(),
        fields,
        runs: Vec::new(),
        kept: 0,
    };
    let rows = prepare_form(&form, &Catalogue::default());
    let order: Vec<(&str, ScheduleCard)> = rows
        .iter()
        .map(|row| (row.field.label(), row.field.card()))
        .collect();
    assert_eq!(
        order,
        [
            ("Name", ScheduleCard::Identity),
            ("Enabled", ScheduleCard::Identity),
            ("Repeat", ScheduleCard::Cadence),
            ("Every", ScheduleCard::Cadence),
            ("Timeout", ScheduleCard::Cadence),
            ("Provider", ScheduleCard::Agent),
            ("Model", ScheduleCard::Agent),
            ("Effort", ScheduleCard::Agent),
            ("Mode", ScheduleCard::Agent),
            ("Prompt", ScheduleCard::Prompt),
        ]
    );
    assert!(
        matches!(
            rows.iter()
                .find(|row| row.field == ScheduleField::Effort)
                .map(|row| &row.value),
            Some(ScheduleValue::Choice { at: Some(0), .. })
        ),
        "effort is segmented, `default` first"
    );
}

/// The amber strip asks about an unsaved schedule in the form's own words.
#[gpui::test]
fn esc_on_a_dirty_schedule_asks_in_amber(cx: &mut gpui::TestAppContext) {
    let state = open(cx, app(BoardKind::Reviews, true, Vec::new()));
    cx.update(|cx| {
        assert!(new_schedule(&state, cx));
        with_host(&state, cx, |host| {
            let draft = &mut host.board_settings;
            draft.set_schedule_text(ScheduleField::Name, "Chat");
            assert_eq!(draft.footer_strip(), None, "dirty, not yet asked");
            assert_eq!(draft.escape(), EscapeStep::Ask);
            assert_eq!(
                draft.footer_strip(),
                Some(FooterStrip::Warning(
                    "Unsaved schedule. Press Esc again to discard it.".to_owned()
                ))
            );
            assert_eq!(draft.error, None);
        });
        open_on_section(&state, BoardSection::General, cx);
    });
}

#[gpui::test]
fn space_sends_the_toggle(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    cx.update(|cx| {
        assert!(toggle_listed(&state, &bridge, cx));
    });
    let sent = requests.take();
    assert_eq!(sent.len(), 1);
    match &sent[0] {
        RequestBody::UpdateSchedule { id, patch } => {
            assert_eq!(id.as_str(), "sch-00000001");
            assert_eq!(
                *patch,
                SchedulePatch {
                    enabled: Some(false),
                    ..SchedulePatch::default()
                }
            );
        }
        other => panic!("expected UpdateSchedule, got {other:?}"),
    }
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn r_sends_run_now(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    cx.update(|cx| run_focused_schedule(&state, &bridge, cx));
    let sent = requests.take();
    assert!(
        matches!(&sent[..], [RequestBody::RunScheduleNow { id }] if id.as_str() == "sch-00000001"),
        "{sent:?}"
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn d_asks_once_then_deletes(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    cx.update(|cx| assert!(delete_schedule(&state, &bridge, cx)));
    assert!(requests.take().is_empty(), "the first `d` only asks");
    assert!(draft(&state, cx).notice.is_some());
    cx.update(|cx| assert!(delete_schedule(&state, &bridge, cx)));
    let sent = requests.take();
    assert!(
        matches!(&sent[..], [RequestBody::DeleteSchedule { id }] if id.as_str() == "sch-00000001"),
        "{sent:?}"
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn the_open_list_follows_the_mirror_without_a_render(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let state = cx.new(|_| {
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        )
    });
    let (bridge, requests) = Bridge::recording();
    // The dialog's host observes `AppState` from here on; nothing below draws a frame.
    let _dialog = cx.new(|cx| {
        let focus = cx.focus_handle();
        crate::dialogs::ActiveDialog::new(state.clone(), bridge, focus, cx)
    });
    cx.run_until_parked();
    assert_eq!(draft(&state, cx).schedules.list.len(), 1);

    state.update(cx, |app, cx| {
        app.schedules.apply(
            &board_id(),
            vec![
                schedule("sch-00000001", "GitHub reviews"),
                schedule("sch-00000002", "Chat reviews"),
            ],
        );
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(draft(&state, cx).schedules.list.len(), 2);
    drop(requests.take());
}

#[gpui::test]
fn moving_off_an_armed_delete_disarms_it(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![
                schedule("sch-00000001", "GitHub reviews"),
                schedule("sch-00000002", "Chat reviews"),
            ],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    cx.update(|cx| assert!(delete_schedule(&state, &bridge, cx)));
    assert!(draft(&state, cx).schedules.pending_delete.is_some());
    // `j` then `k` lands back on the armed row with the question gone.
    cx.update(|cx| {
        move_row(&state, 1, cx);
        move_row(&state, -1, cx);
    });
    let moved = draft(&state, cx);
    assert_eq!(moved.schedules.pending_delete, None);
    assert_eq!(moved.notice, None);
    // So `d` asks again rather than deleting unseen.
    cx.update(|cx| assert!(delete_schedule(&state, &bridge, cx)));
    assert!(requests.take().is_empty(), "`d` after a move only asks");
    assert!(draft(&state, cx).notice.is_some());
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn another_request_disarms_a_delete(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    for other in ["space", "r"] {
        cx.update(|cx| assert!(delete_schedule(&state, &bridge, cx)));
        assert!(requests.take().is_empty(), "the first `d` only asks");
        cx.update(|cx| {
            if other == "space" {
                assert!(toggle_listed(&state, &bridge, cx));
            } else {
                run_focused_schedule(&state, &bridge, cx);
            }
        });
        let sent = requests.take();
        assert_eq!(sent.len(), 1, "`{other}` sends its own request: {sent:?}");
        let after = draft(&state, cx);
        assert_eq!(after.schedules.pending_delete, None, "`{other}` disarms");
        // Its answer would rewrite the notice; release the request so `d` is heard again.
        cx.update(|cx| {
            with_host(&state, cx, |host| {
                host.board_settings.schedules.busy = false
            });
        });
        cx.update(|cx| assert!(delete_schedule(&state, &bridge, cx)));
        assert!(
            requests.take().is_empty(),
            "`d, {other}, d` asks again rather than deleting unseen"
        );
        cx.update(|cx| {
            with_host(&state, cx, |host| {
                host.board_settings.schedules.pending_delete = None;
                host.board_settings.notice = None;
            });
        });
    }
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn tab_asks_before_leaving_an_unsaved_schedule(cx: &mut gpui::TestAppContext) {
    let state = open(cx, app(BoardKind::Reviews, true, Vec::new()));
    cx.update(|cx| {
        assert!(new_schedule(&state, cx));
        with_host(&state, cx, |host| {
            host.board_settings
                .set_schedule_text(ScheduleField::Name, "Nightly triage");
        });
        assert!(!cycle_section(&state, 1, cx), "the first `⇥` only asks");
    });
    let asked = draft(&state, cx);
    assert_eq!(asked.section, BoardSection::Schedules);
    assert_eq!(
        asked
            .schedules
            .form
            .as_ref()
            .map(|form| form.fields.name.as_str()),
        Some("Nightly triage"),
        "the unsaved form is still there"
    );
    assert_eq!(
        asked.footer_strip(),
        Some(FooterStrip::Warning(UNSAVED_SCHEDULE.to_owned())),
        "the question is the amber strip, not a red error"
    );
    cx.update(|cx| {
        assert!(
            cycle_section(&state, -1, cx),
            "asked once; the next one leaves"
        )
    });
    let left = draft(&state, cx);
    assert_ne!(left.section, BoardSection::Schedules);
    assert!(left.schedules.form.is_none());
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn an_answer_after_leaving_the_section_leaves_the_shown_one_alone(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![
                schedule("sch-00000001", "GitHub reviews"),
                schedule("sch-00000002", "Chat reviews"),
            ],
        ),
    );
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.board_settings.section = BoardSection::General;
            host.board_settings.row = 0;
            host.board_settings.notice = None;
        });
        let generation = read_host(&state, cx, |host, _| host.board_settings.generation);
        finish(
            &state,
            &board_id(),
            generation,
            &Sent::Ran,
            Ok(Some(schedule("sch-00000002", "Chat reviews"))),
            cx,
        );
    });
    let after = draft(&state, cx);
    assert_eq!(after.section, BoardSection::General);
    assert_eq!(after.row, 0);
    assert_eq!(
        after.notice, None,
        "a Schedules sentence never lands on General"
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn n_on_a_reviews_board_prefills_the_starter(cx: &mut gpui::TestAppContext) {
    let state = open(cx, app(BoardKind::Reviews, true, Vec::new()));
    cx.update(|cx| assert!(new_schedule(&state, cx)));
    let form = draft(&state, cx)
        .schedules
        .form
        .unwrap_or_else(|| panic!("a form"));
    assert_eq!(form.id, None);
    assert_eq!(form.fields.name, "GitHub reviews");
    assert_eq!(form.fields.every, "15");
    assert_eq!(form.fields.cadence, CadenceKind::Every);
    assert_eq!(form.fields.prompt, STARTER_PROMPT_GITHUB_REVIEWS);
    assert_eq!(form.fields.mode, PermissionMode::FullAccess);

    let state = open(cx, app(BoardKind::Tasks, true, Vec::new()));
    cx.update(|cx| assert!(new_schedule(&state, cx)));
    let form = draft(&state, cx)
        .schedules
        .form
        .unwrap_or_else(|| panic!("a form"));
    assert!(form.fields.prompt.is_empty(), "other boards start empty");
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn the_empty_state_s_enter_opens_the_starter(cx: &mut gpui::TestAppContext) {
    let app = app(BoardKind::Reviews, true, Vec::new());
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let state = cx.new(|_| app);
    cx.update(|cx| {
        // The shell asks before the host has seeded; the seed takes the request.
        open_schedules_section(&state, true, cx);
        seed(&state, cx);
        read_host(&state, cx, |host, _| {
            let draft = &host.board_settings;
            assert_eq!(draft.section, BoardSection::Schedules);
            let form = draft
                .schedules
                .form
                .as_ref()
                .unwrap_or_else(|| panic!("form"));
            assert_eq!(form.fields.name, "GitHub reviews");
        });
        open_on_section(&state, BoardSection::General, cx);
    });
}

#[gpui::test]
fn ctrl_s_sends_a_create_with_every_field(cx: &mut gpui::TestAppContext) {
    let state = open(cx, app(BoardKind::Reviews, true, Vec::new()));
    let (bridge, requests) = Bridge::recording();
    cx.update(|cx| {
        assert!(new_schedule(&state, cx));
        with_host(&state, cx, |host| {
            let form = host
                .board_settings
                .schedules
                .form
                .as_mut()
                .unwrap_or_else(|| panic!("form"));
            form.fields.provider = AgentKind::Codex;
            form.fields.model = "gpt-5.6-sol".to_owned();
            form.fields.effort = "high".to_owned();
            form.fields.mode = PermissionMode::Plan;
            form.fields.every = "30".to_owned();
            form.fields.timeout = "10".to_owned();
            form.fields.enabled = false;
        });
        assert!(save_schedule(&state, &bridge, cx));
    });
    let sent = requests.take();
    assert_eq!(sent.len(), 1);
    match &sent[0] {
        RequestBody::CreateSchedule { draft } => assert_eq!(
            *draft,
            ScheduleDraft {
                board_id: board_id(),
                name: "GitHub reviews".to_owned(),
                prompt: STARTER_PROMPT_GITHUB_REVIEWS.to_owned(),
                cadence: Cadence::Every { minutes: 30 },
                agent: Some(ScheduleAgent {
                    provider: AgentKind::Codex,
                    model: Some("gpt-5.6-sol".to_owned()),
                    effort: Some("high".to_owned()),
                    mode: PermissionMode::Plan,
                }),
                enabled: Some(false),
                timeout_minutes: Some(10),
            }
        ),
        other => panic!("expected CreateSchedule, got {other:?}"),
    }
    assert!(
        draft(&state, cx).schedules.busy,
        "the form waits for the answer"
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn an_edit_sends_only_what_changed(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let focus = cx.focus_handle();
        assert!(confirm_schedule(&state, window, &focus, cx));
        with_host(&state, cx, |host| {
            let draft = &mut host.board_settings;
            draft.row = draft
                .rows()
                .iter()
                .position(|row| *row == SettingRow::ScheduleField(ScheduleField::Cadence))
                .unwrap_or_else(|| panic!("a cadence row"));
            assert!(draft.cycle_schedule(ScheduleField::Cadence, 1));
            assert!(
                draft
                    .rows()
                    .contains(&SettingRow::ScheduleField(ScheduleField::OnceAt)),
                "`once` swaps the value row"
            );
            let form = draft
                .schedules
                .form
                .as_mut()
                .unwrap_or_else(|| panic!("form"));
            form.fields.once_at = "2026-09-23T09:00:00Z".to_owned();
        });
        assert!(save_schedule(&state, &bridge, cx));
    });
    let sent = requests.take();
    match &sent[..] {
        [RequestBody::UpdateSchedule { id, patch }] => {
            assert_eq!(id.as_str(), "sch-00000001");
            assert_eq!(
                *patch,
                SchedulePatch {
                    cadence: Some(Cadence::Once {
                        at: "2026-09-23T09:00:00Z".to_owned()
                    }),
                    ..SchedulePatch::default()
                }
            );
        }
        other => panic!("expected one UpdateSchedule, got {other:?}"),
    }
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn a_daemon_refusal_shows_in_the_footer_and_keeps_the_form(cx: &mut gpui::TestAppContext) {
    let state = open(cx, app(BoardKind::Reviews, true, Vec::new()));
    let (bridge, _requests) = Bridge::recording();
    let refusal = "invalid cadence: every must be between 5 and 1440 minutes";
    cx.update(|cx| {
        assert!(new_schedule(&state, cx));
        assert!(save_schedule(&state, &bridge, cx));
        let generation = read_host(&state, cx, |host, _| host.board_settings.generation);
        finish(
            &state,
            &board_id(),
            generation,
            &Sent::Saved,
            Err(refusal.to_owned()),
            cx,
        );
    });
    let seeded = draft(&state, cx);
    assert_eq!(seeded.error.as_deref(), Some(refusal));
    assert!(seeded.schedules.form.is_some(), "the form stays open");
    assert!(!seeded.schedules.busy);
    assert_eq!(
        cx.update(|cx| state.read(cx).overlay.clone()),
        Some(Overlay::Dialog(Dialogs::BoardSettings))
    );

    // An accepted save closes the form and lists the schedule the daemon answered with.
    cx.update(|cx| {
        assert!(save_schedule(&state, &bridge, cx));
        let generation = read_host(&state, cx, |host, _| host.board_settings.generation);
        finish(
            &state,
            &board_id(),
            generation,
            &Sent::Saved,
            Ok(Some(schedule("sch-00000009", "GitHub reviews"))),
            cx,
        );
    });
    let saved = draft(&state, cx);
    assert!(saved.schedules.form.is_none());
    assert_eq!(saved.error, None);
    assert_eq!(saved.schedules.list.len(), 1);
    assert_eq!(saved.schedules.list[0].schedule.id.as_str(), "sch-00000009");
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn schedules_changed_refreshes_the_list(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    cx.update(|cx| {
        state.update(cx, |app, _| {
            assert!(app.schedules.mark_stale(&board_id()));
        });
        refresh_stale_schedules(&state, &bridge, cx);
    });
    let sent = requests.take();
    assert!(
        matches!(&sent[..], [RequestBody::ListSchedules { board_id: Some(board) }] if *board == board_id()),
        "{sent:?}"
    );
    cx.update(|cx| {
        adopt_load(
            &state,
            &bridge,
            &board_id(),
            Ok(vec![
                schedule("sch-00000001", "GitHub reviews"),
                schedule("sch-00000002", "Chat"),
            ]),
            cx,
        );
    });
    let names: Vec<String> = draft(&state, cx)
        .schedules
        .list
        .iter()
        .map(|row| row.schedule.name.clone())
        .collect();
    assert_eq!(names, ["GitHub reviews", "Chat"]);
    assert!(
        requests.take().is_empty(),
        "an answer with no change behind it asks nothing more"
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn a_change_during_a_load_is_read_again_when_it_answers(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let (bridge, requests) = Bridge::recording();
    cx.update(|cx| {
        state.update(cx, |app, _| {
            assert!(app.schedules.mark_stale(&board_id()));
        });
        refresh_stale_schedules(&state, &bridge, cx);
        // The second event of the batch lands while the first reload is in flight: the event
        // loop's refresh skips a board already loading.
        state.update(cx, |app, _| {
            assert!(app.schedules.mark_stale(&board_id()));
        });
        refresh_stale_schedules(&state, &bridge, cx);
    });
    assert_eq!(requests.take().len(), 1, "one load in flight at a time");
    cx.update(|cx| {
        adopt_load(
            &state,
            &bridge,
            &board_id(),
            Ok(vec![schedule("sch-00000001", "GitHub reviews")]),
            cx,
        );
    });
    let sent = requests.take();
    assert!(
        matches!(&sent[..], [RequestBody::ListSchedules { board_id: Some(board) }] if *board == board_id()),
        "the answer may predate the second change, so it is read again: {sent:?}"
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn esc_leaves_the_form_asking_once_about_unsaved_edits(cx: &mut gpui::TestAppContext) {
    let state = open(cx, app(BoardKind::Reviews, true, Vec::new()));
    cx.update(|cx| {
        assert!(new_schedule(&state, cx));
        with_host(&state, cx, |host| {
            let draft = &mut host.board_settings;
            draft.set_schedule_text(ScheduleField::Name, "Chat");
            assert_eq!(draft.escape(), EscapeStep::Ask);
            assert_eq!(draft.escape(), EscapeStep::Schedule);
            assert!(draft.schedules.form.is_none());
            assert_eq!(
                draft.focused(),
                SettingRow::NoRow,
                "an empty list has no row"
            );
            assert_eq!(draft.focused_text(), None, "and nothing to type into");
        });
        open_on_section(&state, BoardSection::General, cx);
    });
}

#[test]
fn a_typed_local_time_goes_out_as_rfc_3339() {
    let local = Local
        .with_ymd_and_hms(2026, 9, 23, 9, 0, 0)
        .earliest()
        .unwrap_or_else(|| panic!("a local time"));
    assert_eq!(
        once_at_wire("2026-09-23 09:00"),
        local
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    );
    assert_eq!(once_at_wire("2026-09-23T09:00:00Z"), "2026-09-23T09:00:00Z");
    assert_eq!(
        once_at_wire("tomorrow"),
        "tomorrow",
        "the daemon's refusal names it"
    );
}

#[test]
fn switching_provider_keeps_a_mode_it_accepts() {
    let mut fields = ScheduleFields::new(false, Local::now());
    fields.mode = PermissionMode::Auto;
    assert!(fields.cycle(ScheduleField::Provider, 1, &Catalogue::default()));
    assert_eq!(fields.provider, AgentKind::Codex);
    assert_eq!(fields.mode, PermissionMode::FullAccess);
}

#[test]
fn switching_provider_clears_the_old_providers_model_and_effort() {
    let mut fields = ScheduleFields::new(false, Local::now());
    fields.model = "claude-opus".to_owned();
    fields.effort = "high".to_owned();
    assert!(fields.cycle(ScheduleField::Provider, 1, &Catalogue::default()));
    assert_eq!(fields.provider, AgentKind::Codex);
    assert_eq!(fields.agent().model, None);
    assert_eq!(fields.agent().effort, None);

    // A cycle that lands on the same provider changes nothing.
    fields.model = "gpt-5".to_owned();
    assert!(!fields.cycle(ScheduleField::Provider, 1, &Catalogue::default()));
    assert_eq!(fields.model, "gpt-5");
}

fn run(outcome: ScheduleOutcome, summary: &str) -> ScheduleRun {
    ScheduleRun {
        job_id: None,
        started_at: "2026-09-23T09:00:00Z".to_owned(),
        ended_at: Some("2026-09-23T09:00:00Z".to_owned()),
        outcome: Some(outcome),
        summary: Some(summary.to_owned()),
        cost_usd: None,
        log_path: None,
    }
}

#[gpui::test]
fn a_run_now_the_daemon_skipped_says_skipped(cx: &mut gpui::TestAppContext) {
    let state = open(
        cx,
        app(
            BoardKind::Reviews,
            true,
            vec![schedule("sch-00000001", "GitHub reviews")],
        ),
    );
    let mut skipped = schedule("sch-00000001", "GitHub reviews");
    skipped.runs.push(run(
        ScheduleOutcome::Skipped,
        "the previous run is still going",
    ));
    cx.update(|cx| {
        let generation = read_host(&state, cx, |host, _| host.board_settings.generation);
        finish(
            &state,
            &board_id(),
            generation,
            &Sent::Ran,
            Ok(Some(skipped)),
            cx,
        );
    });
    assert_eq!(
        draft(&state, cx).notice.as_deref(),
        Some("skipped GitHub reviews: the previous run is still going")
    );
    cx.update(|cx| {
        let generation = read_host(&state, cx, |host, _| host.board_settings.generation);
        finish(
            &state,
            &board_id(),
            generation,
            &Sent::Ran,
            Ok(Some(schedule("sch-00000001", "GitHub reviews"))),
            cx,
        );
    });
    assert_eq!(
        draft(&state, cx).notice.as_deref(),
        Some("running GitHub reviews now")
    );
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[gpui::test]
fn an_open_forms_last_runs_follow_the_mirror(cx: &mut gpui::TestAppContext) {
    let original = schedule("sch-00000001", "GitHub reviews");
    let state = open(cx, app(BoardKind::Reviews, true, vec![original.clone()]));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.board_settings.open_schedule(&original, Local::now());
            host.board_settings
                .set_schedule_text(ScheduleField::Name, "Edited");
        });
    });
    assert!(
        draft(&state, cx)
            .schedules
            .form
            .is_some_and(|form| form.runs.is_empty())
    );

    let mut ran = original.clone();
    ran.runs
        .push(run(ScheduleOutcome::Succeeded, "filed 2 reviews"));
    cx.update(|cx| {
        state.update(cx, |app, _| app.schedules.apply(&board_id(), vec![ran]));
        sync_open_list(&state, cx);
    });
    let form = draft(&state, cx)
        .schedules
        .form
        .unwrap_or_else(|| panic!("the form stays open"));
    assert_eq!(form.runs.len(), 1);
    assert_eq!(form.runs[0].summary, "filed 2 reviews", "{:?}", form.runs);
    assert_eq!(form.kept, 1, "the card's note counts what fleetd keeps");
    assert_eq!(form.fields.name, "Edited", "what was typed survives");
    assert_eq!(form.baseline.name, "GitHub reviews");
    cx.update(|cx| open_on_section(&state, BoardSection::General, cx));
}

#[test]
fn a_list_rows_summary_is_capped() {
    let mut one = schedule("sch-00000001", "GitHub reviews");
    one.runs
        .push(run(ScheduleOutcome::Succeeded, &"y".repeat(280)));
    let rows = prepare_list(&[one], Local::now());
    let (_, summary) = rows[0]
        .last
        .clone()
        .unwrap_or_else(|| panic!("the schedule has run"));
    assert_eq!(
        summary.chars().count(),
        crate::views::board_screen::SCHEDULE_SUMMARY_BUDGET
    );
    assert!(summary.ends_with(fleet_ui_kit::ELLIPSIS));
}
