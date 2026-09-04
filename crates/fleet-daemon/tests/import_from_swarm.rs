use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use fleet_daemon::{
    DaemonResult,
    jobs::JobManager,
    services::import::{Import, ImportNotifier},
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeFiles, FixedClock},
};
use fleet_proto::job::{JobRecord, JobStatus};

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
            "worktreesDir": legacy_worktrees
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
    let imported_state = state.load().await.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(imported_state.contexts.len(), 1);
    assert_eq!(imported_state.repos.len(), 1);
    assert_eq!(imported_state.worktrees.len(), 1);
    assert_eq!(notifier.0.load(Ordering::SeqCst), 1);
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
