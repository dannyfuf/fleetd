use std::{sync::Arc, time::Duration};

use fleet_core::{
    config::WindowConfig,
    ids::{ContextId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    adapters::{clock::SystemClock, files::RealFiles},
    machines::{
        CommandMachine, LinkOptions, MachineProvider, Machines, RemoteEndpoint, RemoteLink,
    },
    server::BroadcastBus,
    services::{mirror::Mirror, router::Router},
    stores::{config::ConfigStore, state::StateStore},
};
use fleet_proto::{event::Event, request::RequestBody, response::ResponseBody};

mod infra;

#[tokio::test]
async fn remote_terminal_frames_arrive_under_the_local_terminal_id() {
    let _ = infra::DaemonProcess::wait;
    let temp = tempfile::tempdir().expect("remote home");
    let home = temp.path();
    let worktree = prepare_remote_worktree(home).await;
    let remote = infra::RemoteDaemon::start(home);
    infra::assert_remote_contract(&remote);
    let machine: Arc<CommandMachine> = Arc::new(remote.machine);
    let provider: Arc<dyn MachineProvider> = machine;
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: Duration::from_millis(10),
            backoff_max: Duration::from_millis(50),
            hello_timeout: Duration::from_secs(2),
        },
    );
    link.connect().await.expect("connect remote link");

    let machines = Arc::new(Machines::from_config(&fleet_core::config::default_config(
        home.join("local"),
    )));
    machines.install_endpoint(link.host().clone(), link.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let host = link.host().clone();
    router.ids.register_worktree(&host, worktree.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    let response = router
        .forward(
            &host,
            RequestBody::EnsureSession {
                worktree: Some(worktree),
                agent: None,
                sleep_previous: false,
            },
        )
        .await
        .expect("ensure remote session");
    let ResponseBody::Session(session) = response else {
        panic!("expected remote session");
    };
    let local_terminal = session.terminals[0].id;
    router
        .forward(
            &host,
            RequestBody::AttachTerminal {
                terminal: local_terminal,
                cols: 80,
                rows: 24,
            },
        )
        .await
        .expect("attach remote terminal");
    router
        .forward(
            &host,
            RequestBody::TerminalInput {
                terminal: local_terminal,
                bytes: b"echo hi\r".to_vec(),
            },
        )
        .await
        .expect("type into remote terminal");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Event::TerminalFrame(frame) = receiver.recv().await.expect("terminal event")
                && frame.terminal == local_terminal
                && frame
                    .rows_changed
                    .iter()
                    .map(|row| {
                        row.cells
                            .iter()
                            .map(|cell| cell.text.as_str())
                            .collect::<String>()
                    })
                    .any(|row| row.contains("hi"))
            {
                break;
            }
        }
    })
    .await
    .expect("remote echo frame");

    // The remote terminal's PTY lives in a holder that outlives its daemon on purpose, so the
    // test has to end it: killing the daemon process alone would leave a shell behind.
    router
        .forward(
            &host,
            RequestBody::KillSession {
                session: session.id,
            },
        )
        .await
        .expect("stop the remote session");
}

async fn prepare_remote_worktree(home: &std::path::Path) -> WorktreeId {
    let repos = home.join("repos");
    let worktrees = home.join("worktrees");
    let worktree_path = worktrees.join("owner/repo/feature");
    std::fs::create_dir_all(&repos).expect("repos directory");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory");
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [repos.clone(), worktrees],
    ));
    let config = ConfigStore::new(home, files.clone());
    let mut effective = config.load().await.expect("remote config");
    effective.windows = vec![WindowConfig {
        name: "shell".to_owned(),
        command: "exec /bin/sh".to_owned(),
    }];
    config.save(effective).await.expect("save remote config");

    let state = StateStore::new(home, files, Arc::new(SystemClock));
    let context = ContextId::try_from("team").expect("context id");
    let repo = RepoId::try_from("owner/repo").expect("repo id");
    let worktree = WorktreeId::try_from("owner/repo#feature").expect("worktree id");
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context.clone(),
        name: "Team".to_owned(),
        owners: vec!["owner".to_owned()],
        created_at: "2026-09-08T00:00:00Z".to_owned(),
    });
    persisted.repos.push(Repo {
        id: repo.clone(),
        owner: "owner".to_owned(),
        name: "repo".to_owned(),
        url: "https://example.invalid/owner/repo".to_owned(),
        context_id: context,
        default_branch: "main".to_owned(),
        path: repos.join("owner/repo").display().to_string(),
        cloned_at: "2026-09-08T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree.clone(),
        repo_id: repo,
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.display().to_string(),
        session: "repo/feature".to_owned(),
        host: None,
        created_at: "2026-09-08T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state.save(persisted).await.expect("save remote state");

    worktree
}
