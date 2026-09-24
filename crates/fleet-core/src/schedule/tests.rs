//! Schedule model, validation and fire-time rule tests.

use super::*;
use chrono::TimeZone;

const NOW: &str = "2026-09-24T12:00:00Z";

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn draft() -> ScheduleDraft {
    ScheduleDraft {
        board_id: BoardId::try_from("reviews-work").unwrap(),
        name: "GitHub reviews".into(),
        prompt: "Find reviews for {board}".into(),
        cadence: Cadence::Every { minutes: 15 },
        agent: None,
        enabled: None,
        timeout_minutes: None,
    }
}

fn schedule() -> Schedule {
    apply_draft(draft(), ScheduleId::try_from("sch-0123abcd").unwrap(), NOW)
}

fn run(started_at: &str, log_path: Option<&str>) -> ScheduleRun {
    ScheduleRun {
        job_id: None,
        started_at: started_at.into(),
        ended_at: None,
        outcome: Some(ScheduleOutcome::Succeeded),
        summary: None,
        cost_usd: None,
        log_path: log_path.map(Into::into),
    }
}

fn refusal(schedule: &Schedule) -> (String, String) {
    match validate_schedule(schedule) {
        Err(ScheduleError::Invalid { field, reason }) => (field, reason),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn assert_refused(schedule: &Schedule, field: &str, reason: &str) {
    assert_eq!(refusal(schedule), (field.to_owned(), reason.to_owned()));
}

#[test]
fn draft_defaults_fill_enabled_timeout_and_agent() {
    let schedule = schedule();
    assert!(schedule.enabled);
    assert_eq!(schedule.timeout_minutes, SCHEDULE_DEFAULT_TIMEOUT_MINUTES);
    assert_eq!(schedule.agent, ScheduleAgent::default());
    assert_eq!(schedule.agent.provider, AgentKind::Claude);
    assert_eq!(schedule.agent.model, None);
    assert_eq!(schedule.agent.effort, None);
    assert_eq!(schedule.agent.mode, PermissionMode::FullAccess);
    assert_eq!(schedule.created_at, NOW);
    assert_eq!(schedule.updated_at, NOW);
    assert!(schedule.runs.is_empty());
    assert_eq!(schedule.next_run_at, None);
    assert_eq!(validate_schedule(&schedule), Ok(()));
}

#[test]
fn draft_values_override_the_defaults() {
    let agent = ScheduleAgent {
        provider: AgentKind::Codex,
        model: Some("gpt-5".into()),
        effort: Some("high".into()),
        mode: PermissionMode::Ask,
    };
    let schedule = apply_draft(
        ScheduleDraft {
            agent: Some(agent.clone()),
            enabled: Some(false),
            timeout_minutes: Some(45),
            ..draft()
        },
        ScheduleId::try_from("sch-0123abcd").unwrap(),
        NOW,
    );
    assert_eq!(schedule.agent, agent);
    assert!(!schedule.enabled);
    assert_eq!(schedule.timeout_minutes, 45);
}

#[test]
fn patch_changes_only_given_fields_and_stamps_updated_at() {
    let mut schedule = schedule();
    apply_patch(
        &mut schedule,
        SchedulePatch {
            name: Some("Renamed".into()),
            enabled: Some(false),
            ..SchedulePatch::default()
        },
        "2026-09-24T13:00:00Z",
    );
    assert_eq!(schedule.name, "Renamed");
    assert!(!schedule.enabled);
    assert_eq!(schedule.prompt, "Find reviews for {board}");
    assert_eq!(schedule.cadence, Cadence::Every { minutes: 15 });
    assert_eq!(schedule.created_at, NOW);
    assert_eq!(schedule.updated_at, "2026-09-24T13:00:00Z");
}

#[test]
fn once_times_are_normalised_to_utc_seconds_on_create_and_edit() {
    let created = apply_draft(
        ScheduleDraft {
            cadence: Cadence::Once {
                at: "2026-09-25T09:00:00.789+02:00".into(),
            },
            ..draft()
        },
        ScheduleId::try_from("sch-0123abcd").unwrap(),
        NOW,
    );
    assert_eq!(
        created.cadence,
        Cadence::Once {
            at: "2026-09-25T07:00:00Z".into()
        }
    );

    let mut edited = schedule();
    apply_patch(
        &mut edited,
        SchedulePatch {
            cadence: Some(Cadence::Once {
                at: "2026-09-26T10:30:00-03:00".into(),
            }),
            ..SchedulePatch::default()
        },
        NOW,
    );
    assert_eq!(
        edited.cadence,
        Cadence::Once {
            at: "2026-09-26T13:30:00Z".into()
        }
    );
}

#[test]
fn refuses_an_empty_name() {
    let mut schedule = schedule();
    schedule.name = "  ".into();
    assert_refused(&schedule, "name", "must not be empty");
}

#[test]
fn refuses_a_name_over_80_characters() {
    let mut schedule = schedule();
    schedule.name = "é".repeat(SCHEDULE_NAME_MAX_CHARS);
    assert_eq!(validate_schedule(&schedule), Ok(()));
    schedule.name.push('x');
    assert_refused(&schedule, "name", "must be at most 80 characters");
}

#[test]
fn refuses_an_empty_prompt() {
    let mut schedule = schedule();
    schedule.prompt = "\n".into();
    assert_refused(&schedule, "prompt", "must not be empty");
}

#[test]
fn refuses_nul_bytes_in_names_and_prompts() {
    let mut schedule = schedule();
    schedule.name = "reviews\0secret".into();
    assert_refused(&schedule, "name", "must not contain a NUL byte");

    schedule.name = "reviews".into();
    schedule.prompt = "find\0reviews".into();
    assert_refused(&schedule, "prompt", "must not contain a NUL byte");
}

#[test]
fn refuses_a_prompt_over_16_kib() {
    let mut schedule = schedule();
    schedule.prompt = "a".repeat(SCHEDULE_PROMPT_MAX_BYTES);
    assert_eq!(validate_schedule(&schedule), Ok(()));
    schedule.prompt.push('a');
    assert_refused(&schedule, "prompt", "must be at most 16 KiB");
}

#[test]
fn refuses_an_every_cadence_out_of_range() {
    let mut schedule = schedule();
    for minutes in [SCHEDULE_MIN_EVERY_MINUTES, SCHEDULE_MAX_EVERY_MINUTES] {
        schedule.cadence = Cadence::Every { minutes };
        assert_eq!(validate_schedule(&schedule), Ok(()));
    }
    for minutes in [0, 4, 1441] {
        schedule.cadence = Cadence::Every { minutes };
        assert_refused(
            &schedule,
            "cadence",
            "every must be between 5 and 1440 minutes",
        );
    }
}

#[test]
fn refuses_a_once_cadence_without_an_rfc3339_time() {
    let mut schedule = schedule();
    schedule.cadence = Cadence::Once {
        at: "tomorrow at nine".into(),
    };
    assert_refused(&schedule, "cadence", "once needs an RFC 3339 time");
    schedule.cadence = Cadence::Once {
        at: "2026-09-25T09:00:00+02:00".into(),
    };
    assert_eq!(validate_schedule(&schedule), Ok(()));
}

#[test]
fn refuses_a_timeout_out_of_range() {
    let mut schedule = schedule();
    for timeout_minutes in [0, SCHEDULE_MAX_TIMEOUT_MINUTES + 1] {
        schedule.timeout_minutes = timeout_minutes;
        assert_refused(&schedule, "timeout_minutes", "must be between 1 and 120");
    }
    for timeout_minutes in [1, SCHEDULE_MAX_TIMEOUT_MINUTES] {
        schedule.timeout_minutes = timeout_minutes;
        assert_eq!(validate_schedule(&schedule), Ok(()));
    }
}

#[test]
fn refuses_a_mode_the_provider_does_not_support() {
    let mut schedule = schedule();
    schedule.agent.provider = AgentKind::Codex;
    schedule.agent.mode = PermissionMode::Auto;
    assert_refused(&schedule, "mode", "auto is not supported by codex");
    schedule.agent.mode = PermissionMode::DontAsk;
    assert_refused(&schedule, "mode", "dont-ask is not supported by codex");
    schedule.agent.provider = AgentKind::Claude;
    assert_eq!(validate_schedule(&schedule), Ok(()));
}

#[test]
fn refuses_blank_model_and_effort() {
    let mut schedule = schedule();
    schedule.agent.model = Some(" \t".into());
    assert_refused(&schedule, "model", "must not be blank");

    schedule.agent.model = None;
    schedule.agent.effort = Some("\n".into());
    assert_refused(&schedule, "effort", "must not be blank");
}

#[test]
fn refusal_displays_field_and_reason() {
    let mut schedule = schedule();
    schedule.name = String::new();
    assert_eq!(
        validate_schedule(&schedule).unwrap_err().to_string(),
        "invalid name: must not be empty"
    );
}

#[test]
fn never_run_every_schedule_is_due_now() {
    let now = at(NOW);
    assert_eq!(next_run_at(&schedule(), now), Some(now));
}

#[test]
fn every_schedule_is_due_interval_after_last_start() {
    let mut schedule = schedule();
    schedule.runs.push(run("2026-09-24T11:50:00Z", None));
    assert_eq!(
        next_run_at(&schedule, at(NOW)),
        Some(at("2026-09-24T12:05:00Z"))
    );
}

#[test]
fn downtime_catches_up_with_one_run_due_now() {
    let mut schedule = schedule();
    schedule.runs.push(run("2026-09-24T11:00:00Z", None));
    let now = at(NOW);
    assert_eq!(next_run_at(&schedule, now), Some(now));
    // Once that catch-up run starts, the next one is a full interval later, not another now.
    schedule.runs.push(run(NOW, None));
    assert_eq!(
        next_run_at(&schedule, now),
        Some(at("2026-09-24T12:15:00Z"))
    );
}

#[test]
fn unparsable_last_start_counts_as_never_ran() {
    let mut schedule = schedule();
    schedule.runs.push(run("garbage", None));
    let now = at(NOW);
    assert_eq!(next_run_at(&schedule, now), Some(now));
}

#[test]
fn once_in_the_future_is_due_at_its_time() {
    let mut schedule = schedule();
    schedule.cadence = Cadence::Once {
        at: "2026-09-25T09:00:00+02:00".into(),
    };
    assert_eq!(
        next_run_at(&schedule, at(NOW)),
        Some(Utc.with_ymd_and_hms(2026, 9, 25, 7, 0, 0).unwrap())
    );
}

#[test]
fn run_now_before_a_once_time_does_not_consume_the_scheduled_fire() {
    let mut schedule = schedule();
    schedule.cadence = Cadence::Once {
        at: "2026-09-25T09:00:00Z".into(),
    };
    schedule.runs.push(run("2026-09-24T12:00:00Z", None));
    assert_eq!(
        next_run_at(&schedule, at(NOW)),
        Some(at("2026-09-25T09:00:00Z"))
    );
}

#[test]
fn editing_a_fired_once_schedule_to_a_future_time_makes_it_due_again() {
    let mut schedule = schedule();
    schedule.cadence = Cadence::Once {
        at: "2026-09-24T11:00:00Z".into(),
    };
    schedule.runs.push(run("2026-09-24T11:00:00Z", None));
    schedule.cadence = Cadence::Once {
        at: "2026-09-25T09:00:00Z".into(),
    };
    assert_eq!(
        next_run_at(&schedule, at(NOW)),
        Some(at("2026-09-25T09:00:00Z"))
    );
}

#[test]
fn a_new_once_schedule_in_the_past_is_due_now() {
    let mut schedule = schedule();
    schedule.cadence = Cadence::Once {
        at: "2026-09-24T11:00:00Z".into(),
    };
    assert_eq!(next_run_at(&schedule, at(NOW)), Some(at(NOW)));
}

#[test]
fn a_future_every_start_after_a_clock_step_schedules_from_now() {
    let mut schedule = schedule();
    schedule.runs.push(run("2026-09-24T13:00:00Z", None));
    assert_eq!(
        next_run_at(&schedule, at(NOW)),
        Some(at("2026-09-24T12:15:00Z"))
    );
}

#[test]
fn once_already_run_never_fires_again() {
    let mut schedule = schedule();
    schedule.cadence = Cadence::Once {
        at: "2026-09-24T11:00:00Z".into(),
    };
    schedule.runs.push(run("2026-09-24T11:00:00Z", None));
    assert_eq!(next_run_at(&schedule, at(NOW)), None);
}

#[test]
fn a_skipped_run_at_the_once_time_consumes_the_fire() {
    let mut schedule = schedule();
    schedule.cadence = Cadence::Once {
        at: "2026-09-24T11:00:00Z".into(),
    };
    let mut skipped = run("2026-09-24T11:00:00Z", None);
    skipped.outcome = Some(ScheduleOutcome::Skipped);
    schedule.runs.push(skipped);
    assert_eq!(next_run_at(&schedule, at(NOW)), None);
}

#[test]
fn disabled_schedule_never_fires() {
    let mut schedule = schedule();
    schedule.enabled = false;
    assert_eq!(next_run_at(&schedule, at(NOW)), None);
    schedule.cadence = Cadence::Once {
        at: "2026-09-25T09:00:00Z".into(),
    };
    assert_eq!(next_run_at(&schedule, at(NOW)), None);
}

#[test]
fn push_run_caps_runs_and_returns_dropped_log_paths() {
    let mut schedule = schedule();
    for index in 0..MAX_RUNS_PER_SCHEDULE {
        let started_at = format!("2026-09-24T10:{index:02}:00Z");
        let log = format!("/logs/{index}.log");
        let log_path = (index != 1).then_some(log.as_str());
        assert!(push_run(&mut schedule, run(&started_at, log_path)).is_empty());
    }
    assert_eq!(schedule.runs.len(), MAX_RUNS_PER_SCHEDULE);

    let dropped = push_run(&mut schedule, run(NOW, Some("/logs/new.log")));
    assert_eq!(dropped, vec!["/logs/0.log".to_owned()]);
    assert_eq!(schedule.runs.len(), MAX_RUNS_PER_SCHEDULE);
    assert_eq!(schedule.runs[0].started_at, "2026-09-24T10:01:00Z");
    assert_eq!(schedule.runs[MAX_RUNS_PER_SCHEDULE - 1].started_at, NOW);

    // A dropped run without a log contributes nothing.
    assert!(push_run(&mut schedule, run(NOW, None)).is_empty());
    assert_eq!(schedule.runs[0].started_at, "2026-09-24T10:02:00Z");
}

#[test]
fn render_prompt_substitutes_every_placeholder_and_ends_with_the_footer() {
    let mut schedule = schedule();
    schedule.prompt = "Board {board}, since {last_run_at}, at {now}, keep {other}".into();
    let fleet = "'/opt/fleet/bin/fleet'";

    let first = render_prompt(&schedule, fleet, NOW);
    assert!(first.starts_with(&format!(
        "Board reviews-work, since never, at {NOW}, keep {{other}}\n\n--- Fleet scheduled task \"GitHub reviews\" for board reviews-work ---\n"
    )));

    schedule.runs.push(run("2026-09-24T11:45:00Z", None));
    let rendered = render_prompt(&schedule, fleet, NOW);
    let footer = SCHEDULE_FOOTER_TEMPLATE
        .replace("{name}", "GitHub reviews")
        .replace("{board}", "reviews-work")
        .replace("{fleet}", fleet)
        .replace("{last_run_at}", "2026-09-24T11:45:00Z");
    assert_eq!(
        rendered,
        format!(
            "Board reviews-work, since 2026-09-24T11:45:00Z, at {NOW}, keep {{other}}\n\n{footer}"
        )
    );
    assert!(rendered.contains(
        "  '/opt/fleet/bin/fleet' board --board reviews-work card new \"<pull request title>\" --pr <pull request URL>"
    ));
    assert!(rendered.contains("Only consider requests made after 2026-09-24T11:45:00Z when"));
    assert!(rendered.ends_with(
        "End your reply with exactly one line: SUMMARY: <n> created, <n> existing, <n> reopened, <anything the user must know>"
    ));
}

/// A skipped fire started no agent, so the run after it is told the time of the last run that
/// did read its sources.
#[test]
fn render_prompt_skips_a_trailing_skipped_fire_for_last_run_at() {
    let mut schedule = schedule();
    schedule.prompt = "since {last_run_at}".into();
    schedule.runs.push(run("2026-09-24T11:00:00Z", None));
    let mut skipped = run("2026-09-24T11:15:00Z", None);
    skipped.outcome = Some(ScheduleOutcome::Skipped);
    schedule.runs.push(skipped);

    let rendered = render_prompt(&schedule, "fleet", NOW);
    assert!(
        rendered.starts_with("since 2026-09-24T11:00:00Z\n\n"),
        "{rendered}"
    );
    assert!(rendered.contains("Only consider requests made after 2026-09-24T11:00:00Z when"));

    schedule.runs.remove(0);
    assert!(render_prompt(&schedule, "fleet", NOW).starts_with("since never\n\n"));
}

/// Skipped fires during one long run drop older finished runs, never the live one.
#[test]
fn push_run_never_drops_the_live_run() {
    let mut schedule = schedule();
    let mut live = run("2026-09-24T09:00:00Z", Some("/logs/live.log"));
    live.outcome = None;
    schedule.runs.push(live);
    for index in 0..MAX_RUNS_PER_SCHEDULE + 5 {
        let mut skipped = run(&format!("2026-09-24T10:{index:02}:00Z"), None);
        skipped.outcome = Some(ScheduleOutcome::Skipped);
        assert!(push_run(&mut schedule, skipped).is_empty());
    }
    assert_eq!(schedule.runs.len(), MAX_RUNS_PER_SCHEDULE);
    assert_eq!(schedule.runs[0].started_at, "2026-09-24T09:00:00Z");
    assert_eq!(schedule.runs[0].outcome, None);
    assert_eq!(schedule.runs[1].started_at, "2026-09-24T10:06:00Z");
}

#[test]
fn render_prompt_never_expands_a_placeholder_inside_a_value() {
    let mut schedule = schedule();
    schedule.name = "{board}".into();
    schedule.prompt = "{now}".into();
    let rendered = render_prompt(&schedule, "fleet", "{board}");
    assert!(
        rendered.starts_with(
            "{board}\n\n--- Fleet scheduled task \"{board}\" for board reviews-work ---"
        )
    );
}

#[test]
fn summary_line_takes_the_last_summary() {
    let message = "Working.\nSUMMARY: stale\nMore work.\n  SUMMARY:   2 created, 1 existing, 0 reopened, all good  \nBye";
    assert_eq!(
        summary_line(message).as_deref(),
        Some("2 created, 1 existing, 0 reopened, all good")
    );
}

#[test]
fn summary_line_is_none_without_a_summary() {
    assert_eq!(summary_line("Nothing to report.\nsummary: lowercase"), None);
    assert_eq!(summary_line(""), None);
    assert_eq!(summary_line("SUMMARY:   "), None);
}

#[test]
fn summary_line_cuts_a_long_summary_at_a_char_boundary() {
    let long = "é日".repeat(SCHEDULE_SUMMARY_MAX_CHARS);
    let summary = summary_line(&format!("SUMMARY: {long}")).unwrap();
    assert_eq!(summary.chars().count(), SCHEDULE_SUMMARY_MAX_CHARS);
    assert!(long.starts_with(&summary));
}

#[test]
fn documents_and_runs_round_trip() {
    let mut schedule = schedule();
    schedule.runs.push(ScheduleRun {
        cost_usd: Some(0.25),
        summary: Some("1 created".into()),
        ..run(NOW, Some("/logs/a.log"))
    });
    let document = SchedulesDocument {
        schedules: vec![schedule],
        ..SchedulesDocument::default()
    };
    let json = serde_json::to_value(&document).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["schedules"][0]["cadence"]["kind"], "every");
    assert_eq!(json["schedules"][0]["agent"]["mode"], "full_access");
    assert_eq!(json["schedules"][0]["runs"][0]["outcome"], "succeeded");
    assert_eq!(
        serde_json::from_value::<SchedulesDocument>(json).unwrap(),
        document
    );
}
