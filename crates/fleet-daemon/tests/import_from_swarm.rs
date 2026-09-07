use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use fleet_daemon::{
    DaemonError, DaemonResult,
    adapters::{Adapters, files::Files},
    jobs::JobManager,
    server::BroadcastBus,
    services::{
        Services,
        import::{Import, ImportNotifier},
    },
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeFiles, FakeFilesCall, FixedClock},
};
use fleet_proto::job::{JobRecord, JobStatus};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct CountingNotifier(AtomicUsize);

#[async_trait]
impl ImportNotifier for CountingNotifier {
    async fn snapshot_changed(&self) -> DaemonResult<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn existing_fleet_state_is_rejected_before_a_job_is_enqueued() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let fleet_home = temp.path().join(".fleet");
    let swarm_home = temp.path().join(".swarm");
    let files = Arc::new(FakeFiles::new(
        fleet_home.join("trash"),
        vec![fleet_home.join("repos"), fleet_home.join("worktrees")],
    ));
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let config = Arc::new(ConfigStore::new(&fleet_home, files.clone()));
    let state = Arc::new(StateStore::new(&fleet_home, files.clone(), clock.clone()));
    state
        .save(fleet_core::state::default_state())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let jobs = Arc::new(JobManager::with_clock(&fleet_home, clock));
    let importer = Import::new(&fleet_home, swarm_home, config, state, jobs.clone(), files);

    let error = importer.start().await.unwrap_err();

    assert!(matches!(error, DaemonError::Conflict(_)));
    assert!(
        jobs.list().is_empty(),
        "a rejected import is not a failed job"
    );
}

#[tokio::test]
async fn import_validates_and_preserves_legacy_clone_directories() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let user_home = temp.path().join("user");
    let fleet_home = user_home.join(".fleet");
    let swarm_home = user_home.join(".swarm");
    let files = Arc::new(FakeFiles::new(
        fleet_home.join("trash"),
        vec![fleet_home.join("repos"), fleet_home.join("worktrees")],
    ));
    let legacy_repos = user_home.join("legacy-repos");
    let legacy_worktrees = user_home.join("legacy-worktrees");
    files.insert_text(
        swarm_home.join("config.json"),
        serde_json::json!({
            "version": 1,
            "reposDir": legacy_repos,
            "worktreesDir": legacy_worktrees,
            "windows": [
                {"name": "nvim", "command": "nvim ."},
                {"name": "cc", "command": "claude"},
                {"name": "lg", "command": "lazygit"},
                {"name": "lg2", "command": "lazygit -ucf ~/.lg.yml"},
                {"name": "own", "command": "my-lazygit"}
            ]
        })
        .to_string(),
    );
    files.insert_text(
        swarm_home.join("state.json"),
        serde_json::json!({
            "version": 1,
            "contexts": [{
                "id": "acme",
                "name": "Acme",
                "owners": ["acme"],
                "createdAt": "2026-09-04T00:00:00Z"
            }],
            "repos": [{
                "id": "acme/api",
                "owner": "acme",
                "name": "api",
                "url": "git@github.com:acme/api.git",
                "contextId": "acme",
                "defaultBranch": "main",
                "path": legacy_repos.join("acme/api"),
                "clonedAt": "2026-09-04T00:00:00Z",
                "hooks": {"prepare": [], "postCreate": []}
            }],
            "clones": [],
            "worktrees": [{
                "id": "acme/api#feature",
                "repoId": "acme/api",
                "slug": "feature",
                "branch": "feature",
                "baseRef": "origin/main",
                "path": legacy_worktrees.join("acme/api/feature"),
                "session": "api/feature",
                "createdAt": "2026-09-04T00:00:00Z"
            }],
            "activeContextId": "acme"
        })
        .to_string(),
    );

    let config = Arc::new(ConfigStore::new(&fleet_home, files.clone()));
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let state = Arc::new(StateStore::new(&fleet_home, files.clone(), clock.clone()));
    let jobs = Arc::new(JobManager::with_clock(&fleet_home, clock));
    let notifier = Arc::new(CountingNotifier::default());
    let importer = Import::new(
        &fleet_home,
        &swarm_home,
        config.clone(),
        state.clone(),
        jobs.clone(),
        files,
    )
    .with_notifier(notifier.clone());

    let record = importer
        .start()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let finished = wait_finished(&jobs, &record).await;
    assert_eq!(finished.status, JobStatus::Succeeded);
    let imported_config = config
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        imported_config.repos_dir,
        legacy_repos.display().to_string()
    );
    assert_eq!(
        imported_config.worktrees_dir,
        legacy_worktrees.display().to_string()
    );
    // A swarm `lazygit` window means "the git UI lives in this tab", and Fleet has its own.
    // Names and positions — what `ctrl-s <n>` counts — are untouched, and a command that only
    // looks like lazygit is left alone.
    assert_eq!(
        imported_config
            .windows
            .iter()
            .map(|window| (window.name.as_str(), window.command.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("nvim", "nvim ."),
            // The `{agent}` upgrade of `normalize_legacy_agent_window` still applies first.
            ("cc", "{agent}"),
            ("lg", "fleet://lazygit"),
            ("lg2", "fleet://lazygit"),
            ("own", "my-lazygit"),
        ]
    );
    let imported_state = state.load().await.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(imported_state.contexts.len(), 1);
    assert_eq!(imported_state.repos.len(), 1);
    assert_eq!(imported_state.worktrees.len(), 1);
    assert_eq!(notifier.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn partial_commit_recovers_both_or_neither() {
    let temp = tempfile::tempdir().unwrap();
    let fleet_home = temp.path().join(".fleet");
    let swarm_home = temp.path().join(".swarm");
    let files = Arc::new(FakeFiles::new(
        fleet_home.join("trash"),
        vec![fleet_home.join("repos"), fleet_home.join("worktrees")],
    ));
    files.insert_text(
        swarm_home.join("config.json"),
        serde_json::json!({"version": 1, "hotPoolSize": 3}).to_string(),
    );
    let imported_state = serde_json::from_value::<fleet_core::state::State>(serde_json::json!({
        "version": 1,
        "contexts": [{
            "id": "team",
            "name": "Team",
            "owners": [],
            "createdAt": "2026-09-06T00:00:00Z"
        }],
        "repos": [],
        "clones": [],
        "worktrees": [],
        "activeContextId": "team"
    }))
    .unwrap();
    files.insert_text(
        swarm_home.join("state.json"),
        serde_json::to_string(&imported_state).unwrap(),
    );
    let config = Arc::new(ConfigStore::new(&fleet_home, files.clone()));
    config.load().await.unwrap();
    let original_config = files.text(config.path()).unwrap();
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let state = Arc::new(StateStore::new(&fleet_home, files.clone(), clock.clone()));
    let jobs = Arc::new(JobManager::with_clock(&fleet_home, clock));
    let importer = Import::new(
        &fleet_home,
        &swarm_home,
        config.clone(),
        state.clone(),
        jobs.clone(),
        files.clone(),
    );
    let mut state_text = serde_json::to_string_pretty(&imported_state).unwrap();
    state_text.push('\n');
    files.fail_next(
        FakeFilesCall::Write(state.path().to_path_buf(), state_text),
        io::ErrorKind::Other,
    );

    let failed = importer.start().await.unwrap();
    assert!(matches!(
        wait_finished(&jobs, &failed).await.status,
        JobStatus::Failed { .. }
    ));
    assert_eq!(
        files.text(config.path()).as_deref(),
        Some(original_config.as_str())
    );
    assert!(!files.exists(state.path()));
    assert!(!files.exists(&fleet_home.join("import-transaction.json")));

    let retried = importer.start().await.unwrap();
    assert_eq!(
        wait_finished(&jobs, &retried).await.status,
        JobStatus::Succeeded
    );
    assert_eq!(config.load().await.unwrap().hot_pool_size, 3);
    assert_eq!(state.load().await.unwrap(), imported_state);
}

#[tokio::test]
async fn import_rearms_runtime_policy() {
    let temp = tempfile::tempdir().unwrap();
    let fleet_home = temp.path().join(".fleet");
    let swarm_home = temp.path().join(".swarm");
    let old_repos = fleet_home.join("repos");
    let old_worktrees = fleet_home.join("worktrees");
    let new_repos = temp.path().join("imported-repos");
    let new_worktrees = temp.path().join("imported-worktrees");
    let files = Arc::new(FakeFiles::new(
        fleet_home.join("trash"),
        vec![old_repos.clone(), old_worktrees],
    ));
    files.insert_text(
        swarm_home.join("config.json"),
        serde_json::json!({
            "version": 1,
            "reposDir": new_repos,
            "worktreesDir": new_worktrees,
            "jobs": {"keepFinishedFor": 0}
        })
        .to_string(),
    );
    files.insert_text(
        swarm_home.join("state.json"),
        serde_json::to_string(&fleet_core::state::default_state()).unwrap(),
    );
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let config = Arc::new(ConfigStore::new(&fleet_home, files.clone()));
    let state = Arc::new(StateStore::new(&fleet_home, files.clone(), clock.clone()));
    let jobs = Arc::new(JobManager::with_clock(&fleet_home, clock.clone()));
    let stale = jobs.submit(
        fleet_proto::job::JobKind::Custom("stale".into()),
        "old-policy",
        "Old completed job",
        false,
        false,
        |_| async { Ok(()) },
    );
    jobs.wait(&stale).await.unwrap();
    clock.set(chrono::Utc::now() + chrono::Duration::seconds(1));
    let importer = Import::new(
        &fleet_home,
        &swarm_home,
        config,
        state,
        jobs.clone(),
        files.clone(),
    );

    let record = importer.start().await.unwrap();
    assert_eq!(
        wait_finished(&jobs, &record).await.status,
        JobStatus::Succeeded
    );

    assert!(jobs.record(&stale).is_none());
    assert!(
        files
            .guard_strict_descendant(&new_repos.join("owner/repo"))
            .is_err()
    );
    files.create_dir_all(&new_repos.join("owner")).unwrap();
    files
        .create_dir_all(&new_worktrees.join("owner/repo"))
        .unwrap();
    assert!(
        files
            .guard_strict_descendant(&new_repos.join("owner/repo"))
            .is_ok()
    );
    assert!(
        files
            .guard_strict_descendant(&new_worktrees.join("owner/repo/main"))
            .is_ok()
    );
    assert!(
        files
            .guard_strict_descendant(&old_repos.join("owner/repo"))
            .is_err()
    );
}

#[tokio::test]
async fn committed_import_recovery_never_rolls_config_back_after_state_changes() {
    let temp = tempfile::tempdir().unwrap();
    let fleet_home = temp.path().join(".fleet");
    let swarm_home = temp.path().join(".swarm");
    let files = Arc::new(FakeFiles::new(
        fleet_home.join("trash"),
        vec![fleet_home.join("repos"), fleet_home.join("worktrees")],
    ));
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let config = Arc::new(ConfigStore::new(&fleet_home, files.clone()));
    let original_config = config.load().await.unwrap();
    let original_text = files.text(config.path()).unwrap();
    let mut imported_config = original_config;
    imported_config.hot_pool_size = 7;
    let state = Arc::new(StateStore::new(&fleet_home, files.clone(), clock.clone()));
    let mut imported_state = fleet_core::state::default_state();
    imported_state.contexts.push(fleet_core::model::Context {
        id: "team".parse().unwrap(),
        name: "Team".into(),
        owners: Vec::new(),
        created_at: "2026-09-06T00:00:00Z".into(),
    });
    state.save(imported_state.clone()).await.unwrap();
    state
        .transaction(|state| {
            state.contexts[0].name = "Changed after restart".into();
            Ok(())
        })
        .await
        .unwrap();
    files.insert_text(
        fleet_home.join("import-transaction.json"),
        serde_json::json!({
            "originalConfig": original_text,
            "importedConfig": imported_config,
            "importedState": imported_state,
            "stateCommitted": true
        })
        .to_string(),
    );
    let jobs = Arc::new(JobManager::with_clock(&fleet_home, clock));
    let importer = Import::new(
        &fleet_home,
        swarm_home,
        config.clone(),
        state.clone(),
        jobs,
        files.clone(),
    );

    assert!(matches!(
        importer.start().await,
        Err(DaemonError::Conflict(_))
    ));

    assert_eq!(config.load().await.unwrap().hot_pool_size, 7);
    assert_eq!(
        state.load().await.unwrap().contexts[0].name,
        "Changed after restart"
    );
    assert!(!files.exists(&fleet_home.join("import-transaction.json")));
}

#[tokio::test]
async fn startup_replays_a_half_applied_import_without_an_import_request() {
    let temp = tempfile::tempdir().unwrap();
    let fleet_home = temp.path().join(".fleet");
    let files = Arc::new(FakeFiles::new(
        fleet_home.join("trash"),
        vec![fleet_home.join("repos"), fleet_home.join("worktrees")],
    ));
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let config = Arc::new(ConfigStore::new(&fleet_home, files.clone()));
    let original_config = config.load().await.unwrap();
    let original_text = files.text(config.path()).unwrap();
    let mut imported_config = original_config.clone();
    imported_config.hot_pool_size = 9;
    config.save(imported_config.clone()).await.unwrap();
    let state = Arc::new(StateStore::new(&fleet_home, files.clone(), clock));
    let imported_state = serde_json::from_value::<fleet_core::state::State>(serde_json::json!({
        "version": 1,
        "contexts": [{
            "id": "team", "name": "Team", "owners": [],
            "createdAt": "2026-09-06T00:00:00Z"
        }],
        "repos": [], "clones": [], "worktrees": [], "activeContextId": "team"
    }))
    .unwrap();
    state
        .save(fleet_core::state::default_state())
        .await
        .unwrap();
    files.insert_text(
        fleet_home.join("import-transaction.json"),
        serde_json::json!({
            "originalConfig": original_text,
            "importedConfig": imported_config,
            "importedState": imported_state,
            "stateCommitted": false
        })
        .to_string(),
    );
    let services = Arc::new(Services::new(
        &fleet_home,
        config.clone(),
        state.clone(),
        Arc::new(JobManager::new(&fleet_home)),
        Adapters::system(files.clone()),
    ));
    let shutdown = CancellationToken::new();
    let tasks = services
        .start_periodic_tasks(BroadcastBus::default(), shutdown.clone())
        .await
        .unwrap();
    shutdown.cancel();
    tasks.join().await;

    assert_eq!(config.load().await.unwrap(), original_config);
    assert_eq!(
        state.load().await.unwrap(),
        fleet_core::state::default_state()
    );
    assert!(!files.exists(&fleet_home.join("import-transaction.json")));
}

async fn wait_finished(jobs: &JobManager, initial: &JobRecord) -> JobRecord {
    for _ in 0..200 {
        if let Some(record) = jobs.list().into_iter().find(|job| job.id == initial.id)
            && !matches!(
                record.status,
                JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
            )
        {
            return record;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("import job did not finish")
}
