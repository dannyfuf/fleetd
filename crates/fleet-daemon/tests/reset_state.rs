use std::sync::Arc;

use fleet_core::{
    model::{Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

#[tokio::test]
async fn reset_state_dispatch_reports_the_preserved_archive() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
    std::fs::write(state.path(), "not json").unwrap();
    assert!(state.load().await.is_err());
    let services = Services::new(
        home,
        Arc::new(ConfigStore::new(home, files.clone())),
        state.clone(),
        Arc::new(JobManager::new(home)),
        Adapters::system(files),
    );

    let response = services.dispatch(RequestBody::ResetState).await.unwrap();
    let ResponseBody::Path {
        path: archive,
        host: None,
    } = response
    else {
        panic!("reset returned an unexpected response");
    };

    assert!(std::path::Path::new(&archive).exists());
    assert_eq!(state.load().await.unwrap(), default_state());
}

#[tokio::test]
async fn startup_moves_persisted_remote_worktrees_to_the_legacy_archive() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: "acme".parse().unwrap(),
        name: "Acme".into(),
        owners: vec!["acme".into()],
        created_at: "2026-09-08T00:00:00Z".into(),
    });
    persisted.repos.push(Repo {
        id: "acme/api".parse().unwrap(),
        owner: "acme".into(),
        name: "api".into(),
        url: "https://example.invalid/acme/api".into(),
        context_id: "acme".parse().unwrap(),
        default_branch: "main".into(),
        path: home.join("repos/acme/api").display().to_string(),
        cloned_at: "2026-09-08T00:00:00Z".into(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: "acme/api#remote".parse().unwrap(),
        repo_id: "acme/api".parse().unwrap(),
        slug: "remote".into(),
        branch: "remote".into(),
        base_ref: "origin/main".into(),
        path: "/remote/acme/api/remote".into(),
        session: "api/remote".into(),
        host: Some("dev-box".parse().unwrap()),
        created_at: "2026-09-08T00:00:00Z".into(),
        last_opened_at: None,
        degraded: None,
    });
    state.save(persisted).await.unwrap();
    let services = Services::new(
        home,
        Arc::new(ConfigStore::new(home, files.clone())),
        state.clone(),
        Arc::new(JobManager::new(home)),
        Adapters::system(files),
    );

    services.worktrees.recover_startup().await.unwrap();

    assert!(state.load().await.unwrap().worktrees.is_empty());
    let archive = services.worktrees.legacy_remote_records().await.unwrap();
    assert_eq!(archive.worktrees.len(), 1);
    assert_eq!(
        archive.worktrees[0].host.as_ref().unwrap().as_str(),
        "dev-box"
    );
}
