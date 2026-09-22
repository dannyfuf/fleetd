//! Unit tests for the local column-vector edits.
//!
//! Every verb is planned here against a board held in memory, so the arithmetic — where a column
//! lands, what survives an edit, which cards a removal has to move — is asserted without a
//! daemon. The request each verb finally sends is asserted against a scripted socket in
//! `commands/board/tests.rs`.

use super::{Planned, plan, table};
use crate::args::{BoardCommand, Cli, Command};
use clap::Parser;
use fleet_core::{
    board::{ActionKind, BoardView, Card, PRESET_REVIEW_SKILL, Status, StatusCategory, new_board},
    ids::StatusId,
    model::Context,
};
use fleet_proto::error::ProtoError;
use serde_json::json;

fn view() -> BoardView {
    let mut board = new_board(
        &Context {
            id: "work".parse().expect("static context id is valid"),
            name: "Fleet".into(),
            owners: vec![],
            created_at: "now".into(),
        },
        "now",
    );
    board.prefix = "FLT".into();
    BoardView {
        board,
        cards: Vec::new(),
        live_runs: Vec::new(),
    }
}

fn card(id: &str, number: u64, status: &str, archived: bool) -> Card {
    serde_json::from_value(json!({
        "id": id, "boardId": "work", "number": number, "title": "Card",
        "statusId": status, "createdAt": "now", "updatedAt": "now", "archived": archived
    }))
    .expect("static card fixture is valid")
}

/// Parses `fleet board columns <arguments>` and plans it against `view`.
fn planned(view: &BoardView, arguments: &[&str]) -> Result<Planned, ProtoError> {
    let argv = ["fleet", "board", "columns"]
        .into_iter()
        .chain(arguments.iter().copied());
    let Some(Command::Board(board)) = Cli::try_parse_from(argv)
        .expect("static argument vector parses")
        .command
    else {
        panic!("expected a board command");
    };
    let BoardCommand::Columns(arguments) = board.command else {
        panic!("expected the columns command");
    };
    plan(
        view,
        arguments.command.expect("every case here names a verb"),
    )
}

fn ids(statuses: &[Status]) -> Vec<&str> {
    statuses.iter().map(|status| status.id.as_str()).collect()
}

fn status<'a>(statuses: &'a [Status], id: &str) -> &'a Status {
    statuses
        .iter()
        .find(|status| status.id.as_str() == id)
        .expect("the fixture board carries this column")
}

fn on_enter<'a>(statuses: &'a [Status], id: &str) -> Option<&'a ActionKind> {
    status(statuses, id)
        .automation
        .as_ref()
        .and_then(|automation| automation.on_enter.as_ref())
        .map(|action| &action.kind)
}

#[test]
fn add_appends_and_slugs_the_name_when_no_id_is_given() {
    let view = view();
    let planned = planned(&view, &["add", "In Review"]).expect("add plans");
    assert!(planned.changed);
    assert!(planned.moves.is_empty());
    assert_eq!(
        ids(&planned.statuses),
        [
            "backlog",
            "todo",
            "in-progress",
            "done",
            "canceled",
            "in-review"
        ]
    );
    let added = status(&planned.statuses, "in-review");
    assert_eq!(added.name, "In Review");
    assert_eq!(added.category, StatusCategory::Unstarted);
    assert!(added.automation.is_none());
}

#[test]
fn add_places_the_column_where_after_and_before_ask() {
    let view = view();
    let after = planned(&view, &["add", "Ready", "--after", "todo"]).expect("add plans");
    assert_eq!(
        ids(&after.statuses),
        [
            "backlog",
            "todo",
            "ready",
            "in-progress",
            "done",
            "canceled"
        ]
    );
    // The neighbour resolves by name as well as by id, exactly as `card move` does.
    let before = planned(&view, &["add", "Ready", "--before", "In Progress"]).expect("add plans");
    assert_eq!(ids(&before.statuses), ids(&after.statuses));
}

#[test]
fn add_takes_an_explicit_id_and_category() {
    let view = view();
    let planned = planned(
        &view,
        &[
            "add",
            "Shipped",
            "--id",
            "shipped",
            "--category",
            "completed",
        ],
    )
    .expect("add plans");
    let added = status(&planned.statuses, "shipped");
    assert_eq!(added.name, "Shipped");
    assert_eq!(added.category, StatusCategory::Completed);
}

#[test]
fn add_refuses_a_duplicate_id_an_unusable_name_and_an_invalid_id() {
    let view = view();
    for (arguments, reason) in [
        (vec!["add", "Done"], "column `done` already exists"),
        (
            vec!["add", "///"],
            "column `///` must contain a letter or number",
        ),
    ] {
        let error = planned(&view, &arguments).expect_err("add is refused");
        assert_eq!(error.message, reason);
    }
    // The slug rule is `fleet-core`'s and speaks for itself; the CLI passes its sentence on
    // rather than writing a second one.
    let error =
        planned(&view, &["add", "Ready", "--id", "Not A Slug"]).expect_err("add is refused");
    assert!(
        error.message.contains("Not A Slug") && error.message.contains("^[a-z0-9]"),
        "the id refusal quotes the value and the rule: {}",
        error.message
    );
}

#[test]
fn add_refuses_a_neighbour_that_names_nothing() {
    let view = view();
    let error =
        planned(&view, &["add", "Ready", "--after", "nowhere"]).expect_err("add is refused");
    assert_eq!(error.message, "status `nowhere` was not found");
}

#[test]
fn edit_renames_recategorises_and_recolours_a_column() {
    let view = view();
    let planned = planned(
        &view,
        &[
            "edit",
            "in-progress",
            "--name",
            "Working",
            "--category",
            "started",
            "--color",
            "accent",
        ],
    )
    .expect("edit plans");
    let edited = status(&planned.statuses, "in-progress");
    assert_eq!(edited.name, "Working");
    assert_eq!(edited.category, StatusCategory::Started);
    assert_eq!(edited.color.as_deref(), Some("accent"));
}

#[test]
fn edit_writes_an_action_with_every_flag_it_carries() {
    let view = view();
    let planned = planned(
        &view,
        &[
            "edit",
            "In Progress",
            "--on-enter",
            "prompt",
            "--provider",
            "claude",
            "--model",
            "opus",
            "--effort",
            "high",
            "--mode",
            "full-access",
            "--instructions",
            "Implement {key}.",
            "--expect",
            "make lint passes",
            "--env",
            "RUST_LOG=debug",
            "--env",
            "CI=1",
        ],
    )
    .expect("edit plans");
    let automation = status(&planned.statuses, "in-progress")
        .automation
        .as_ref()
        .expect("the column carries automation");
    let action = automation.on_enter.as_ref().expect("the column runs");
    assert_eq!(action.kind, ActionKind::Prompt);
    assert_eq!(action.instructions, "Implement {key}.");
    assert_eq!(action.expect, "make lint passes");
    assert_eq!(
        action.agent.provider,
        Some(fleet_core::agents::AgentKind::Claude)
    );
    assert_eq!(action.agent.model.as_deref(), Some("opus"));
    assert_eq!(action.agent.effort.as_deref(), Some("high"));
    assert_eq!(
        action.agent.mode,
        Some(fleet_core::agents::PermissionMode::FullAccess)
    );
    assert_eq!(action.env, ["RUST_LOG=debug", "CI=1"]);
}

#[test]
fn edit_parses_every_on_enter_word_and_refuses_the_rest() {
    let view = view();
    assert_eq!(
        on_enter(
            &planned(&view, &["edit", "done", "--on-enter", "prompt"])
                .expect("edit plans")
                .statuses,
            "done"
        ),
        Some(&ActionKind::Prompt)
    );
    let skill = planned(
        &view,
        &["edit", "done", "--on-enter", "skill:deep-review:--fast"],
    )
    .expect("edit plans");
    assert_eq!(
        on_enter(&skill.statuses, "done"),
        Some(&ActionKind::Skill {
            name: "deep-review".into(),
            args: "--fast".into(),
        })
    );
    let bare =
        planned(&view, &["edit", "done", "--on-enter", "skill:deep-review"]).expect("edit plans");
    assert_eq!(
        on_enter(&bare.statuses, "done"),
        Some(&ActionKind::Skill {
            name: "deep-review".into(),
            args: String::new(),
        })
    );
    let error = planned(&view, &["edit", "done", "--on-enter", "review"])
        .expect_err("an unknown word is refused");
    assert_eq!(
        error.message,
        "--on-enter must be none, prompt, or skill:<name>[:<args>]"
    );
}

#[test]
fn edit_keeps_the_actions_other_fields_when_only_the_kind_changes() {
    let mut view = view();
    let with_action = planned(
        &view,
        &[
            "edit",
            "in-progress",
            "--on-enter",
            "prompt",
            "--instructions",
            "Implement it.",
            "--expect",
            "green",
        ],
    )
    .expect("edit plans");
    view.board.statuses = with_action.statuses;
    let swapped = planned(
        &view,
        &["edit", "in-progress", "--on-enter", "skill:deep-review"],
    )
    .expect("edit plans");
    let action = status(&swapped.statuses, "in-progress")
        .automation
        .as_ref()
        .and_then(|automation| automation.on_enter.as_ref())
        .expect("the column still runs");
    assert_eq!(
        action.kind,
        ActionKind::Skill {
            name: "deep-review".into(),
            args: String::new(),
        }
    );
    assert_eq!(action.instructions, "Implement it.");
    assert_eq!(action.expect, "green");
}

#[test]
fn edit_clears_an_action_with_none_and_drops_an_emptied_block() {
    let mut view = view();
    let with_action =
        planned(&view, &["edit", "in-progress", "--on-enter", "prompt"]).expect("edit plans");
    view.board.statuses = with_action.statuses;
    let cleared =
        planned(&view, &["edit", "in-progress", "--on-enter", "none"]).expect("edit plans");
    // The whole block goes, not just the action: a column that asks for nothing is not
    // automated, and an empty block would hold the document at version 2.
    assert!(
        status(&cleared.statuses, "in-progress")
            .automation
            .is_none()
    );
}

#[test]
fn edit_refuses_an_action_flag_on_a_column_that_runs_nothing() {
    let view = view();
    let error = planned(&view, &["edit", "done", "--instructions", "Ship it."])
        .expect_err("the flag is refused");
    assert_eq!(
        error.message,
        "column `Done` runs nothing; give it an action with --on-enter prompt or --on-enter skill:<name>"
    );
}

#[test]
fn edit_reads_the_instructions_from_a_file() {
    let view = view();
    let directory = tempfile::tempdir().expect("a temporary directory is available");
    let path = directory.path().join("instructions.md");
    std::fs::write(&path, "Implement {key} in this worktree.")
        .expect("the fixture file is written");
    let from_file = planned(
        &view,
        &[
            "edit",
            "in-progress",
            "--on-enter",
            "prompt",
            "--instructions-file",
            path.to_str().expect("the temporary path is UTF-8"),
        ],
    )
    .expect("edit plans");
    let action = status(&from_file.statuses, "in-progress")
        .automation
        .as_ref()
        .and_then(|automation| automation.on_enter.as_ref())
        .expect("the column runs");
    assert_eq!(action.instructions, "Implement {key} in this worktree.");
    let missing = planned(
        &view,
        &[
            "edit",
            "in-progress",
            "--on-enter",
            "prompt",
            "--instructions-file",
            "/nonexistent/instructions.md",
        ],
    )
    .expect_err("a missing file is refused");
    assert!(
        missing
            .message
            .starts_with("could not read instructions file /nonexistent/instructions.md:"),
        "the refusal names the file: {}",
        missing.message
    );
}

#[test]
fn edit_clears_and_accumulates_the_environment() {
    let mut view = view();
    let first = planned(
        &view,
        &[
            "edit",
            "in-progress",
            "--on-enter",
            "prompt",
            "--env",
            "RUST_LOG=debug",
        ],
    )
    .expect("edit plans");
    view.board.statuses = first.statuses;
    let appended = planned(&view, &["edit", "in-progress", "--env", "CI=1"]).expect("edit plans");
    let env = |statuses: &[Status]| {
        status(statuses, "in-progress")
            .automation
            .as_ref()
            .and_then(|automation| automation.on_enter.as_ref())
            .map(|action| action.env.clone())
            .expect("the column runs")
    };
    assert_eq!(env(&appended.statuses), ["RUST_LOG=debug", "CI=1"]);
    let cleared = planned(&view, &["edit", "in-progress", "--clear-env"]).expect("edit plans");
    assert!(env(&cleared.statuses).is_empty());

    // A key given again *replaces* the value it had. Appending would leave the column carrying
    // it twice, which the daemon refuses as "given twice" — so there would be no single command
    // that changes one variable, only a clear and a retype of every other one.
    let replaced =
        planned(&view, &["edit", "in-progress", "--env", "RUST_LOG=trace"]).expect("edit plans");
    assert_eq!(env(&replaced.statuses), ["RUST_LOG=trace"]);

    // And the pair reads as "these are the variables now", which is the other half of the same
    // gesture: `--clear-env` empties the column before any `--env` is applied.
    let replaced_all = planned(
        &view,
        &["edit", "in-progress", "--clear-env", "--env", "CI=1"],
    )
    .expect("edit plans");
    assert_eq!(env(&replaced_all.statuses), ["CI=1"]);
}

#[test]
fn edit_writes_and_clears_both_routes() {
    let mut view = view();
    let routed = planned(
        &view,
        &[
            "edit",
            "in-progress",
            "--on-success",
            "Done",
            "--when-unblocked",
            "in-progress",
        ],
    )
    .expect("edit plans");
    let automation = status(&routed.statuses, "in-progress")
        .automation
        .as_ref()
        .expect("the column carries automation");
    assert_eq!(automation.on_success, Some(done_id()));
    assert_eq!(
        automation.advance_when_unblocked,
        Some(StatusId::try_from("in-progress").expect("static status slug is valid"))
    );
    view.board.statuses = routed.statuses;
    let cleared = planned(
        &view,
        &[
            "edit",
            "in-progress",
            "--no-on-success",
            "--no-when-unblocked",
        ],
    )
    .expect("edit plans");
    assert!(
        status(&cleared.statuses, "in-progress")
            .automation
            .is_none()
    );
}

fn done_id() -> StatusId {
    StatusId::try_from("done").expect("static status slug is valid")
}

#[test]
fn edit_refuses_a_route_that_names_nothing() {
    let view = view();
    let error = planned(&view, &["edit", "todo", "--on-success", "shipped"])
        .expect_err("the route is refused");
    assert_eq!(error.message, "status `shipped` was not found");
}

#[test]
fn move_repositions_in_both_directions() {
    let view = view();
    let forward = planned(&view, &["move", "todo", "--after", "done"]).expect("move plans");
    assert_eq!(
        ids(&forward.statuses),
        ["backlog", "in-progress", "done", "todo", "canceled"]
    );
    let backward = planned(&view, &["move", "done", "--before", "todo"]).expect("move plans");
    assert_eq!(
        ids(&backward.statuses),
        ["backlog", "done", "todo", "in-progress", "canceled"]
    );
    let adjacent = planned(&view, &["move", "todo", "--after", "in-progress"]).expect("move plans");
    assert_eq!(
        ids(&adjacent.statuses),
        ["backlog", "in-progress", "todo", "done", "canceled"]
    );
}

#[test]
fn move_refuses_a_column_relative_to_itself() {
    let view = view();
    let error = planned(&view, &["move", "todo", "--after", "Todo"]).expect_err("move is refused");
    assert_eq!(error.message, "a column cannot move relative to itself");
}

#[test]
fn remove_drops_the_column_and_moves_every_card_archived_ones_included() {
    let mut view = view();
    view.cards = vec![
        card("card-1", 1, "in-progress", false),
        card("card-2", 2, "todo", false),
        card("card-3", 3, "in-progress", true),
    ];
    let planned = planned(&view, &["remove", "in-progress", "--move-cards-to", "Todo"])
        .expect("remove plans");
    assert_eq!(
        ids(&planned.statuses),
        ["backlog", "todo", "done", "canceled"]
    );
    let todo = StatusId::try_from("todo").expect("static status slug is valid");
    assert_eq!(
        planned.moves,
        vec![
            (
                "card-1".parse().expect("static card id is valid"),
                todo.clone()
            ),
            ("card-3".parse().expect("static card id is valid"), todo),
        ]
    );
    assert_eq!(planned.notes, ["moved 2 cards to Todo"]);
}

#[test]
fn remove_without_a_target_sends_no_moves() {
    let mut view = view();
    view.cards = vec![card("card-1", 1, "in-progress", false)];
    let planned = planned(&view, &["remove", "in-progress"]).expect("remove plans");
    assert!(planned.moves.is_empty());
    assert!(planned.notes.is_empty());
    assert_eq!(
        ids(&planned.statuses),
        ["backlog", "todo", "done", "canceled"]
    );
}

#[test]
fn remove_refuses_moving_the_cards_into_the_column_it_removes() {
    let view = view();
    let error = planned(
        &view,
        &["remove", "in-progress", "--move-cards-to", "In Progress"],
    )
    .expect_err("remove is refused");
    assert_eq!(
        error.message,
        "--move-cards-to must name a different column"
    );
}

#[test]
fn preset_adds_the_missing_columns_and_says_the_shipped_one_runs_nothing() {
    let view = view();
    let planned = planned(&view, &["preset", "workflow"]).expect("preset plans");
    assert!(planned.changed);
    assert_eq!(
        ids(&planned.statuses),
        [
            "backlog",
            "todo",
            "ready",
            "in-progress",
            "in-review",
            "done",
            "canceled"
        ]
    );
    assert_eq!(
        on_enter(&planned.statuses, "in-review"),
        Some(&ActionKind::Skill {
            name: PRESET_REVIEW_SKILL.into(),
            args: String::new(),
        })
    );
    // The shipped In Progress column predates the preset, so the preset left it alone and the
    // verb is the only thing that can say so.
    assert!(on_enter(&planned.statuses, "in-progress").is_none());
    assert_eq!(
        planned.notes,
        [
            "In Progress was already here, so the preset left it alone and it still runs nothing; give it one with: fleet board columns edit in-progress --on-enter prompt"
        ]
    );
}

#[test]
fn preset_applied_twice_asks_for_no_second_write() {
    let mut view = view();
    let first = planned(&view, &["preset", "workflow"]).expect("preset plans");
    view.board.statuses = first.statuses;
    let second = planned(&view, &["preset", "workflow"]).expect("preset plans");
    assert!(!second.changed);
    assert!(second.moves.is_empty());
    assert_eq!(
        second.notes.first().map(String::as_str),
        Some("every workflow column is already on this board")
    );
}

#[test]
fn the_table_prints_every_automation_cell_as_its_flag_spells_it() {
    let view = view();
    let planned = planned(&view, &["preset", "workflow"]).expect("preset plans");
    let rows = table(&planned.statuses);
    assert_eq!(
        rows.first().map(String::as_str),
        Some("ID           NAME         CATEGORY   ON ENTER           ON SUCCESS  WHEN UNBLOCKED")
    );
    assert!(
        rows.iter()
            .any(|row| row.starts_with("ready ") && row.ends_with("in-progress")),
        "the ready row routes to in-progress: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("skill:deep-review")),
        "the review row names its skill: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.starts_with("todo ") && row.ends_with('\u{2014}')),
        "a column with no automation prints dashes: {rows:?}"
    );
}
