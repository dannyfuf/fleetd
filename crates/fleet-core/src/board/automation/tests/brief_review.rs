//! Pull request and user-note sections of the run brief (FEA-4).

use super::*;
use crate::{board::model::PullRequestRef, ids::RepoId};

fn pull_request() -> PullRequestRef {
    PullRequestRef {
        repo: RepoId::try_from("acme/api").expect("a static repository id is valid"),
        number: 7,
        url: "https://github.com/acme/api/pull/7".into(),
    }
}

fn note(id: &str, author: Option<&str>, body: &str, at: &str) -> Comment {
    Comment {
        id: id.into(),
        author: author.map(str::to_owned),
        body: body.into(),
        created_at: at.into(),
        remote_id: None,
        run_id: None,
    }
}

/// Gives the card a run that ended at `at`.
fn ended_at(card: &mut Card, at: &str) {
    card.runs.push(CardRun {
        id: DelegationId::new(),
        thread_id: None,
        status_id: card.status_id.clone(),
        action: ActionKind::Prompt,
        provider: AgentKind::Claude,
        model: None,
        effort: None,
        started_at: EARLIER.into(),
        ended_at: Some(at.into()),
        outcome: Some(RunOutcome::Succeeded),
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    });
}

#[test]
fn a_brief_names_the_pull_request() {
    let mut card = card();
    card.description = "Look at the auth change.".into();
    card.pull_request = Some(pull_request());
    let mut action = action(ActionKind::Prompt);
    action.instructions = "Review {pr_url} ({pr_repo} #{pr_number}) for {key}.".into();

    let brief = brief(&action, "REV-3", &card, &[]);

    assert_eq!(
        brief,
        "Review https://github.com/acme/api/pull/7 (acme/api #7) for REV-3.\n\n\
         # REV-3 — Fix login\n\n\
         Look at the auth change.\n\n\
         ## Pull request\n\n\
         acme/api#7 · https://github.com/acme/api/pull/7\n"
    );
}

#[test]
fn pull_request_placeholders_are_left_alone_without_one() {
    let card = card();
    let mut action = action(ActionKind::Prompt);
    action.instructions = "Print {pr_url}, {pr_repo} and {pr_number} for {key}.".into();

    let brief = brief(&action, "FLE-12", &card, &[]);

    assert!(
        brief.starts_with("Print {pr_url}, {pr_repo} and {pr_number} for FLE-12.\n\n"),
        "{brief}"
    );
    assert!(!brief.contains("## Pull request"), "{brief}");
}

#[test]
fn a_brief_carries_notes_made_after_the_last_run() {
    let mut card = card();
    ended_at(&mut card, NOW);
    card.comments = vec![
        note(
            "before",
            Some("dani"),
            "an old note the last run read",
            EARLIER,
        ),
        note("after", Some("dani"), "please also check the tests", LATER),
    ];

    let brief = brief(&action(ActionKind::Prompt), "FLE-12", &card, &[]);

    assert!(
        brief.ends_with("\n\n## Notes from you\n\n- dani: please also check the tests\n"),
        "{brief}"
    );
    assert!(!brief.contains("an old note the last run read"), "{brief}");
}

/// A note written while the review ran never reached that run's brief, which was written when
/// it started: it reaches the next run instead.
#[test]
fn a_note_written_while_the_review_ran_reaches_the_next_run() {
    let mut card = card();
    ended_at(&mut card, LATER);
    card.comments = vec![note("during", Some("dani"), "drop finding 3", NOW)];

    let brief = brief(&action(ActionKind::Prompt), "REV-3", &card, &[]);

    assert!(
        brief.ends_with("\n\n## Notes from you\n\n- dani: drop finding 3\n"),
        "{brief}"
    );
}

#[test]
fn a_brief_carries_every_note_when_the_card_never_ran() {
    let mut card = card();
    card.comments = vec![
        note("second", Some("  "), "and the second", LATER),
        note("first", None, "the first note", EARLIER),
    ];

    let brief = brief(&action(ActionKind::Prompt), "FLE-12", &card, &[]);

    assert!(
        brief.ends_with("## Notes from you\n\n- you: the first note\n- you: and the second\n"),
        "{brief}"
    );
}

#[test]
fn run_reports_are_not_notes() {
    let mut card = card();
    let mut report = note("report", None, "what the run did", LATER);
    report.run_id = Some(DelegationId::new());
    card.comments = vec![report];

    let brief = brief(&action(ActionKind::Prompt), "FLE-12", &card, &[]);

    assert!(!brief.contains("## Notes from you"), "{brief}");
    assert!(!brief.contains("what the run did"), "{brief}");
}

/// A publish that failed does not swallow the note it was asked to apply, and its own report is
/// told apart from the review's: the retry publishes the review, corrected.
#[test]
fn a_retried_publish_keeps_the_notes_and_names_the_review() {
    let mut card = card();
    card.pull_request = Some(pull_request());
    ended_at(&mut card, "2026-09-22T10:00:00Z");
    let review = card.runs[0].id;
    ended_at(&mut card, "2026-09-22T12:00:00Z");
    let publish = card.runs[1].id;
    card.runs[1].outcome = Some(RunOutcome::Failed);
    let mut review_report = note(
        "review",
        None,
        "verdict: request changes",
        "2026-09-22T10:00:00Z",
    );
    review_report.run_id = Some(review);
    let mut failure = note(
        "failure",
        None,
        "could not post: gh auth",
        "2026-09-22T12:00:00Z",
    );
    failure.run_id = Some(publish);
    card.comments = vec![
        review_report.clone(),
        note(
            "fix",
            Some("dani"),
            "drop finding 3",
            "2026-09-22T11:00:00Z",
        ),
        failure.clone(),
    ];

    let brief = brief(
        &action(ActionKind::Prompt),
        "REV-3",
        &card,
        &[&review_report, &failure],
    );

    assert!(
        brief.contains("## Notes from you\n\n- dani: drop finding 3\n"),
        "{brief}"
    );
    assert!(
        brief.contains(
            "### Report from 2026-09-22T10:00:00Z · succeeded\n\nverdict: request changes"
        ),
        "{brief}"
    );
    assert!(
        brief.contains("### Report from 2026-09-22T12:00:00Z · failed\n\ncould not post: gh auth"),
        "{brief}"
    );
}
