use super::defaults::{default_prefix, new_worktree_board, worktree_board_id};
use super::*;
use crate::{
    agents::{AgentKind, DelegationId, DelegationStatus, PermissionMode, ThreadId},
    ids::{BoardId, CardId, ContextId, WorktreeId},
    model::{Context, Worktree},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, fmt::Debug};

fn board() -> Board {
    new_board(
        &Context {
            id: ContextId::try_from("work").unwrap(),
            name: "Fleet".into(),
            owners: vec![],
            created_at: "2026-09-06T12:00:00Z".into(),
        },
        "2026-09-06T12:00:00Z",
    )
}
fn card(board: &Board) -> Card {
    // Required wire fields alone must produce a complete defaulted card.
    serde_json::from_value(serde_json::json!({
        "id": "card-1", "boardId": board.id, "number": 12, "title": "Fix login",
        "statusId": "todo", "createdAt": board.created_at, "updatedAt": board.updated_at
    }))
    .unwrap()
}
fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    assert_eq!(
        &serde_json::from_str::<T>(&serde_json::to_string(value).unwrap()).unwrap(),
        value
    );
}

fn worktree(id: &str) -> Worktree {
    let id = WorktreeId::try_from(id).unwrap();
    Worktree {
        repo_id: id.repo().parse().unwrap(),
        slug: id.slug().into(),
        id,
        branch: "feature".into(),
        base_ref: "origin/main".into(),
        path: "/tmp/worktree".into(),
        session: "api/feature".into(),
        host: None,
        created_at: "2026-09-06T12:00:00Z".into(),
        last_opened_at: None,
        degraded: None,
    }
}

#[test]
fn duplicate_error_names_a_generic_scope() {
    let error = BoardError::Duplicate("acme/api#feature".into());

    assert_eq!(
        error.to_string(),
        "board already exists for scope acme/api#feature"
    );
}

#[test]
fn board_card_and_document_round_trip() {
    let board = board();
    let mut card = card(&board);
    card.properties
        .insert("score".into(), PropertyValue::Number(2.5));
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "fake".into(),
        key: "REMOTE-4".into(),
        url: None,
        version: Some("v1".into()),
        synced_at: board.created_at.clone(),
        remote_updated_at: None,
    });
    card.conflict = Some(Conflict {
        detected_at: board.updated_at.clone(),
        remote: RemoteCard::default(),
        fields: vec!["title".into()],
    });
    round_trip(&board);
    round_trip(&card);
    round_trip(&BoardDocument {
        version: BOARD_DOCUMENT_VERSION,
        board,
        cards: vec![card],
    });
}

#[test]
fn board_scope_is_additive_and_uses_worktree_id_on_the_wire() {
    let mut old_json = serde_json::to_value(board()).unwrap();
    old_json.as_object_mut().unwrap().remove("worktreeId");
    let old_board: Board = serde_json::from_value(old_json).unwrap();
    assert_eq!(old_board.worktree_id, None);

    let mut scoped = board();
    scoped.worktree_id = Some("acme/api#feature".parse().unwrap());
    let scoped_json = serde_json::to_value(&scoped).unwrap();
    assert_eq!(scoped_json["worktreeId"], "acme/api#feature");
    round_trip(&scoped);

    let summary = summarize(&scoped, &[], &[], "2026-09-06T12:00:00Z");
    assert_eq!(summary.worktree_id, scoped.worktree_id);
    round_trip(&summary);
}

#[test]
fn worktree_defaults_derive_valid_ids_and_prefixes() {
    assert_eq!(
        worktree_board_id(&"acme/api#feature".parse().unwrap()).as_str(),
        "wt-acme-api-feature"
    );
    let unusual: WorktreeId = "ACME/Prój.ect#Fïx_Ünicode".parse().unwrap();
    assert_eq!(
        worktree_board_id(&unusual).as_str(),
        "wt-acme-pr-j-ect-f-x-nicode"
    );

    let long: WorktreeId = format!("{}/{}#{}", "a".repeat(80), "b".repeat(80), "c".repeat(80))
        .parse()
        .unwrap();
    let long_id = worktree_board_id(&long);
    assert!(long_id.as_str().len() <= 64);
    assert!(!long_id.as_str().ends_with('-'));
    assert!(BoardId::try_from(long_id.as_str()).is_ok());

    let context = Context {
        id: "work".parse().unwrap(),
        name: "Work".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let scoped = new_worktree_board(&context, &worktree("acme/api#中文"), "2026-09-06T12:00:00Z");
    assert_eq!(scoped.id.as_str(), "wt-acme-api-x");
    assert_eq!(
        scoped.worktree_id.as_ref().map(WorktreeId::as_str),
        Some("acme/api#中文")
    );
    assert_eq!(scoped.name, "中文");
    assert_eq!(scoped.prefix, "WT");
    assert_eq!(
        scoped.default_repo_id.as_ref().map(|id| id.as_str()),
        Some("acme/api")
    );
}
#[test]
fn property_value_wire_tags_and_display() {
    let cases = [
        (
            PropertyValue::Text("text".into()),
            serde_json::json!({"kind":"text","value":"text"}),
            Some(PropertyKind::Text),
        ),
        (
            PropertyValue::Number(2.5),
            serde_json::json!({"kind":"number","value":2.5}),
            Some(PropertyKind::Number),
        ),
        (
            PropertyValue::Bool(true),
            serde_json::json!({"kind":"bool","value":true}),
            Some(PropertyKind::Bool),
        ),
        (
            PropertyValue::Date("2026-09-06".into()),
            serde_json::json!({"kind":"date","value":"2026-09-06"}),
            Some(PropertyKind::Date),
        ),
        (
            PropertyValue::Select("a".into()),
            serde_json::json!({"kind":"select","value":"a"}),
            Some(PropertyKind::Select),
        ),
        (
            PropertyValue::MultiSelect(vec!["a".into(), "b".into()]),
            serde_json::json!({"kind":"multi_select","value":["a","b"]}),
            Some(PropertyKind::MultiSelect),
        ),
        (
            PropertyValue::User("Danny".into()),
            serde_json::json!({"kind":"user","value":"Danny"}),
            Some(PropertyKind::User),
        ),
        (
            PropertyValue::Url("https://example.com".into()),
            serde_json::json!({"kind":"url","value":"https://example.com"}),
            Some(PropertyKind::Url),
        ),
        (
            PropertyValue::Null,
            serde_json::json!({"kind":"null"}),
            None,
        ),
    ];
    for (value, json, kind) in cases {
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        round_trip(&value);
        if let Some(kind) = kind {
            assert!(value.matches_kind(kind));
        }
    }
    assert_eq!(
        PropertyValue::MultiSelect(vec!["a".into(), "b".into()]).display(),
        "a, b"
    );
    assert!(PropertyValue::Null.matches_kind(PropertyKind::Text));
    assert!(!PropertyValue::Text("1".into()).matches_kind(PropertyKind::Number));
}
#[test]
fn defaults_agree_with_serde_and_prefix_is_ascii() {
    let settings: BoardSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings, BoardSettings::default());
    assert!(settings.start_on_worktree);
    assert_eq!(settings.branch_template, "{key}-{slug}");
    assert_eq!(board().prefix, "FLE");
    let context = Context {
        id: "work".parse().unwrap(),
        name: "中文!?".into(),
        owners: vec![],
        created_at: String::new(),
    };
    assert_eq!(default_prefix(&context), "FLT");
    assert_eq!(default_statuses().len(), 5);
}
#[test]
fn patch_null_clears_while_absence_leaves_unchanged() {
    let patch: CardPatch =
        serde_json::from_str(r#"{"assignee":null,"estimate":null,"repoId":null,"agent":null}"#)
            .unwrap();
    assert_eq!(patch.agent, Some(None));
    assert_eq!(patch.assignee, Some(None));
    assert_eq!(patch.estimate, Some(None));
    assert_eq!(patch.repo_id, Some(None));
    assert_eq!(patch.parent_id, None);
    round_trip(&patch);
    round_trip(&CardPatch::default());
    assert!(CardPatch::default().is_empty());
    assert!(!patch.is_empty());
    let board_patch: BoardPatch = serde_json::from_str(r#"{"defaultRepoId":null}"#).unwrap();
    assert_eq!(board_patch.default_repo_id, Some(None));
    round_trip(&board_patch);
    round_trip(&BoardPatch::default());
}
#[test]
fn validates_references_prefix_and_calendar_date() {
    let mut board = board();
    let mut card = card(&board);
    assert!(validate_board(&board).is_ok());
    assert!(validate_card(&board, &card).is_ok());
    card.due_date = Some("2024-02-29".into());
    assert!(validate_card(&board, &card).is_ok());
    for bad in [
        "2025-02-29",
        "2026-2-03",
        "2026-13-03",
        "0000-01-01",
        "ééééé",
    ] {
        card.due_date = Some(bad.into());
        assert!(validate_card(&board, &card).is_err());
    }
    card.due_date = None;
    card.status_id = "missing".parse().unwrap();
    assert!(matches!(
        validate_card(&board, &card),
        Err(BoardError::UnknownStatus(_))
    ));
    card.status_id = "todo".parse().unwrap();
    card.labels.push("bug".parse().unwrap());
    assert!(matches!(
        validate_card(&board, &card),
        Err(BoardError::UnknownLabel(_))
    ));
    board.statuses.push(board.statuses[0].clone());
    assert!(validate_board(&board).is_err());
    board.statuses.pop();
    board.prefix = "bad".into();
    assert!(validate_board(&board).is_err());
}
#[test]
fn checks_custom_property_schema() {
    let mut board = board();
    let mut card = card(&board);
    board.properties.push(PropertySchema {
        key: "score".into(),
        name: "Score".into(),
        kind: PropertyKind::Number,
        options: vec![],
        editable: true,
        source: PropertySource::Local,
        show_on_card: true,
    });
    card.properties = BTreeMap::from([("score".into(), PropertyValue::Number(1.5))]);
    assert!(validate_card(&board, &card).is_ok());
    card.properties
        .insert("score".into(), PropertyValue::Text("1".into()));
    assert!(validate_card(&board, &card).is_err());
}
#[test]
fn orders_columns_and_summarizes_live_cards() {
    let board = board();
    let mut first = card(&board);
    first.position = 10;
    let mut second = first.clone();
    second.id = CardId::try_from("card-2").unwrap();
    second.number = 13;
    second.position = 0;
    second.dirty = true;
    let mut archived = second.clone();
    archived.archived = true;
    let mut done = first.clone();
    done.status_id = "done".parse().unwrap();
    let cards = vec![first, second, archived, done];
    let column = column_cards(&cards, &"todo".parse().unwrap());
    assert_eq!(
        column.iter().map(|c| c.number).collect::<Vec<_>>(),
        vec![13, 12]
    );
    let summary = summarize(&board, &cards, &[], "2026-09-06T12:00:00Z");
    assert_eq!(
        (summary.card_count, summary.open_count, summary.dirty_count),
        (3, 2, 1)
    );
    assert_eq!(
        first_status_in(&board, StatusCategory::Started)
            .unwrap()
            .id
            .as_str(),
        "in-progress"
    );
}
#[test]
fn comments_stamp_activity_and_cap_history() {
    let board = board();
    let mut card = card(&board);
    assert!(add_comment(&mut card, "empty".into(), None, "  ".into(), "now").is_err());
    add_comment(&mut card, "comment".into(), None, "hello".into(), "now").unwrap();
    assert_eq!(card.updated_at, "now");
    assert!(!card.dirty);
    assert_eq!(card.activity[0].kind, ActivityKind::Commented);
    for i in 0..205 {
        push_activity(
            &mut card,
            ActivityKind::Updated,
            None,
            i.to_string(),
            "later",
        );
    }
    assert_eq!(card.activity.len(), 200);
    assert_eq!(card.activity[0].message, "5");
}
#[test]
fn worktree_slug_uses_remote_keys_template_and_limits() {
    let mut board = board();
    let mut card = card(&board);
    assert_eq!(card.local_key(&board), "FLE-12");
    assert_eq!(worktree_slug(&board, &card), "fle-12-fix-login");
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "fake".into(),
        key: "EXT-9".into(),
        url: None,
        version: None,
        synced_at: String::new(),
        remote_updated_at: None,
    });
    assert_eq!(card.display_key(&board), "EXT-9");
    assert_eq!(worktree_slug(&board, &card), "ext-9-fix-login");
    card.title = "x".repeat(100);
    assert_eq!(worktree_slug(&board, &card).len(), 48);
    board.settings.branch_template = "!?".into();
    assert_eq!(worktree_slug(&board, &card), "fle-12");
}

#[test]
fn a_column_that_automates_nothing_encodes_as_it_did_before() {
    let status = board().statuses.remove(0);

    assert!(status.automation.is_none());
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        serde_json::json!({"id": "backlog", "name": "Backlog", "category": "backlog", "color": null})
    );
}

#[test]
fn a_prompt_column_action_round_trips_with_its_agent_env_and_routing() {
    let automation = ColumnAutomation {
        on_enter: Some(Action {
            kind: ActionKind::Prompt,
            instructions: "Implement {key}.".into(),
            expect: "make test passes".into(),
            agent: ColumnAgentPrefs {
                provider: Some(AgentKind::Claude),
                model: Some("opus".into()),
                effort: None,
                mode: Some(PermissionMode::FullAccess),
            },
            env: vec!["CARD={key}".into()],
        }),
        on_success: Some("in-review".parse().unwrap()),
        advance_when_unblocked: None,
    };

    assert_eq!(
        serde_json::to_value(&automation).unwrap(),
        serde_json::json!({
            "onEnter": {
                "kind": {"kind": "prompt"},
                "instructions": "Implement {key}.",
                "expect": "make test passes",
                "agent": {"provider": "claude", "model": "opus", "mode": "full_access"},
                "env": ["CARD={key}"]
            },
            "onSuccess": "in-review"
        })
    );
    assert_eq!(
        automation.on_enter.as_ref().map(|a| a.kind.word()),
        Some("run card".to_owned())
    );
    round_trip(&automation);
}

#[test]
fn a_skill_action_omits_empty_arguments_and_an_empty_agent_block() {
    let action = Action {
        kind: ActionKind::Skill {
            name: "deep-review".into(),
            args: String::new(),
        },
        instructions: String::new(),
        expect: String::new(),
        agent: ColumnAgentPrefs::default(),
        env: Vec::new(),
    };

    assert_eq!(
        serde_json::to_value(&action).unwrap(),
        serde_json::json!({"kind": {"kind": "skill", "name": "deep-review"}})
    );
    assert_eq!(action.kind.word(), "run skill deep-review");
    round_trip(&action);
}

#[test]
fn an_automation_block_that_asks_for_nothing_normalises_to_none() {
    let mut statuses = default_statuses();
    statuses[0].automation = Some(ColumnAutomation::default());
    statuses[1].automation = Some(ColumnAutomation {
        on_success: Some("done".parse().unwrap()),
        ..ColumnAutomation::default()
    });

    normalise_automation(&mut statuses);

    assert_eq!(statuses[0].automation, None);
    assert!(statuses[1].automation.is_some());
}

#[test]
fn the_board_throttle_is_one_live_run_until_a_board_sets_it() {
    let mut settings = BoardSettings::default();

    assert_eq!(settings.max_live_runs, None);
    assert_eq!(settings.max_live_runs(), 1);
    assert_eq!(
        serde_json::to_value(&settings).unwrap().get("maxLiveRuns"),
        None
    );

    settings.max_live_runs = Some(MAX_LIVE_RUNS_PER_BOARD);

    assert_eq!(settings.max_live_runs(), 8);
    round_trip(&settings);
}

#[test]
fn a_card_with_no_run_history_encodes_as_it_did_before() {
    let board = board();

    assert_eq!(
        serde_json::to_value(card(&board)).unwrap(),
        serde_json::json!({
            "id": "card-1", "boardId": "work", "number": 12, "title": "Fix login",
            "description": "", "statusId": "todo", "priority": "none", "labels": [],
            "assignee": null, "estimate": null, "dueDate": null, "parentId": null,
            "repoId": null, "worktreeId": null, "properties": {}, "comments": [],
            "activity": [], "remote": null, "conflict": null, "dirty": false,
            "archived": false, "position": 0,
            "createdAt": "2026-09-06T12:00:00Z", "updatedAt": "2026-09-06T12:00:00Z"
        })
    );
}

#[test]
fn a_card_carrying_agent_links_a_pending_run_and_a_run_round_trips() {
    let board = board();
    let mut card = card(&board);
    card.agent = Some(CardAgentPrefs {
        provider: Some(AgentKind::Codex),
        model: Some("gpt-5".into()),
        effort: Some("high".into()),
    });
    card.blocked_by = vec!["card-2".parse().unwrap()];
    card.pending_run = Some(PendingRun {
        status_id: "in-progress".parse().unwrap(),
        since: board.created_at.clone(),
    });
    card.runs = vec![CardRun {
        id: DelegationId::new(),
        thread_id: Some(ThreadId::new()),
        status_id: "in-progress".parse().unwrap(),
        action: ActionKind::Skill {
            name: "deep-review".into(),
            args: String::new(),
        },
        provider: AgentKind::Claude,
        model: Some("opus".into()),
        effort: None,
        started_at: board.created_at.clone(),
        ended_at: Some(board.updated_at.clone()),
        outcome: Some(RunOutcome::NeedsYou),
        detail: Some("reported blocked".into()),
        report_comment_id: Some("comment-1".into()),
        files_changed: 3,
        cost_usd: Some(0.42),
        tokens: Some(1_200),
    }];
    card.comments.push(Comment {
        id: "comment-1".into(),
        author: None,
        body: "report".into(),
        created_at: board.updated_at.clone(),
        remote_id: None,
        run_id: Some(card.runs[0].id),
    });

    let json = serde_json::to_value(&card).unwrap();

    assert_eq!(json["blockedBy"], serde_json::json!(["card-2"]));
    assert_eq!(json["runs"][0]["outcome"], "needs_you");
    assert_eq!(json["runs"][0]["filesChanged"], 3);
    assert_eq!(json["comments"][0]["runId"], json["runs"][0]["id"]);
    round_trip(&card);
}

#[test]
fn a_live_run_is_one_without_an_outcome_and_a_failed_start_has_no_thread() {
    let mut run = CardRun {
        id: DelegationId::new(),
        thread_id: None,
        status_id: "in-progress".parse().unwrap(),
        action: ActionKind::Prompt,
        provider: AgentKind::Claude,
        model: None,
        effort: None,
        started_at: "2026-09-06T12:00:00Z".into(),
        ended_at: None,
        outcome: None,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
    };

    assert!(run.is_live());
    assert!(run.failed_to_start());
    assert_eq!(
        serde_json::to_value(&run).unwrap(),
        serde_json::json!({
            "id": run.id, "statusId": "in-progress", "action": {"kind": "prompt"},
            "provider": "claude", "startedAt": "2026-09-06T12:00:00Z"
        })
    );

    run.thread_id = Some(ThreadId::new());
    run.outcome = Some(RunOutcome::Succeeded);

    assert!(!run.is_live());
    assert!(!run.failed_to_start());
}

#[test]
fn outcome_words_and_the_ones_that_want_a_human() {
    assert_eq!(
        [
            RunOutcome::Succeeded,
            RunOutcome::NeedsYou,
            RunOutcome::Failed,
            RunOutcome::Incomplete,
            RunOutcome::Cancelled,
        ]
        .map(RunOutcome::word),
        [
            "succeeded",
            "needs you",
            "failed",
            "incomplete",
            "cancelled"
        ]
    );
    assert_eq!(
        [
            RunOutcome::Succeeded,
            RunOutcome::NeedsYou,
            RunOutcome::Failed,
            RunOutcome::Incomplete,
            RunOutcome::Cancelled,
        ]
        .map(RunOutcome::needs_attention),
        [false, true, true, true, false]
    );
}

#[test]
fn a_board_view_omits_live_runs_until_one_is_joined_onto_it() {
    let board = board();
    let mut view = BoardView {
        cards: vec![card(&board)],
        board,
        live_runs: Vec::new(),
    };

    assert_eq!(serde_json::to_value(&view).unwrap().get("liveRuns"), None);

    view.live_runs = vec![LiveRun {
        card_id: view.cards[0].id.clone(),
        run: DelegationId::new(),
        status: DelegationStatus::Running,
        headline: Some("editing model.rs".into()),
        started: view.board.created_at.clone(),
    }];

    assert_eq!(
        serde_json::to_value(&view).unwrap()["liveRuns"][0]["status"],
        "running"
    );
    round_trip(&view);
}

#[test]
fn the_agent_and_link_fields_have_labels_every_surface_prints() {
    assert_eq!(field_label("agent"), "Agent");
    assert_eq!(field_label("blocked_by"), "Blocked by");
    assert_eq!(field_label("something_new"), "something_new");
}

#[test]
fn a_board_that_never_opted_into_automation_is_written_at_the_version_before_it() {
    let board = board();
    let cards = vec![card(&board)];

    assert_eq!(document_version(&board, &cards), BOARD_DOCUMENT_MIN_VERSION);
    assert_eq!(BOARD_DOCUMENT_MIN_VERSION, 1);
}

#[test]
fn any_one_automation_field_alone_takes_the_document_to_version_2() {
    let base = board();
    let plain = card(&base);

    let mut automated = base.clone();
    automated.statuses[1].automation = Some(ColumnAutomation {
        on_success: Some("done".parse().unwrap()),
        ..ColumnAutomation::default()
    });
    assert_eq!(
        document_version(&automated, std::slice::from_ref(&plain)),
        2
    );

    let mut throttled = base.clone();
    throttled.settings.max_live_runs = Some(2);
    assert_eq!(
        document_version(&throttled, std::slice::from_ref(&plain)),
        2
    );

    let mut with_agent = plain.clone();
    with_agent.agent = Some(CardAgentPrefs::default());
    assert_eq!(document_version(&base, &[with_agent]), 2);

    let mut linked = plain.clone();
    linked.blocked_by = vec!["card-2".parse().unwrap()];
    assert_eq!(document_version(&base, &[linked]), 2);

    let mut pending = plain.clone();
    pending.pending_run = Some(PendingRun {
        status_id: "in-progress".parse().unwrap(),
        since: base.created_at.clone(),
    });
    assert_eq!(document_version(&base, &[pending]), 2);

    let mut ran = plain.clone();
    ran.runs = vec![CardRun {
        id: DelegationId::new(),
        thread_id: Some(ThreadId::new()),
        status_id: "in-progress".parse().unwrap(),
        action: ActionKind::Prompt,
        provider: AgentKind::Claude,
        model: None,
        effort: None,
        started_at: base.created_at.clone(),
        ended_at: None,
        outcome: None,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
    }];
    assert_eq!(document_version(&base, &[ran]), 2);

    let mut reported = plain;
    reported.comments.push(Comment {
        id: "comment-1".into(),
        author: None,
        body: "report".into(),
        created_at: base.created_at.clone(),
        remote_id: None,
        run_id: Some(DelegationId::new()),
    });
    assert_eq!(document_version(&base, &[reported]), BOARD_DOCUMENT_VERSION);
}
