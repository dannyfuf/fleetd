use super::{output, *};
use crate::args::{Cli, Command};
use clap::Parser;
use fleet_core::ids::JobId;

fn parse(arguments: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("fleet").chain(arguments.iter().copied()))
}

fn schedule_args(arguments: &[&str]) -> ScheduleArgs {
    match parse(arguments) {
        Ok(Cli {
            command: Some(Command::Schedule(arguments)),
            ..
        }) => arguments,
        other => panic!("expected a schedule command, got {other:?}"),
    }
}

fn new_args(arguments: &[&str]) -> ScheduleNewArgs {
    match schedule_args(arguments).command {
        ScheduleCommand::New(arguments) => arguments,
        other => panic!("expected schedule new, got {other:?}"),
    }
}

fn edit_args(arguments: &[&str]) -> ScheduleEditArgs {
    match schedule_args(arguments).command {
        ScheduleCommand::Edit(arguments) => arguments,
        other => panic!("expected schedule edit, got {other:?}"),
    }
}

fn id(value: &str) -> ScheduleId {
    ScheduleId::try_from(value).unwrap_or_else(|error| panic!("{error}"))
}

fn board(value: &str) -> BoardId {
    BoardId::try_from(value).unwrap_or_else(|error| panic!("{error}"))
}

fn run(started_at: &str, outcome: Option<ScheduleOutcome>, summary: Option<&str>) -> ScheduleRun {
    ScheduleRun {
        job_id: Some(JobId::try_from("job-7").unwrap_or_else(|error| panic!("{error}"))),
        started_at: started_at.to_owned(),
        ended_at: None,
        outcome,
        summary: summary.map(str::to_owned),
        cost_usd: Some(0.126),
        log_path: Some("/home/u/.fleet/schedules/sch-0a1b2c3d/logs/20260923T090000Z.log".into()),
    }
}

fn sample(cadence: Cadence, runs: Vec<ScheduleRun>) -> Schedule {
    Schedule {
        id: id("sch-0a1b2c3d"),
        board_id: board("reviews-work"),
        name: "GitHub reviews".to_owned(),
        prompt: STARTER_PROMPT_GITHUB_REVIEWS.to_owned(),
        cadence,
        agent: ScheduleAgent::default(),
        enabled: true,
        timeout_minutes: 20,
        created_at: "2026-09-23T08:00:00Z".to_owned(),
        updated_at: "2026-09-23T08:00:00Z".to_owned(),
        runs,
        next_run_at: Some("2026-09-23T09:15:00Z".to_owned()),
    }
}

#[test]
fn prompt_sources_and_cadences_are_exclusive_and_each_is_required_on_new() {
    let base = ["schedule", "new", "--name", "n"];
    let with = |extra: &[&str]| parse(&[&base[..], extra].concat());
    assert!(with(&["--prompt", "p", "--every", "15"]).is_ok());
    assert!(
        with(&[
            "--starter",
            "github-reviews",
            "--once",
            "2026-09-23T09:00:00Z"
        ])
        .is_ok()
    );
    assert!(
        with(&[
            "--prompt",
            "p",
            "--starter",
            "github-reviews",
            "--every",
            "15"
        ])
        .is_err()
    );
    assert!(with(&["--prompt", "p", "--prompt-file", "f", "--every", "15"]).is_err());
    assert!(
        with(&[
            "--prompt",
            "p",
            "--every",
            "15",
            "--once",
            "2026-09-23T09:00:00Z"
        ])
        .is_err()
    );
    assert!(
        with(&["--every", "15"]).is_err(),
        "a prompt source is required"
    );
    assert!(with(&["--prompt", "p"]).is_err(), "a cadence is required");
    assert!(
        parse(&[
            "schedule",
            "edit",
            "sch-0a1b2c3d",
            "--every",
            "5",
            "--once",
            "x"
        ])
        .is_err()
    );
    assert!(parse(&["schedule", "edit", "sch-0a1b2c3d", "--enable", "--disable"]).is_err());
}

#[test]
fn board_selectors_conflict_even_across_command_levels() {
    assert!(parse(&["schedule", "--reviews", "--worktree", "list"]).is_err());
    let refusal = |selector: Selector| selector.refuse_conflicts().err().map(|error| error.message);
    let reviews_and_board = Selector {
        board: Some(board("work")),
        worktree: None,
        context: None,
        reviews: true,
    };
    assert_eq!(
        refusal(reviews_and_board).as_deref(),
        Some("--board and --reviews cannot be used together")
    );
    let reviews_of_a_context = Selector {
        board: None,
        worktree: None,
        context: Some(ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}"))),
        reviews: true,
    };
    assert_eq!(refusal(reviews_of_a_context), None);
}

#[test]
fn an_edit_that_names_nothing_is_refused_before_any_request() {
    let error = edit_request(edit_args(&["schedule", "edit", "sch-0a1b2c3d"]))
        .err()
        .map(|error| (error.kind, error.message));
    assert_eq!(
        error,
        Some((ErrorKind::Validation, "nothing to change".to_owned()))
    );
}

#[test]
fn an_edit_patches_only_the_fields_it_names_and_merges_agent_flags() {
    let (edited, patch, agent) = edit_request(edit_args(&[
        "schedule",
        "edit",
        "sch-0a1b2c3d",
        "--disable",
        "--every",
        "30",
        "--model",
        "opus",
    ]))
    .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(edited, id("sch-0a1b2c3d"));
    assert_eq!(
        patch,
        SchedulePatch {
            cadence: Some(Cadence::Every { minutes: 30 }),
            enabled: Some(false),
            ..SchedulePatch::default()
        }
    );
    let base = ScheduleAgent {
        provider: AgentKind::Codex,
        model: Some("gpt".to_owned()),
        effort: Some("high".to_owned()),
        mode: PermissionMode::Ask,
    };
    assert_eq!(
        agent.over(base),
        ScheduleAgent {
            provider: AgentKind::Codex,
            model: Some("opus".to_owned()),
            effort: Some("high".to_owned()),
            mode: PermissionMode::Ask,
        }
    );
}

#[test]
fn changing_provider_without_model_or_effort_clears_the_old_providers_values() {
    let (_, _, agent) = edit_request(edit_args(&[
        "schedule",
        "edit",
        "sch-0a1b2c3d",
        "--provider",
        "codex",
    ]))
    .unwrap_or_else(|error| panic!("{error:?}"));
    let base = ScheduleAgent {
        provider: AgentKind::Claude,
        model: Some("opus".to_owned()),
        effort: Some("high".to_owned()),
        mode: PermissionMode::FullAccess,
    };

    assert_eq!(
        agent.over(base),
        ScheduleAgent {
            provider: AgentKind::Codex,
            model: None,
            effort: None,
            mode: PermissionMode::FullAccess,
        }
    );
}

#[test]
fn empty_model_and_effort_flags_clear_the_stored_values() {
    let (_, _, agent) = edit_request(edit_args(&[
        "schedule",
        "edit",
        "sch-0a1b2c3d",
        "--model",
        "",
        "--effort",
        "",
    ]))
    .unwrap_or_else(|error| panic!("{error:?}"));
    let base = ScheduleAgent {
        provider: AgentKind::Claude,
        model: Some("opus".to_owned()),
        effort: Some("high".to_owned()),
        mode: PermissionMode::Ask,
    };

    assert_eq!(
        agent.over(base),
        ScheduleAgent {
            provider: AgentKind::Claude,
            model: None,
            effort: None,
            mode: PermissionMode::Ask,
        }
    );
}

#[test]
fn the_github_reviews_starter_fills_the_draft_prompt() {
    let fields = NewFields::from_args(new_args(&[
        "schedule",
        "--reviews",
        "new",
        "--name",
        "GitHub reviews",
        "--starter",
        "github-reviews",
        "--every",
        "15",
    ]))
    .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(
        fields.draft(board("reviews-work")),
        ScheduleDraft {
            board_id: board("reviews-work"),
            name: "GitHub reviews".to_owned(),
            prompt: STARTER_PROMPT_GITHUB_REVIEWS.to_owned(),
            cadence: Cadence::Every { minutes: 15 },
            agent: None,
            enabled: None,
            timeout_minutes: None,
        }
    );
}

#[test]
fn new_flags_become_the_draft_agent_enabled_and_timeout() {
    let fields = NewFields::from_args(new_args(&[
        "schedule",
        "new",
        "--name",
        "n",
        "--prompt",
        "p",
        "--once",
        "2026-09-23T09:00:00Z",
        "--provider",
        "codex",
        "--mode",
        "plan",
        "--timeout",
        "45",
        "--disabled",
    ]))
    .unwrap_or_else(|error| panic!("{error:?}"));
    let draft = fields.draft(board("work"));
    assert_eq!(
        draft.agent,
        Some(ScheduleAgent {
            provider: AgentKind::Codex,
            model: None,
            effort: None,
            mode: PermissionMode::Plan,
        })
    );
    assert_eq!(draft.enabled, Some(false));
    assert_eq!(draft.timeout_minutes, Some(45));
    assert_eq!(
        draft.cadence,
        Cadence::Once {
            at: "2026-09-23T09:00:00Z".to_owned()
        }
    );
}

#[test]
fn list_rows_print_each_cadence_and_an_em_dash_for_empty_cells() {
    let every = sample(
        Cadence::Every { minutes: 15 },
        vec![run(
            "2026-09-23T09:00:00Z",
            Some(ScheduleOutcome::Succeeded),
            Some("3 created, 5 existing, 0 reopened"),
        )],
    );
    assert_eq!(
        output::list_row(&every),
        [
            "sch-0a1b2c3d",
            "GitHub reviews",
            "every 15m",
            "2026-09-23 09:15",
            "succeeded",
            "3 created, 5 existing, 0 reopened",
        ]
    );
    let mut once = sample(
        Cadence::Once {
            at: "2026-09-23T09:00:00+02:00".to_owned(),
        },
        Vec::new(),
    );
    once.next_run_at = None;
    assert_eq!(
        output::list_row(&once),
        [
            "sch-0a1b2c3d",
            "GitHub reviews",
            "once 2026-09-23 09:00+02:00",
            "—",
            "—",
            "—",
        ]
    );
    assert_eq!(
        output::cadence(&Cadence::Once {
            at: "2026-09-23T09:00:00Z".to_owned()
        }),
        "once 2026-09-23 09:00"
    );
    assert_eq!(
        output::list(&board("work"), &[]),
        "No schedules on board work"
    );
    let table = output::list(&board("work"), &[every]);
    let mut lines = table.lines();
    assert!(
        lines.next().is_some_and(|header| header.starts_with("ID")
            && header.contains("CADENCE")
            && header.contains("LAST SUMMARY")),
        "{table}"
    );
    assert!(
        lines
            .next()
            .is_some_and(|row| row.starts_with("sch-0a1b2c3d")),
        "{table}"
    );
}

#[test]
fn show_lists_the_last_five_runs_and_runs_lists_every_log_path() {
    let runs = (0..7)
        .map(|minute| {
            run(
                &format!("2026-09-23T09:0{minute}:00Z"),
                Some(ScheduleOutcome::Failed),
                None,
            )
        })
        .collect::<Vec<_>>();
    let schedule = sample(Cadence::Every { minutes: 15 }, runs);
    let shown = output::show(&schedule);
    assert!(shown.contains("runs (last 5 of 7):"), "{shown}");
    assert!(shown.contains("2026-09-23 09:06"), "{shown}");
    assert!(!shown.contains("2026-09-23 09:01"), "{shown}");
    assert!(shown.contains("claude \u{b7} full-access"), "{shown}");
    let shown_runs = shown
        .lines()
        .filter(|line| line.trim_start().starts_with("2026-09-23 09:"))
        .collect::<Vec<_>>();
    assert_eq!(shown_runs.len(), 5, "{shown}");
    assert!(shown_runs[0].trim_start().starts_with("2026-09-23 09:02"));
    assert!(shown_runs[4].trim_start().starts_with("2026-09-23 09:06"));
    let listed = output::runs(&schedule);
    assert_eq!(listed.lines().count(), 8, "{listed}");
    let mut lines = listed.lines();
    assert!(
        lines
            .next()
            .is_some_and(|header| header.starts_with("STARTED") && header.contains("LOG")),
        "{listed}"
    );
    assert!(
        lines.all(|line| line.contains("/logs/20260923T090000Z.log") && line.contains("$0.13")),
        "{listed}"
    );
    // Newest last, as `fleet board card runs` lists a card's.
    assert!(
        listed
            .lines()
            .nth(1)
            .is_some_and(|row| row.starts_with("2026-09-23 09:00"))
    );
    assert!(
        listed
            .lines()
            .last()
            .is_some_and(|row| row.starts_with("2026-09-23 09:06"))
    );
}

#[test]
fn run_prints_the_started_job_and_with_wait_how_it_ended() {
    let schedule_id = id("sch-0a1b2c3d");
    let going = run("2026-09-23T09:00:00Z", None, None);
    assert_eq!(
        output::started(&schedule_id, &going, false),
        "Started job-7"
    );
    let done = run(
        "2026-09-23T09:00:00Z",
        Some(ScheduleOutcome::TimedOut),
        Some("0 created"),
    );
    assert_eq!(
        output::started(&schedule_id, &done, true),
        "Started job-7\ntimed out: 0 created"
    );
    let no_output_timeout = ScheduleRun {
        summary: Some("stopped after 20 min".to_owned()),
        ..done
    };
    let printed = output::started(&schedule_id, &no_output_timeout, true);
    assert_eq!(printed, "Started job-7\ntimed out: stopped after 20 min");
    assert_eq!(printed.matches("timed out").count(), 1);
    let skipped = ScheduleRun {
        job_id: None,
        ..run(
            "2026-09-23T09:00:00Z",
            Some(ScheduleOutcome::Skipped),
            Some("the previous run was still going"),
        )
    };
    assert_eq!(
        output::started(&schedule_id, &skipped, false),
        "Skipped sch-0a1b2c3d: the previous run was still going"
    );
}

#[test]
fn envelopes_carry_protocol_one_and_the_schedule_as_the_wire_spells_it() {
    let schedule = sample(Cadence::Every { minutes: 15 }, Vec::new());
    let one = serde_json::to_value(output::ScheduleEnvelope::new(&schedule))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(one["protocol"], 1);
    assert_eq!(one["schedule"]["id"], "sch-0a1b2c3d");
    assert_eq!(one["schedule"]["boardId"], "reviews-work");
    assert_eq!(one["schedule"]["cadence"]["kind"], "every");
    let many = serde_json::to_value(output::SchedulesEnvelope::new(std::slice::from_ref(
        &schedule,
    )))
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(many["protocol"], 1);
    assert_eq!(many["schedules"][0]["name"], "GitHub reviews");
}

#[test]
fn times_drop_seconds_and_keep_only_a_non_utc_offset() {
    assert_eq!(output::time("2026-09-23T09:00:00Z"), "2026-09-23 09:00");
    assert_eq!(
        output::time("2026-09-23T09:00:00.123+00:00"),
        "2026-09-23 09:00"
    );
    assert_eq!(
        output::time("2026-09-23T09:00:00-05:00"),
        "2026-09-23 09:00-05:00"
    );
    assert_eq!(output::time("soon"), "soon");
}
