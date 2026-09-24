use chrono::{FixedOffset, Utc};
use fleet_core::{
    ids::{BoardId, ScheduleId},
    schedule::{Cadence, Schedule, ScheduleAgent, ScheduleOutcome, ScheduleRun},
};

use super::*;

/// 2026-09-24T13:50:00Z, the clock every strip here is read against.
const NOW: i64 = 1_790_257_800;

fn schedule(id: &str, name: &str, next: Option<&str>) -> Schedule {
    Schedule {
        id: ScheduleId::try_from(id).unwrap_or_else(|error| panic!("{error}")),
        board_id: BoardId::try_from("reviews").unwrap_or_else(|error| panic!("{error}")),
        name: name.to_owned(),
        prompt: "List my review requests.".to_owned(),
        cadence: Cadence::Every { minutes: 15 },
        agent: ScheduleAgent::default(),
        enabled: true,
        timeout_minutes: 20,
        created_at: "2026-09-24T12:00:00Z".to_owned(),
        updated_at: "2026-09-24T12:00:00Z".to_owned(),
        runs: Vec::new(),
        next_run_at: next.map(ToOwned::to_owned),
    }
}

fn run(outcome: Option<ScheduleOutcome>, summary: Option<&str>) -> ScheduleRun {
    ScheduleRun {
        job_id: None,
        started_at: "2026-09-24T13:35:00Z".to_owned(),
        ended_at: outcome.map(|_| "2026-09-24T13:36:00Z".to_owned()),
        outcome,
        summary: summary.map(ToOwned::to_owned),
        cost_usd: None,
        log_path: None,
    }
}

fn strip(schedules: &[Schedule]) -> Option<ScheduleStrip> {
    ScheduleStrip::in_zone(true, schedules, NOW, &Utc)
}

fn label(schedules: &[Schedule]) -> String {
    strip(schedules).map_or_else(String::new, |strip| strip.label.to_string())
}

#[test]
fn no_schedule_is_no_strip() {
    assert_eq!(strip(&[]), None);
}

#[test]
fn no_strip_without_the_capability() {
    let schedules = [schedule(
        "sch-00000001",
        "GitHub reviews",
        Some("2026-09-24T14:05:00Z"),
    )];
    assert_eq!(ScheduleStrip::in_zone(false, &schedules, NOW, &Utc), None);
    assert!(ScheduleStrip::of(true, &schedules, NOW).is_some());
}

#[test]
fn one_schedule_names_itself_its_next_run_and_its_last_summary() {
    let mut one = schedule(
        "sch-00000001",
        "GitHub reviews",
        Some("2026-09-24T14:05:00Z"),
    );
    assert_eq!(
        label(&[one.clone()]),
        "\u{27f3} GitHub reviews \u{b7} next 14:05"
    );
    one.runs.push(run(
        Some(ScheduleOutcome::Succeeded),
        Some("3 created, 5 existing, 0 reopened"),
    ));
    let strip = strip(&[one]).unwrap_or_else(|| panic!("a schedule draws a strip"));
    assert_eq!(
        strip.label.as_ref(),
        "\u{27f3} GitHub reviews \u{b7} next 14:05 \u{b7} last 3 created"
    );
    assert!(!strip.failed);
}

#[test]
fn several_schedules_are_counted_with_the_soonest_next_run() {
    let schedules = [
        schedule(
            "sch-00000001",
            "GitHub reviews",
            Some("2026-09-24T14:20:00Z"),
        ),
        schedule("sch-00000002", "Slack asks", Some("2026-09-24T14:05:00Z")),
        schedule("sch-00000003", "Mail", Some("2026-09-25T09:00:00Z")),
    ];
    assert_eq!(label(&schedules), "\u{27f3} 3 schedules \u{b7} next 14:05");
}

#[test]
fn the_next_run_reads_the_local_clock_and_names_another_day() {
    let one = [schedule(
        "sch-00000001",
        "GitHub reviews",
        Some("2026-09-24T14:05:00Z"),
    )];
    let santiago = FixedOffset::west_opt(3 * 3600).unwrap_or_else(|| panic!("valid offset"));
    let strip = ScheduleStrip::in_zone(true, &one, NOW, &santiago)
        .unwrap_or_else(|| panic!("a schedule draws a strip"));
    assert_eq!(
        strip.label.as_ref(),
        "\u{27f3} GitHub reviews \u{b7} next 11:05"
    );
    let tomorrow = [schedule(
        "sch-00000001",
        "GitHub reviews",
        Some("2026-09-25T09:00:00Z"),
    )];
    assert_eq!(
        label(&tomorrow),
        "\u{27f3} GitHub reviews \u{b7} next Sep 25 09:00"
    );
    let due = [schedule(
        "sch-00000001",
        "GitHub reviews",
        Some("2026-09-24T13:45:00Z"),
    )];
    assert_eq!(label(&due), "\u{27f3} GitHub reviews \u{b7} next now");
}

#[test]
fn a_disabled_schedule_says_so_instead_of_a_next_run() {
    let mut one = schedule("sch-00000001", "GitHub reviews", None);
    one.enabled = false;
    assert_eq!(label(&[one]), "\u{27f3} GitHub reviews \u{b7} disabled");
}

#[test]
fn a_failed_or_timed_out_last_run_takes_the_warning_tone() {
    for (outcome, words) in [
        (ScheduleOutcome::Failed, "last failed"),
        (ScheduleOutcome::TimedOut, "last timed out"),
    ] {
        let mut one = schedule(
            "sch-00000001",
            "GitHub reviews",
            Some("2026-09-24T14:05:00Z"),
        );
        one.runs.push(run(Some(outcome), None));
        let strip = strip(&[one]).unwrap_or_else(|| panic!("a schedule draws a strip"));
        assert!(strip.failed, "{outcome:?}");
        assert!(strip.label.ends_with(words), "{}", strip.label);
    }
    // One failure among several tints the whole strip.
    let mut failed = schedule("sch-00000002", "Slack asks", None);
    failed.runs.push(run(Some(ScheduleOutcome::Failed), None));
    let schedules = [
        schedule(
            "sch-00000001",
            "GitHub reviews",
            Some("2026-09-24T14:05:00Z"),
        ),
        failed,
    ];
    assert!(strip(&schedules).is_some_and(|strip| strip.failed));
}

#[test]
fn a_skipped_fire_does_not_hide_the_run_before_it() {
    let mut one = schedule(
        "sch-00000001",
        "GitHub reviews",
        Some("2026-09-24T14:05:00Z"),
    );
    one.runs.push(run(Some(ScheduleOutcome::Failed), None));
    one.runs.push(run(Some(ScheduleOutcome::Skipped), None));
    let strip = strip(&[one]).unwrap_or_else(|| panic!("a schedule draws a strip"));
    assert!(strip.failed);
    assert!(strip.label.ends_with("last failed"), "{}", strip.label);
}

#[test]
fn a_live_run_reads_running_and_is_not_a_failure() {
    let mut one = schedule(
        "sch-00000001",
        "GitHub reviews",
        Some("2026-09-24T14:05:00Z"),
    );
    one.runs.push(run(Some(ScheduleOutcome::Failed), None));
    one.runs.push(run(None, None));
    let strip = strip(&[one]).unwrap_or_else(|| panic!("a schedule draws a strip"));
    assert!(!strip.failed);
    assert!(strip.label.ends_with("running"), "{}", strip.label);
}

#[test]
fn run_schedules_fires_every_enabled_schedule_once() {
    let mut off = schedule("sch-00000002", "Slack asks", None);
    off.enabled = false;
    let schedules = [
        schedule("sch-00000001", "GitHub reviews", None),
        off,
        schedule("sch-00000003", "Mail", None),
    ];
    let ids: Vec<String> = runnable_schedules(&schedules)
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect();
    assert_eq!(ids, ["sch-00000001", "sch-00000003"]);
    assert!(runnable_schedules(&[]).is_empty());
}

#[test]
fn a_long_summary_clause_is_capped_in_the_strip() {
    let mut one = schedule("sch-00000001", "GitHub reviews", None);
    let line = "x".repeat(280);
    one.runs
        .push(run(Some(ScheduleOutcome::Succeeded), Some(line.as_str())));
    let label = label(&[one]);
    let clause = label
        .rsplit(" \u{b7} ")
        .next()
        .unwrap_or_else(|| panic!("{label}"));
    assert!(clause.starts_with("last x"), "{clause}");
    assert!(clause.ends_with(fleet_ui_kit::ELLIPSIS), "{clause}");
    assert_eq!(
        clause.trim_start_matches("last ").chars().count(),
        SUMMARY_BUDGET
    );
}
