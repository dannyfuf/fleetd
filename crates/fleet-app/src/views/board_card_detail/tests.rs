use super::*;
use fleet_core::{
    agents::{AgentKind, DelegationId, ThreadId},
    board::{
        Action, CardAgentPrefs, CardDraft, CardRun, ColumnAgentPrefs, ColumnAutomation, Label,
        PendingRun, RemoteLink, create_card, new_board,
    },
    ids::{ContextId, LabelId},
    model::Context,
};

fn fixture() -> (Board, Card) {
    let context = Context {
        id: ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = new_board(&context, "2026-09-06T12:00:00Z");
    board.labels.push(Label {
        id: LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}")),
        name: "Bug".into(),
        color: None,
    });
    let card = create_card(
        &mut board,
        &[],
        "card-1".parse().unwrap_or_else(|error| panic!("{error}")),
        CardDraft {
            title: "Fix login".into(),
            labels: vec![LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}"))],
            ..CardDraft::default()
        },
        "2026-09-06T12:00:00Z",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    (board, card)
}

#[test]
fn the_rows_are_the_contract_order_and_unset_values_read_as_a_dash() {
    let (board, card) = fixture();
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_ref()).collect();
    assert_eq!(
        labels,
        [
            "Status", "Priority", "Assignee", "Labels", "Estimate", "Due", "Parent", "Repo",
            "Worktree"
        ]
    );
    let assignee = &rows[2];
    assert_eq!(assignee.value.as_ref(), "\u{2013}");
    assert_eq!(assignee.tone, Tone::Muted);
    assert_eq!(rows[3].value.as_ref(), "Bug");
}

#[test]
fn every_editable_row_names_the_picker_that_edits_it() {
    let (board, card) = fixture();
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert_eq!(rows[0].target, PropertyTarget::Pick(PickerKind::Status));
    assert_eq!(rows[5].target, PropertyTarget::Pick(PickerKind::DueDate));
    assert_eq!(
        rows[6].target,
        PropertyTarget::ReadOnly,
        "parent is v1 read-only"
    );
    assert_eq!(rows[8].target, PropertyTarget::ReadOnly, "no worktree yet");
}

#[test]
fn a_linked_card_gains_the_remote_rows_and_a_worktree_target() {
    let (board, mut card) = fixture();
    card.worktree_id = Some(
        "buk/payroll#fix"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    );
    card.dirty = true;
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "PROJ-12".into(),
        url: Some("https://example.test/PROJ-12".into()),
        version: None,
        synced_at: "2026-09-06T12:00:00Z".into(),
        remote_updated_at: None,
    });
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert_eq!(rows[8].target, PropertyTarget::Worktree);
    let remote = rows
        .iter()
        .find(|row| row.label.as_ref() == "Remote")
        .unwrap_or_else(|| panic!("no remote row"));
    assert!(remote.value.contains("PROJ-12"));
    assert!(remote.value.contains("dirty"));
    assert_eq!(remote.tone, Tone::Warning);
    assert!(rows.iter().any(|row| row.label.as_ref() == "URL"));
    assert!(rows.iter().any(|row| row.label.as_ref() == "Synced"));
}

#[test]
fn a_backend_owned_field_reads_as_locked_but_keeps_its_picker() {
    let (mut board, card) = fixture();
    board.sync.readonly_fields = vec!["priority".into(), "parent_id".into()];
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert!(
        rows.iter().all(|row| !row.locked),
        "a local board declares no backend, so nothing on it is read-only"
    );

    board.backend.kind = "jira".into();
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let locked: Vec<&str> = rows
        .iter()
        .filter(|row| row.locked)
        .map(|row| row.label.as_ref())
        .collect();
    assert_eq!(locked, ["Priority", "Parent"]);
    assert_eq!(rows[1].tone, Tone::Secondary);
    assert_eq!(
        rows[1].target,
        PropertyTarget::Pick(PickerKind::Priority),
        "the row still has to answer Enter \u{2014} with the backend's own sentence"
    );
    assert!(!rows[0].locked, "status is writable on this board");
}

#[test]
fn a_property_the_schema_calls_uneditable_wears_the_same_lock() {
    let (mut board, card) = fixture();
    board.properties.push(fleet_core::board::PropertySchema {
        key: "created".into(),
        name: "Created".into(),
        kind: PropertyKind::Date,
        options: Vec::new(),
        editable: false,
        source: PropertySource::Backend,
        show_on_card: false,
    });
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let row = rows.last().unwrap_or_else(|| panic!("no property row"));
    assert!(row.locked);
    assert_eq!(row.tone, Tone::Secondary);
    assert_eq!(row.target, PropertyTarget::ReadOnly);
}

#[test]
fn a_conflict_banner_names_the_fields_that_differ() {
    let (_, mut card) = fixture();
    assert!(conflict_banner(&card, &Theme::dark()).is_none());
    card.conflict = Some(fleet_core::board::Conflict {
        detected_at: "2026-09-06T12:00:00Z".into(),
        remote: fleet_core::board::RemoteCard {
            key: "PROJ-12".into(),
            ..fleet_core::board::RemoteCard::default()
        },
        fields: vec!["title".into(), "status".into()],
    });
    assert!(conflict_banner(&card, &Theme::dark()).is_some());
}

/// `differing_fields` answers in wire names; the banner has to say what the rows say.
#[test]
fn the_conflict_banner_names_fields_the_way_the_property_rows_do() {
    assert_eq!(field_label("status_id"), "Status");
    assert_eq!(field_label("due_date"), "Due");
    assert_eq!(field_label("parent_id"), "Parent");
    assert_eq!(field_label("title"), "Title");
    // A name nobody mapped is still printed rather than dropped.
    assert_eq!(field_label("something_new"), "something_new");
}

/// The epoch second a stamp names, so a test states its clock in the daemon's own vocabulary.
#[track_caller]
fn at(stamp: &str) -> i64 {
    parse_timestamp(stamp).unwrap_or_else(|| panic!("{stamp} is a timestamp"))
}

/// A run on the card's own column, live until `outcome` says otherwise.
fn run_on(card: &Card, outcome: Option<RunOutcome>) -> CardRun {
    CardRun {
        id: DelegationId::new(),
        thread_id: Some(ThreadId::new()),
        status_id: card.status_id.clone(),
        action: ActionKind::Prompt,
        provider: AgentKind::Codex,
        model: Some("gpt-5".to_owned()),
        effort: Some("high".to_owned()),
        started_at: "2026-09-06T12:00:00Z".to_owned(),
        ended_at: outcome.is_some().then(|| "2026-09-06T12:05:00Z".to_owned()),
        outcome,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
    }
}

/// Gives the card's column an action, with `agent` as the column's own defaults.
#[track_caller]
fn automate(board: &mut Board, card: &Card, kind: ActionKind, agent: ColumnAgentPrefs) {
    let status = board
        .statuses
        .iter_mut()
        .find(|status| status.id == card.status_id)
        .unwrap_or_else(|| panic!("the card sits in a column"));
    status.automation = Some(ColumnAutomation {
        on_enter: Some(Action {
            kind,
            instructions: String::new(),
            expect: String::new(),
            agent,
            env: Vec::new(),
        }),
        ..ColumnAutomation::default()
    });
}

/// The row a label names, or a failure naming the labels there are.
#[track_caller]
fn row<'a>(rows: &'a [PropertyRow], label: &str) -> &'a PropertyRow {
    rows.iter()
        .find(|row| row.label.as_ref() == label)
        .unwrap_or_else(|| {
            let labels: Vec<&str> = rows.iter().map(|row| row.label.as_ref()).collect();
            panic!("no {label} row in {labels:?}")
        })
}

#[test]
fn a_live_run_reads_its_state_word_the_agent_and_how_long_it_has_been_going() {
    let (_, mut card) = fixture();
    let run = run_on(&card, None);
    card.runs.push(run);
    let line = run_line(
        &card,
        Some(RunMark::Working),
        Some(DelegationStatus::Running),
        at("2026-09-06T12:04:00Z"),
    )
    .unwrap_or_else(|| panic!("a card with a run has a run row"));
    assert_eq!(line.mark, Some(RunMark::Working));
    assert_eq!(line.text.as_ref(), "working 4m · codex · gpt-5 · high");
}

#[test]
fn a_live_run_takes_its_word_from_the_delegation_and_reads_as_working_without_one() {
    let (_, mut card) = fixture();
    let run = run_on(&card, None);
    card.runs.push(run);
    let word = |live| {
        run_line(&card, None, live, at("2026-09-06T12:04:00Z"))
            .unwrap_or_else(|| panic!("a card with a run has a run row"))
            .text
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(word(Some(DelegationStatus::Starting)), "starting");
    assert_eq!(word(Some(DelegationStatus::Blocked)), "blocked");
    assert_eq!(word(Some(DelegationStatus::Settling)), "settling");
    // Neither the mirror nor the view's join knows it, and the card has not ended it: a child
    // is still out there.
    assert_eq!(word(None), "working");
}

#[test]
fn an_owed_run_says_what_it_is_waiting_for_and_wins_over_a_finished_one() {
    let (_, mut card) = fixture();
    let finished = run_on(&card, Some(RunOutcome::Succeeded));
    card.runs.push(finished);
    card.pending_run = Some(PendingRun {
        status_id: card.status_id.clone(),
        since: "2026-09-06T12:10:00Z".to_owned(),
    });
    let line = run_line(
        &card,
        Some(RunMark::Pending),
        None,
        at("2026-09-06T12:12:00Z"),
    )
    .unwrap_or_else(|| panic!("an owed run has a row"));
    assert_eq!(line.text.as_ref(), "pending 2m · waiting for a slot");
}

#[test]
fn a_finished_run_states_its_outcome_tokens_and_cost_and_a_live_one_does_not() {
    let (_, mut card) = fixture();
    let mut ended = run_on(&card, Some(RunOutcome::Succeeded));
    ended.tokens = Some(12_400);
    ended.cost_usd = Some(0.42);
    card.runs.push(ended);
    // Long after the fact: a finished run is timed by its own two stamps, never by the clock.
    let line = run_line(&card, None, None, at("2026-09-07T12:00:00Z"))
        .unwrap_or_else(|| panic!("a card with a run has a run row"));
    assert_eq!(
        line.text.as_ref(),
        "succeeded 5m · codex · gpt-5 · high · 12.4k tok · $0.42"
    );

    let mut live = run_on(&card, None);
    live.tokens = Some(12_400);
    live.cost_usd = Some(0.42);
    card.runs.push(live);
    let line = run_line(&card, None, None, at("2026-09-06T12:05:00Z"))
        .unwrap_or_else(|| panic!("a card with a run has a run row"));
    assert!(!line.text.contains("tok"), "{}", line.text);
}

#[test]
fn every_outcome_is_worded_and_a_missing_part_drops_its_separator() {
    let (_, card) = fixture();
    for outcome in [
        RunOutcome::Succeeded,
        RunOutcome::NeedsYou,
        RunOutcome::Failed,
        RunOutcome::Incomplete,
        RunOutcome::Cancelled,
    ] {
        let mut card = card.clone();
        let mut run = run_on(&card, Some(outcome));
        run.model = None;
        run.effort = None;
        card.runs.push(run);
        let line = run_line(&card, None, None, at("2026-09-06T12:05:00Z"))
            .unwrap_or_else(|| panic!("a card with a run has a run row"));
        assert_eq!(line.text.as_ref(), format!("{} 5m · codex", outcome.word()));
    }
}

#[test]
fn a_card_with_neither_a_run_nor_one_owed_to_it_has_no_run_row() {
    let (_, card) = fixture();
    assert!(run_line(&card, None, None, at("2026-09-06T12:00:00Z")).is_none());
}

#[test]
fn a_report_carries_its_run_number_and_folds_at_eight_lines() {
    let (_, mut card) = fixture();
    let run = run_on(&card, Some(RunOutcome::Succeeded));
    let id = run.id;
    card.runs.push(run);
    let body = (0..9)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    card.comments.push(Comment {
        id: "report-1".to_owned(),
        author: None,
        body,
        created_at: "2026-09-06T12:05:00Z".to_owned(),
        remote_id: None,
        run_id: Some(id),
    });
    card.comments.push(Comment {
        id: "said-so".to_owned(),
        author: Some("danny".to_owned()),
        body: "line 0\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8".to_owned(),
        created_at: "2026-09-06T12:06:00Z".to_owned(),
        remote_id: None,
        run_id: None,
    });

    assert_eq!(report_run(&card, &card.comments[0]), Some(1));
    assert_eq!(
        report_run(&card, &card.comments[1]),
        None,
        "an ordinary comment keeps its author"
    );
    let mut expanded = HashSet::new();
    assert_eq!(
        folded_reports(&card, &expanded),
        vec!["report-1".to_owned()],
        "a long comment that is not a report never folds"
    );
    assert_eq!(
        head_lines(&card.comments[0].body).lines().count(),
        REPORT_COLLAPSE_LINES
    );

    expanded.insert("report-1".to_owned());
    assert!(
        folded_reports(&card, &expanded).is_empty(),
        "a report the reader opened stays open"
    );

    card.comments[0].body = "short".to_owned();
    assert!(
        folded_reports(&card, &HashSet::new()).is_empty(),
        "eight lines or fewer is the whole report"
    );
}

#[test]
fn a_column_without_an_action_adds_no_agent_rows() {
    let (board, card) = fixture();
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    for absent in ["Provider", "Model", "Effort", "Blocked by", "Blocks"] {
        assert!(
            rows.iter().all(|row| row.label.as_ref() != absent),
            "{absent} on a board nobody automated"
        );
    }
}

#[test]
fn an_inherited_agent_value_names_the_column_and_a_card_level_one_does_not() {
    let (mut board, mut card) = fixture();
    automate(
        &mut board,
        &card,
        ActionKind::Prompt,
        ColumnAgentPrefs {
            provider: Some(AgentKind::Codex),
            model: Some("gpt-5".to_owned()),
            effort: None,
            mode: None,
        },
    );
    card.agent = Some(CardAgentPrefs {
        provider: None,
        model: Some("gpt-5-mini".to_owned()),
        effort: None,
    });
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert_eq!(
        rows[1].label.as_ref(),
        "Provider",
        "the workflow rows sit under Status"
    );
    assert_eq!(row(&rows, "Provider").value.as_ref(), "codex");
    assert_eq!(
        row(&rows, "Provider").source.as_deref(),
        Some("column default")
    );
    assert_eq!(
        row(&rows, "Provider").target,
        PropertyTarget::Pick(PickerKind::Provider)
    );
    assert_eq!(row(&rows, "Model").value.as_ref(), "gpt-5-mini");
    assert!(
        row(&rows, "Model").source.is_none(),
        "the card asked for this one"
    );
    assert_eq!(row(&rows, "Effort").value.as_ref(), "\u{2013}");
    assert!(
        row(&rows, "Effort").source.is_none(),
        "nothing set it, so there is no column to inherit it from"
    );
}

#[test]
fn a_skill_column_shows_the_provider_it_will_really_run() {
    let (mut board, mut card) = fixture();
    automate(
        &mut board,
        &card,
        ActionKind::Skill {
            name: "deep-review".to_owned(),
            args: String::new(),
        },
        ColumnAgentPrefs::default(),
    );
    card.agent = Some(CardAgentPrefs {
        provider: Some(AgentKind::Codex),
        model: None,
        effort: None,
    });
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert_eq!(row(&rows, "Provider").value.as_ref(), "claude");
    assert_eq!(
        row(&rows, "Provider").source.as_deref(),
        Some("column default"),
        "a skill column ignores the card's provider, so the row says whose it is"
    );
}

/// Three cards: one finished, one canceled, and the card they both block.
fn linked() -> (Board, Vec<Card>) {
    let (mut board, first) = fixture();
    let mut cards = vec![first];
    for (index, title) in [(2, "Ship it"), (3, "Third")] {
        let card = create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: title.to_owned(),
                ..CardDraft::default()
            },
            "2026-09-06T12:00:00Z",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    let done = board.statuses[3].id.clone();
    let canceled = board.statuses[4].id.clone();
    cards[0].status_id = done;
    cards[2].status_id = canceled;
    cards[1].blocked_by = vec![cards[0].id.clone(), cards[2].id.clone()];
    (board, cards)
}

#[test]
fn a_satisfied_blocker_is_checked_and_one_nobody_can_finish_is_amber() {
    let (board, cards) = linked();
    let rows = property_rows(&board, &cards, &cards[1], 0);
    let blocked: Vec<&PropertyRow> = rows
        .iter()
        .skip_while(|row| row.label.as_ref() != "Blocked by")
        .take(2)
        .collect();
    assert!(blocked[0].value.starts_with("✓ "), "{}", blocked[0].value);
    assert!(blocked[0].value.contains("Done"), "{}", blocked[0].value);
    assert_eq!(blocked[0].tone, Tone::Default);
    assert_eq!(
        blocked[0].target,
        PropertyTarget::Pick(PickerKind::BlockedBy)
    );
    assert_eq!(
        blocked[1].label.as_ref(),
        "",
        "one label for the whole group"
    );
    assert_eq!(
        blocked[1].tone,
        Tone::Warning,
        "a canceled blocker never releases this card"
    );
    assert!(!blocked[1].value.starts_with("✓"), "{}", blocked[1].value);
}

#[test]
fn the_blocks_row_is_the_reverse_of_everyone_elses_blockers() {
    let (board, cards) = linked();
    let rows = property_rows(&board, &cards, &cards[0], 0);
    let blocks = row(&rows, "Blocks");
    assert!(
        blocks.value.contains(&cards[1].display_key(&board)),
        "{}",
        blocks.value
    );
    assert_eq!(blocks.target, PropertyTarget::Pick(PickerKind::Blocks));
    assert_eq!(
        row(&rows, "Blocked by").value.as_ref(),
        "\u{2013}",
        "a card with no blockers still gets the row once the board uses links"
    );
}

#[test]
fn a_backend_that_owns_a_field_locks_the_workflow_rows_it_owns() {
    let (mut board, cards) = linked();
    let card = cards[1].clone();
    automate(
        &mut board,
        &card,
        ActionKind::Prompt,
        ColumnAgentPrefs {
            provider: Some(AgentKind::Claude),
            ..ColumnAgentPrefs::default()
        },
    );
    board.backend.kind = "jira".into();
    board.sync.readonly_fields = vec!["agent".into(), "blocked_by".into()];
    let rows = property_rows(&board, &cards, &card, 0);
    for label in ["Provider", "Model", "Effort", "Blocked by", "Blocks"] {
        assert!(row(&rows, label).locked, "{label} is the backend's");
    }
    assert_eq!(
        row(&rows, "Provider").target,
        PropertyTarget::Pick(PickerKind::Provider),
        "a locked row still has to answer Enter"
    );
}

/// The run card leads with its state as a sentence and keeps a finished run's usage apart, so
/// it can sit at the line's right end; the whole line still reads as §11.9 words it.
#[test]
fn the_run_card_splits_its_line_into_head_facts_and_usage() {
    let (_, mut card) = fixture();
    let mut run = run_on(&card, Some(RunOutcome::Succeeded));
    run.tokens = Some(12_400);
    run.cost_usd = Some(0.31);
    card.runs.push(run);
    let line = run_line(&card, None, None, at("2026-09-06T12:09:00Z"))
        .unwrap_or_else(|| panic!("a card with a run has a run row"));
    assert!(line.head.starts_with("Succeeded"), "{}", line.head);
    assert_eq!(line.facts.as_ref(), "codex · gpt-5 · high");
    assert_eq!(line.usage.as_deref(), Some("12.4k tok · $0.31"));
    assert!(line.text.starts_with("succeeded "), "{}", line.text);
    assert!(line.text.ends_with("12.4k tok · $0.31"), "{}", line.text);
}

/// A linked card's Remote row opens the issue, as `x` does; the rows beside it state facts.
#[test]
fn the_remote_row_opens_the_issue_and_a_locked_row_takes_no_click() {
    let (board, mut card) = fixture();
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "PROJ-12".into(),
        url: Some("https://example.test/PROJ-12".into()),
        version: None,
        synced_at: "2026-09-06T12:00:00Z".into(),
        remote_updated_at: None,
    });
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let remote = row(&rows, "Remote");
    assert_eq!(remote.target, PropertyTarget::Remote);
    assert!(is_clickable(remote));
    assert!(!is_clickable(row(&rows, "Synced")));
    assert!(is_clickable(row(&rows, "Status")));
    let mut locked = row(&rows, "Status").clone();
    locked.locked = true;
    assert!(!is_clickable(&locked));
}
