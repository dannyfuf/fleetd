use std::{path::PathBuf, sync::Arc};

use fleet_core::{
    ids::{ContextId, RepoId},
    model::{Context, Repo, RepoHooks},
    paths::{HotMarker, hot_marker_path, slot_path},
    state::default_state,
};
use fleet_daemon::{
    adapters::clock::SystemClock,
    jobs::JobManager,
    services::pool::Pool,
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeFiles, FakeFilesCall, FakeGit, FakeShell},
};
use serde_json::json;

#[tokio::test]
async fn pool_claim_uses_the_lowest_ready_fake_slot() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    std::fs::create_dir_all(&home).unwrap_or_else(|error| panic!("{error}"));
    let worktrees_root = home.join("worktrees");
    let files = Arc::new(FakeFiles::new(
        home.join("trash"),
        vec![home.join("repos"), worktrees_root.clone()],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    config
        .update(json!({
            "reposDir": home.join("repos"),
            "worktreesDir": &worktrees_root,
            "hotPoolSize": 2,
            "hotRefreshIntervalMs": 0
        }))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
    let jobs = Arc::new(JobManager::new(&home));
    let shell = Arc::new(FakeShell::new());
    let pool = Pool::new(
        config,
        state.clone(),
        jobs,
        Arc::new(FakeGit::new(Arc::clone(&shell))),
        files.clone(),
        shell,
    );
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let repo = fixture_repo(&home.join("repos/acme/api"));
    let mut persisted = default_state();
    persisted.contexts.push(fixture_context());
    persisted.repos.push(repo.clone());
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let root = worktrees_root.join("acme/api");
    let first_marker = marker("01");
    let second_marker = marker("02");
    files.insert_text(
        hot_marker_path(slot_path(&root, 0)),
        serde_json::to_string(&first_marker).unwrap_or_else(|error| panic!("{error}")),
    );
    files.insert_text(
        hot_marker_path(slot_path(&root, 1)),
        serde_json::to_string(&second_marker).unwrap_or_else(|error| panic!("{error}")),
    );

    let claimed = pool
        .claim(repo.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("slot should be claimed"));
    let text = files
        .text(&hot_marker_path(&claimed))
        .unwrap_or_else(|| panic!("claimed marker"));
    let actual: HotMarker = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(actual.fetched_at, first_marker.fetched_at);
    assert!(files.calls().iter().any(|call| matches!(
        call,
        FakeFilesCall::Rename(source, destination)
            if source == &slot_path(&root, 0) && destination == &claimed
    )));
}

fn marker(suffix: &str) -> HotMarker {
    HotMarker {
        fetched_at: format!("2026-09-04T12:00:{suffix}Z"),
        default_branch: "main".to_owned(),
        sha: "0123456789012345678901234567890123456789".to_owned(),
        prepare_fingerprint: "0".repeat(64),
    }
}

fn fixture_context() -> Context {
    Context {
        id: ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}")),
        name: "Acme".to_owned(),
        owners: vec!["acme".to_owned()],
        created_at: "2026-09-04T00:00:00Z".to_owned(),
    }
}

fn fixture_repo(path: &std::path::Path) -> Repo {
    Repo {
        id: RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
        owner: "acme".to_owned(),
        name: "api".to_owned(),
        url: "unused".to_owned(),
        context_id: ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}")),
        default_branch: "main".to_owned(),
        path: PathBuf::from(path).to_string_lossy().into_owned(),
        cloned_at: "2026-09-04T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    }
}
