use super::*;

#[test]
fn adopt_pristine_schema_wholesale_with_guessed_categories() {
    let mut board = board();
    let schema = BackendSchema {
        statuses: vec![
            status("1", "Backlog", None),
            status("2", "Open", None),
            status("3", "In Review", None),
            status("4", "Resolved", None),
            status("5", "Won't do", None),
        ],
        ..BackendSchema::default()
    };
    assert!(adopt_schema(&mut board, &schema, LATER).is_empty());
    assert_eq!(
        board
            .statuses
            .iter()
            .map(|status| status.category)
            .collect::<Vec<_>>(),
        [
            StatusCategory::Unstarted,
            StatusCategory::Unstarted,
            StatusCategory::Started,
            StatusCategory::Completed,
            StatusCategory::Canceled,
        ]
    );
    assert_eq!(board.statuses[2].id.as_str(), "in-review");
    assert_eq!(
        board.sync.status_map.remote_to_local["3"].as_str(),
        "in-review"
    );
    assert_eq!(
        board.sync.status_map.local_to_remote[&board.statuses[2].id],
        "3"
    );
    assert_eq!(board.updated_at, LATER);
}

#[test]
fn adopt_disambiguates_slug_collisions_and_empty_slugs() {
    let mut board = board();
    let schema = BackendSchema {
        statuses: vec![
            status("1", "A!", None),
            status("2", "A?", None),
            status("3", "中文", None),
        ],
        ..BackendSchema::default()
    };
    adopt_schema(&mut board, &schema, NOW);
    assert_eq!(
        board
            .statuses
            .iter()
            .map(|status| status.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "a-2", "status"]
    );
    assert!(validate_board(&board).is_ok());
}

#[test]
fn adopt_custom_schema_maps_name_before_category() {
    let mut board = board();
    board.statuses[0].name = "Queue".into();
    let original = board.statuses.clone();
    let schema = BackendSchema {
        statuses: vec![
            status("q", "QUEUE", Some(StatusCategory::Completed)),
            status("s", "Running", Some(StatusCategory::Started)),
        ],
        ..BackendSchema::default()
    };
    assert!(adopt_schema(&mut board, &schema, NOW).is_empty());
    assert_eq!(board.statuses, original);
    assert_eq!(
        board.sync.status_map.remote_to_local["q"].as_str(),
        "backlog"
    );
    assert_eq!(
        board.sync.status_map.remote_to_local["s"].as_str(),
        "in-progress"
    );
}

#[test]
fn adopt_previously_synced_default_board_only_maps() {
    let mut board = board();
    board.sync.last_synced_at = Some(NOW.into());
    let schema = BackendSchema {
        statuses: vec![status("s", "Running", Some(StatusCategory::Started))],
        ..BackendSchema::default()
    };
    adopt_schema(&mut board, &schema, LATER);
    assert_eq!(board.statuses, default_statuses());
    assert_eq!(
        board.sync.status_map.remote_to_local["s"].as_str(),
        "in-progress"
    );
}

#[test]
fn re_adopting_keeps_same_named_remote_statuses_apart() {
    let mut board = board();
    let schema = BackendSchema {
        statuses: vec![
            status("s1", "Open", Some(StatusCategory::Unstarted)),
            status("s2", "Open", Some(StatusCategory::Started)),
        ],
        ..BackendSchema::default()
    };
    assert!(adopt_schema(&mut board, &schema, NOW).is_empty());
    let first = board.sync.status_map.clone();
    assert_ne!(first.remote_to_local["s1"], first.remote_to_local["s2"]);
    // A second describe must not collapse both onto the name's first match,
    // which would strip the reverse mapping and block transitions into `s2`.
    assert!(adopt_schema(&mut board, &schema, LATER).is_empty());
    assert_eq!(board.sync.status_map.remote_to_local, first.remote_to_local);
    assert_eq!(board.sync.status_map.local_to_remote, first.local_to_remote);
    assert_eq!(board.sync.status_map.local_to_remote.len(), 2);
}

#[test]
fn adopt_reports_unmapped_and_clears_stale_maps() {
    let mut board = board();
    board.statuses = vec![board.statuses[0].clone()];
    board
        .sync
        .status_map
        .remote_to_local
        .insert("old".into(), board.statuses[0].id.clone());
    let schema = BackendSchema {
        statuses: vec![status("unknown", "Mystery", None)],
        ..BackendSchema::default()
    };
    assert_eq!(adopt_schema(&mut board, &schema, NOW), ["unknown"]);
    assert!(board.sync.status_map.remote_to_local.is_empty());
    assert!(board.sync.status_map.local_to_remote.is_empty());
}

#[test]
fn adopt_empty_schema_keeps_at_least_one_status() {
    let mut board = board();
    adopt_schema(&mut board, &BackendSchema::default(), NOW);
    assert_eq!(board.statuses, default_statuses());
    // A schema that describes nothing is a degraded answer, not an authoritative empty
    // one: adopting it would erase the status map, so no transition could ever be pushed
    // again, and every backend property, whose values the service then drops from cards.
    board.properties = vec![property("remote", PropertySource::Backend)];
    board
        .sync
        .status_map
        .local_to_remote
        .insert(board.statuses[0].id.clone(), "open".into());
    let before = board.clone();
    assert!(adopt_schema(&mut board, &BackendSchema::default(), LATER).is_empty());
    assert_eq!(board, before);
}

#[test]
fn adopt_replaces_backend_properties_and_preserves_local_keys() {
    let mut board = board();
    board.properties = vec![
        property("local", PropertySource::Local),
        property("old", PropertySource::Backend),
    ];
    let mut replacement = property("local", PropertySource::Backend);
    replacement.kind = PropertyKind::Bool;
    let schema = BackendSchema {
        properties: vec![replacement, property("new", PropertySource::Local)],
        labels: vec!["Bug".into()],
        ..BackendSchema::default()
    };
    adopt_schema(&mut board, &schema, NOW);
    assert_eq!(
        board.properties,
        [
            property("local", PropertySource::Local),
            property("new", PropertySource::Backend)
        ]
    );
    assert_eq!(board.labels[0].name, "Bug");
    let before = board.clone();
    adopt_schema(&mut board, &schema, NOW);
    assert_eq!(board, before);
}

/// A schema with no statuses at all is a degraded answer on a board that already had a map.
#[test]
fn a_schema_with_no_statuses_never_disarms_an_existing_status_map() {
    let mut board = board();
    let schema = BackendSchema {
        statuses: vec![status("todo", "Todo", Some(StatusCategory::Unstarted))],
        ..BackendSchema::default()
    };
    adopt_schema(&mut board, &schema, NOW);
    let before = board.sync.status_map.clone();
    // The properties every Jira schema carries keep the "describes nothing" guard from
    // firing, so statuses alone have to be enough.
    let degraded = BackendSchema {
        statuses: vec![],
        properties: vec![property("jira.created", PropertySource::Backend)],
        ..BackendSchema::default()
    };
    assert!(adopt_schema(&mut board, &degraded, LATER).is_empty());
    assert_eq!(board.sync.status_map, before);
}

/// A status the schema stopped naming is still a column cards can be dropped into: losing
/// its remote key would make every move into it push nothing and stay dirty forever.
#[test]
fn a_column_the_schema_no_longer_names_keeps_the_remote_status_it_was_mapped_to() {
    let mut board = board();
    let schema = BackendSchema {
        statuses: vec![
            status("todo", "Todo", Some(StatusCategory::Unstarted)),
            status("blocked", "Blocked", Some(StatusCategory::Started)),
        ],
        ..BackendSchema::default()
    };
    assert!(adopt_schema(&mut board, &schema, NOW).is_empty());
    let blocked = board
        .sync
        .status_map
        .remote_to_local
        .get("blocked")
        .cloned()
        .expect("Blocked was adopted");
    // The last card leaves Blocked, so a backend that samples its statuses from live issues
    // stops naming it.
    let thinner = BackendSchema {
        statuses: vec![status("todo", "Todo", Some(StatusCategory::Unstarted))],
        ..schema.clone()
    };
    adopt_schema(&mut board, &thinner, LATER);
    assert_eq!(
        board.sync.status_map.local_to_remote.get(&blocked),
        Some(&"blocked".to_owned()),
        "the column is still on the board, so the status it pushes to must be too"
    );
}

/// A synced column keeps the status it pushes to, whatever the next sample happens to name.
///
/// The sample is taken from live issues, so an ordinary Jira workflow move — the last two
/// `In Progress` issues going to `In Review` — makes `describe` stop naming `In Progress`
/// and start naming `In Review`. Both are `Started`, so `match_status`'s category fallback
/// used to hand `In Review` the `in-progress` column: `local_to_remote` kept that first
/// binding forever, and from then on every move into the `In Progress` column transitioned
/// the issue to `In Review` — written to Jira, reported as a success, never healed.
#[test]
fn a_status_the_sample_stopped_naming_never_loses_its_column_to_another_one() {
    let mut board = board();
    let described = |names: &[(&str, StatusCategory)]| BackendSchema {
        statuses: names
            .iter()
            .map(|(name, category)| status(name, name, Some(*category)))
            .collect(),
        ..BackendSchema::default()
    };
    let first = described(&[
        ("To Do", StatusCategory::Unstarted),
        ("In Progress", StatusCategory::Started),
        ("Done", StatusCategory::Completed),
    ]);
    assert!(adopt_schema(&mut board, &first, NOW).is_empty());
    board.sync.last_synced_at = Some(NOW.into());
    let in_progress = board
        .sync
        .status_map
        .remote_to_local
        .get("In Progress")
        .cloned()
        .expect("In Progress was adopted");

    // Every issue leaves In Progress for In Review, an ordinary workflow step.
    let moved = described(&[
        ("To Do", StatusCategory::Unstarted),
        ("In Review", StatusCategory::Started),
        ("Done", StatusCategory::Completed),
    ]);
    assert_eq!(adopt_schema(&mut board, &moved, LATER), ["In Review"]);
    assert_eq!(
        board.sync.status_map.local_to_remote.get(&in_progress),
        Some(&"In Progress".to_owned()),
        "the column still pushes to the status the user bound it to"
    );
    assert!(
        !board
            .sync
            .status_map
            .remote_to_local
            .contains_key("In Review"),
        "a status with no column of its own is unmapped, so a column is made for it"
    );

    // And when In Progress comes back beside In Review, it is still its own column.
    let both = described(&[
        ("To Do", StatusCategory::Unstarted),
        ("In Review", StatusCategory::Started),
        ("In Progress", StatusCategory::Started),
        ("Done", StatusCategory::Completed),
    ]);
    adopt_schema(&mut board, &both, LATER);
    assert_eq!(
        board.sync.status_map.local_to_remote.get(&in_progress),
        Some(&"In Progress".to_owned())
    );
}
