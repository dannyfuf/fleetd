use chrono::{TimeZone, Utc};
use fleet_core::{
    ids::{BoardId, ScheduleId},
    schedule::{Cadence, Schedule, ScheduleAgent},
};

use crate::testing::fakes::{FakeFiles, FixedClock};

use super::*;

const EPOCH_MILLIS: i64 = 1_700_000_000_123;

fn fixture() -> (ScheduleStore, Arc<FakeFiles>) {
    let home = FleetHome::new("/home/user/.fleet");
    let files = Arc::new(FakeFiles::new(home.root().join("trash"), Vec::new()));
    let now = Utc
        .timestamp_millis_opt(EPOCH_MILLIS)
        .single()
        .expect("valid timestamp");
    let store = ScheduleStore::new(
        &home,
        Arc::clone(&files) as Arc<dyn Files>,
        Arc::new(FixedClock::new(now)),
    );
    (store, files)
}

fn schedule(id: &str) -> Schedule {
    Schedule {
        id: ScheduleId::try_from(id).expect("valid schedule id"),
        board_id: BoardId::try_from("reviews-team").expect("valid board id"),
        name: "GitHub reviews".to_owned(),
        prompt: "List my review requests.".to_owned(),
        cadence: Cadence::Every { minutes: 15 },
        agent: ScheduleAgent::default(),
        enabled: true,
        timeout_minutes: 20,
        created_at: "2026-09-22T10:00:00Z".to_owned(),
        updated_at: "2026-09-22T10:00:00Z".to_owned(),
        runs: Vec::new(),
        next_run_at: None,
    }
}

fn broken_path(store: &ScheduleStore) -> PathBuf {
    store
        .path()
        .with_file_name(format!("schedules.json.broken-{EPOCH_MILLIS}"))
}

#[tokio::test]
async fn a_missing_file_is_an_empty_document() {
    let (store, files) = fixture();

    let document = store.load().await.expect("load");

    assert_eq!(document, SchedulesDocument::default());
    assert_eq!(document.version, SCHEDULES_DOCUMENT_VERSION);
    assert!(document.schedules.is_empty());
    assert!(!files.exists(store.path()));
}

#[tokio::test]
async fn a_saved_schedule_round_trips() {
    let (store, files) = fixture();

    let returned = store
        .transaction(|document| {
            document.schedules.push(schedule("sch-0a1b2c3d"));
            Ok(document.schedules.len())
        })
        .await
        .expect("transaction");

    assert_eq!(returned, 1);
    let loaded = store.load().await.expect("reload");
    assert_eq!(loaded.version, SCHEDULES_DOCUMENT_VERSION);
    assert_eq!(loaded.schedules, [schedule("sch-0a1b2c3d")]);
    let text = files.text(store.path()).expect("written file");
    assert!(text.ends_with('\n'));
    assert!(text.contains("\"boardId\": \"reviews-team\""));
}

#[tokio::test]
async fn an_invalid_schedule_refuses_the_save_and_leaves_the_file_unchanged() {
    let (store, files) = fixture();
    store
        .transaction(|document| {
            document.schedules.push(schedule("sch-0a1b2c3d"));
            Ok(())
        })
        .await
        .expect("initial save");
    let before = files.text(store.path()).expect("saved file");

    let refused = store
        .transaction(|document| {
            let mut invalid = schedule("sch-11111111");
            invalid.cadence = Cadence::Every { minutes: 1 };
            document.schedules.push(invalid);
            Ok(())
        })
        .await;

    assert!(
        matches!(
            &refused,
            Err(DaemonError::Validation(message))
                if message == "invalid cadence: every must be between 5 and 1440 minutes"
        ),
        "{refused:?}"
    );
    let duplicate = store
        .transaction(|document| {
            document.schedules.push(schedule("sch-0a1b2c3d"));
            Ok(())
        })
        .await;
    assert!(matches!(duplicate, Err(DaemonError::Validation(_))));
    let failed: DaemonResult<()> = store
        .transaction(|document| {
            document.schedules.clear();
            Err(DaemonError::Conflict("operation failed".to_owned()))
        })
        .await;
    assert!(matches!(failed, Err(DaemonError::Conflict(_))));
    assert_eq!(files.text(store.path()), Some(before));
}

#[tokio::test]
async fn a_corrupt_file_is_quarantined_and_read_as_empty() {
    let (store, files) = fixture();
    files.insert_text(store.path(), "not json");

    let document = store.load().await.expect("load after quarantine");

    assert_eq!(document, SchedulesDocument::default());
    assert!(!files.exists(store.path()));
    assert_eq!(
        files.text(&broken_path(&store)).as_deref(),
        Some("not json")
    );

    store
        .transaction(|document| {
            document.schedules.push(schedule("sch-0a1b2c3d"));
            Ok(())
        })
        .await
        .expect("save after quarantine");
    assert_eq!(
        store.load().await.expect("reload").schedules,
        [schedule("sch-0a1b2c3d")]
    );
    assert_eq!(
        files.text(&broken_path(&store)).as_deref(),
        Some("not json")
    );
}

/// One schedule this build refuses costs that schedule, not the document: it is set aside in a
/// copy of the file as it was, and every other schedule is kept.
#[tokio::test]
async fn a_refused_schedule_is_set_aside_and_the_rest_kept() {
    let (store, files) = fixture();
    let mut invalid = schedule("sch-0a1b2c3d");
    invalid.name = String::new();
    let text = serde_json::to_string(&SchedulesDocument {
        version: SCHEDULES_DOCUMENT_VERSION,
        schedules: vec![invalid, schedule("sch-1a2b3c4d")],
    })
    .expect("serialize");
    files.insert_text(store.path(), text.clone());

    let document = store.load().await.expect("load after setting one aside");

    assert_eq!(document.schedules, [schedule("sch-1a2b3c4d")]);
    assert_eq!(files.text(&broken_path(&store)), Some(text));
    let written: SchedulesDocument =
        serde_json::from_str(&files.text(store.path()).expect("the repaired document"))
            .expect("parse");
    assert_eq!(written.schedules, [schedule("sch-1a2b3c4d")]);
}

/// A document written before blank `model` and `effort` were refused loads as it meant:
/// the provider's defaults.
#[tokio::test]
async fn a_stored_blank_model_and_effort_load_as_the_defaults() {
    let (store, files) = fixture();
    let mut value = serde_json::to_value(SchedulesDocument {
        version: SCHEDULES_DOCUMENT_VERSION,
        schedules: vec![schedule("sch-0a1b2c3d")],
    })
    .expect("serialize");
    value["schedules"][0]["agent"]["model"] = serde_json::json!("");
    value["schedules"][0]["agent"]["effort"] = serde_json::json!("  ");
    files.insert_text(store.path(), value.to_string());

    let document = store.load().await.expect("load");

    assert_eq!(document.schedules, [schedule("sch-0a1b2c3d")]);
    assert!(!files.exists(&broken_path(&store)));
}

#[tokio::test]
async fn a_future_version_is_refused_and_left_in_place() {
    let (store, files) = fixture();
    let text = format!(
        "{{\"version\": {}, \"schedules\": [{{\"shape\": \"from the future\"}}]}}\n",
        SCHEDULES_DOCUMENT_VERSION + 1
    );
    files.insert_text(store.path(), text.clone());

    let loaded = store.load().await;
    let written = store
        .transaction(|document| {
            document.schedules.clear();
            Ok(())
        })
        .await;

    assert!(
        matches!(&loaded, Err(DaemonError::Unsupported(message)) if message.contains("version 2")),
        "{loaded:?}"
    );
    assert!(matches!(written, Err(DaemonError::Unsupported(_))));
    assert_eq!(files.text(store.path()), Some(text));
    assert!(!files.exists(&broken_path(&store)));
}
