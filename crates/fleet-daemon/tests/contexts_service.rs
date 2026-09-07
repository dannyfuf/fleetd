use std::sync::Arc;

use fleet_core::{
    config::{NATIVE_LAZYGIT, WindowConfig},
    ids::{ContextId, RepoId, WorktreeId},
    model::{CloneJob, CloneStatus, Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    adapters::{
        Adapters,
        clock::SystemClock,
        files::{Files, RealFiles},
    },
    jobs::JobManager,
    server::BroadcastBus,
    services::{Services, contexts::Contexts},
    stores::{config::ConfigStore, state::StateStore},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

fn state_store(temp: &tempfile::TempDir) -> Arc<StateStore> {
    let home = temp.path().join(".fleet");
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    Arc::new(StateStore::new(home, files, Arc::new(SystemClock)))
}

#[tokio::test]
async fn contexts_create_update_and_activate() {
    let temp = tempfile::tempdir().unwrap();
    let state = state_store(&temp);
    let contexts = Contexts::new(Arc::clone(&state));
    let context = contexts
        .create("Platform / API".to_owned(), vec!["acme".to_owned()])
        .await
        .unwrap();
    assert_eq!(context.id.as_str(), "platform-api");
    contexts.set_active(Some(context.id.clone())).await.unwrap();
    let updated = contexts
        .update(
            context.id.clone(),
            Some("Platform".to_owned()),
            Some(vec!["acme-inc".to_owned()]),
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "Platform");

    contexts.delete(context.id).await.unwrap();
    let snapshot = state.load().await.unwrap();
    assert!(snapshot.contexts.is_empty());
    assert!(snapshot.active_context_id.is_none());
}

#[tokio::test]
async fn delete_cascades_every_context_resource() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join(".fleet");
    let files = Arc::new(fleet_daemon::testing::fakes::FakeFiles::new(
        home.join("trash"),
        vec![home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let mut effective = config.load().await.unwrap();
    effective.windows = vec![WindowConfig {
        name: "git".into(),
        command: NATIVE_LAZYGIT.into(),
    }];
    config.save(effective).await.unwrap();
    let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
    let context = ContextId::try_from("team").unwrap();
    let repo = RepoId::try_from("acme/api").unwrap();
    let clone = RepoId::try_from("acme/pending").unwrap();
    let worktree = WorktreeId::try_from("acme/api#feature").unwrap();
    let repo_path = home.join("repos/acme/api");
    let staging_path = home.join("repos/acme/pending.staging");
    let worktree_path = home.join("worktrees/acme/api/feature");
    for path in [&repo_path, &staging_path, &worktree_path] {
        files.create_dir_all(path).unwrap();
    }
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context.clone(),
        name: "Team".into(),
        owners: vec!["acme".into()],
        created_at: "2026-09-06T00:00:00Z".into(),
    });
    persisted.repos.push(Repo {
        id: repo.clone(),
        owner: "acme".into(),
        name: "api".into(),
        url: "https://example.invalid/acme/api".into(),
        context_id: context.clone(),
        default_branch: "main".into(),
        path: repo_path.display().to_string(),
        cloned_at: "2026-09-06T00:00:00Z".into(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree.clone(),
        repo_id: repo,
        slug: "feature".into(),
        branch: "feature".into(),
        base_ref: "origin/main".into(),
        path: worktree_path.display().to_string(),
        session: "api/feature".into(),
        host: None,
        created_at: "2026-09-06T00:00:00Z".into(),
        last_opened_at: None,
        degraded: None,
    });
    persisted.clones.push(CloneJob {
        id: clone,
        owner: "acme".into(),
        name: "pending".into(),
        url: "https://example.invalid/acme/pending".into(),
        context_id: context.clone(),
        default_branch: "main".into(),
        path: home.join("repos/acme/pending").display().to_string(),
        staging_path: staging_path.display().to_string(),
        log_path: home.join("logs/pending.log").display().to_string(),
        pid: None,
        started_at: "2026-09-06T00:00:00Z".into(),
        status: CloneStatus::Failed,
        error: Some("interrupted".into()),
    });
    state.save(persisted).await.unwrap();
    let services = Services::new_with_events(
        &home,
        config,
        state.clone(),
        Arc::new(JobManager::new(&home)),
        Adapters::system(files.clone()),
        BroadcastBus::default(),
    );
    services
        .sessions
        .ensure(Some(worktree), None, false)
        .await
        .unwrap();

    assert_eq!(
        services
            .dispatch(RequestBody::DeleteContext { id: context })
            .await
            .unwrap(),
        ResponseBody::Ack
    );

    let persisted = state.load().await.unwrap();
    assert!(persisted.contexts.is_empty());
    assert!(persisted.repos.is_empty());
    assert!(persisted.clones.is_empty());
    assert!(persisted.worktrees.is_empty());
    assert!(services.sessions.snapshot().is_empty());
    assert!(!files.exists(&repo_path));
    assert!(!files.exists(&staging_path));
    assert!(!files.exists(&worktree_path));
}

#[tokio::test]
async fn active_context_must_exist() {
    let temp = tempfile::tempdir().unwrap();
    let contexts = Contexts::new(state_store(&temp));
    let missing = ContextId::try_from("missing").unwrap();
    assert!(contexts.set_active(Some(missing)).await.is_err());
}
