use std::sync::Arc;

use fleet_core::state::default_state;
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
