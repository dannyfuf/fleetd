//! `fleet board --reviews` and `card new --pr`: the Reviews board selector, the pull-request
//! upsert and the lines both print.

use super::*;
use crate::human;
use fleet_core::board::{BoardKind, CardDraft, PullRequestRef, UpsertOutcome};
use fleet_proto::response::BOARD_REVIEWS_CAPABILITY;

const PR_URL: &str = "https://github.com/acme/api/pull/42";

fn reviews_view() -> BoardView {
    let mut view = view();
    view.board.kind = BoardKind::Reviews;
    view
}

fn pull_request() -> PullRequestRef {
    PullRequestRef::parse(PR_URL).unwrap()
}

fn review_card(view: &BoardView) -> Card {
    let mut card = view.cards[0].clone();
    card.pull_request = Some(pull_request());
    card
}

fn upsert(requested_at: Option<&str>) -> RequestBody {
    RequestBody::UpsertPullRequestCard {
        board_id: "work".parse().unwrap(),
        draft: CardDraft {
            title: "Review the login fix".into(),
            pull_request: Some(pull_request()),
            ..CardDraft::default()
        },
        requested_at: requested_at.map(str::to_owned),
    }
}

fn reviews() -> Vec<String> {
    vec![BOARD_REVIEWS_CAPABILITY.to_owned()]
}

#[tokio::test]
async fn reviews_selects_the_reviews_board_of_the_active_or_named_context() {
    let view = reviews_view();
    let snapshot = Snapshot {
        active_context: Some("work".parse().unwrap()),
        ..empty_snapshot()
    };
    let ensure = RequestBody::EnsureReviewsBoard {
        context_id: "work".parse().unwrap(),
    };
    let output = run_with_capabilities(
        &["--reviews", "show"],
        reviews(),
        vec![
            (
                RequestBody::GetSnapshot,
                Ok(ResponseBody::Snapshot(snapshot)),
            ),
            (ensure.clone(), Ok(ResponseBody::Board(view.clone()))),
            (
                RequestBody::ListBoardBackends {},
                Ok(ResponseBody::BoardBackends(Vec::new())),
            ),
        ],
    )
    .await
    .unwrap();
    assert_eq!(output.exit_code, 0);
    let header = output.text.lines().next().unwrap();
    assert!(header.ends_with(" · reviews of work"), "{header}");

    let output = run_with_capabilities(
        &["--reviews", "--context", "work", "card", "show", "flt-12"],
        reviews(),
        vec![(ensure, Ok(ResponseBody::Board(view)))],
    )
    .await
    .unwrap();
    assert_eq!(output.exit_code, 0);
}

#[tokio::test]
async fn reviews_on_an_old_daemon_is_refused_before_any_board_request() {
    let error = run(&["--reviews", "--context", "work", "show"], vec![])
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "this daemon does not support review boards; run `fleet daemon restart`"
    );
}

#[test]
fn reviews_conflicts_with_board_and_worktree_at_one_level() {
    for arguments in [
        ["--reviews", "--board", "work", "show"].as_slice(),
        ["--reviews", "--worktree", "show"].as_slice(),
        ["--reviews", "--worktree=acme/api#feature", "show"].as_slice(),
    ] {
        let argv = ["fleet", "board"]
            .into_iter()
            .chain(arguments.iter().copied());
        assert!(Cli::try_parse_from(argv).is_err(), "accepted {arguments:?}");
    }
}

#[tokio::test]
async fn reviews_conflicts_with_board_and_worktree_across_levels() {
    for (arguments, message) in [
        (
            ["--reviews", "card", "show", "FLT-12", "--board", "work"].as_slice(),
            "--board and --reviews cannot be used together",
        ),
        (
            [
                "--reviews",
                "card",
                "show",
                "FLT-12",
                "--worktree=acme/api#feature",
            ]
            .as_slice(),
            "--reviews and --worktree cannot be used together",
        ),
    ] {
        let argv = ["fleet", "board"]
            .into_iter()
            .chain(arguments.iter().copied());
        // Whichever level catches it, the pair never reaches the daemon.
        if Cli::try_parse_from(argv).is_err() {
            continue;
        }
        let error = run(arguments, vec![]).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert_eq!(error.message, message);
    }
}

#[tokio::test]
async fn requested_at_without_pr_is_refused_before_any_card_request() {
    let error = run(
        &[
            "--board",
            "work",
            "card",
            "new",
            "Review the login fix",
            "--requested-at",
            "2026-09-22T10:00:00Z",
        ],
        // Refused before the board is resolved: no request at all goes out.
        vec![],
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(error.message, "--requested-at needs --pr");
}

/// A request time that is not RFC 3339 is refused locally, in the daemon's words, and before
/// `--reviews` would ensure a board.
#[tokio::test]
async fn a_bad_requested_at_is_refused_before_any_request() {
    let error = run(
        &[
            "--reviews",
            "card",
            "new",
            "Review the login fix",
            "--pr",
            PR_URL,
            "--requested-at",
            "yesterday",
        ],
        vec![],
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(
        error.message,
        "invalid requested_at: must be an RFC 3339 time"
    );
}

#[tokio::test]
async fn a_bad_pr_prints_the_parse_sentence() {
    let error = run(
        &[
            "--board",
            "work",
            "card",
            "new",
            "Review the login fix",
            "--pr",
            "acme/api/42",
        ],
        vec![],
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(
        error.message,
        "invalid pull_request: expected a GitHub pull request URL or owner/name#number"
    );
}

#[tokio::test]
async fn card_new_with_pr_leads_with_each_outcome_then_prints_the_card() {
    let view = reviews_view();
    let card = review_card(&view);
    for (outcome, line) in [
        (UpsertOutcome::Created, "Created FLT-12"),
        (UpsertOutcome::Existing, "Existing FLT-12"),
        (UpsertOutcome::Reopened, "Reopened FLT-12"),
    ] {
        let output = run_with_capabilities(
            &[
                "--board",
                "work",
                "card",
                "new",
                "Review the login fix",
                "--pr",
                "acme/api#42",
                "--requested-at",
                "2026-09-22T10:00:00Z",
            ],
            reviews(),
            vec![
                (get_board(), Ok(ResponseBody::Board(view.clone()))),
                (
                    upsert(Some("2026-09-22T10:00:00Z")),
                    Ok(ResponseBody::CardUpsert {
                        card: card.clone(),
                        outcome,
                    }),
                ),
            ],
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, 0);
        let mut lines = output.text.lines();
        assert_eq!(lines.next(), Some(line));
        assert_eq!(lines.next(), Some("FLT-12  Fix login"));
        assert!(
            output
                .text
                .contains(&format!("Pull request: acme/api#42  {PR_URL}")),
            "{}",
            output.text
        );
        // The card the daemon handed back replaces the view's copy instead of printing twice.
        assert_eq!(output.text.matches("FLT-12  Fix login").count(), 1);
    }
}

#[tokio::test]
async fn card_new_with_pr_and_json_prints_the_upsert_envelope() {
    let view = reviews_view();
    let card = review_card(&view);
    let output = run_with_capabilities(
        &[
            "--board",
            "work",
            "card",
            "new",
            "Review the login fix",
            "--pr",
            PR_URL,
            "--json",
        ],
        reviews(),
        vec![
            (get_board(), Ok(ResponseBody::Board(view))),
            (
                upsert(None),
                Ok(ResponseBody::CardUpsert {
                    card,
                    outcome: UpsertOutcome::Reopened,
                }),
            ),
        ],
    )
    .await
    .unwrap();
    let value: serde_json::Value = serde_json::from_str(&output.text).unwrap();
    assert_eq!(value["protocol"], 1);
    assert_eq!(value["outcome"], "reopened");
    assert_eq!(value["card"]["pullRequest"]["url"], PR_URL);
}

#[test]
fn only_a_review_card_prints_a_pull_request_line() {
    let view = view();
    let text = human::board_card(&view.board, &view.cards, &view.cards[0]);
    assert!(!text.contains("Pull request:"), "{text}");
    let card = review_card(&view);
    let text = human::board_card(&view.board, &view.cards, &card);
    let worktree = text
        .lines()
        .position(|line| line.starts_with("Worktree:"))
        .unwrap();
    assert_eq!(
        text.lines().nth(worktree + 1),
        Some(format!("Pull request: acme/api#42  {PR_URL}").as_str())
    );
}

#[test]
fn only_a_reviews_board_header_names_its_context() {
    assert!(!human::board(&view(), None, 0).contains("reviews of"));
    let text = human::board(&reviews_view(), None, 0);
    assert!(
        text.lines().next().unwrap().ends_with(" · reviews of work"),
        "{text}"
    );
}
