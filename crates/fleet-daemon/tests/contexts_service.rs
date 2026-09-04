use std::sync::Arc;

use fleet_core::{
    ids::{ContextId, RepoId, WorktreeId},
    model::{Repo, RepoHooks, Worktree},
};
use fleet_daemon::{
    adapters::{clock::SystemClock, files::RealFiles},
    services::contexts::Contexts,
    stores::state::StateStore,
};

fn state_store(temp: &tempfile::TempDir) -> Arc<StateStore> {
    let home = temp.path().join(".fleet");
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    Arc::new(StateStore::new(home, files, Arc::new(SystemClock)))
}

#[tokio::test]
async fn contexts_create_update_activate_and_delete_descendants() {
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

    let context_id = context.id.clone();
    state
        .transaction(move |state| {
            let repo_id = RepoId::try_from("acme/api").unwrap();
            state.repos.push(Repo {
                id: repo_id.clone(),
                owner: "acme".to_owned(),
                name: "api".to_owned(),
                url: "git@example/acme/api".to_owned(),
                context_id,
                default_branch: "main".to_owned(),
                path: "/tmp/api".to_owned(),
                cloned_at: "2026-09-04T00:00:00Z".to_owned(),
                hooks: RepoHooks::default(),
            });
            state.worktrees.push(Worktree {
                id: WorktreeId::try_from("acme/api#feature").unwrap(),
                repo_id,
                slug: "feature".to_owned(),
                branch: "feature".to_owned(),
                base_ref: "origin/main".to_owned(),
                path: "/tmp/feature".to_owned(),
                session: "api/feature".to_owned(),
                host: None,
                created_at: "2026-09-04T00:00:00Z".to_owned(),
                last_opened_at: None,
                degraded: None,
            });
            Ok(())
        })
        .await
        .unwrap();

    contexts.delete(context.id).await.unwrap();
    let snapshot = state.load().await.unwrap();
    assert!(snapshot.contexts.is_empty());
    assert!(snapshot.repos.is_empty());
    assert!(snapshot.worktrees.is_empty());
    assert!(snapshot.active_context_id.is_none());
}

#[tokio::test]
async fn active_context_must_exist() {
    let temp = tempfile::tempdir().unwrap();
    let contexts = Contexts::new(state_store(&temp));
    let missing = ContextId::try_from("missing").unwrap();
    assert!(contexts.set_active(Some(missing)).await.is_err());
}
