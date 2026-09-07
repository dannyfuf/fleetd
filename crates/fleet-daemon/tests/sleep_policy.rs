use std::sync::Arc;

use fleet_core::{
    config::{NATIVE_LAZYGIT, WindowConfig},
    ids::{ContextId, RepoId, SessionId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    adapters::{
        clock::SystemClock,
        files::RealFiles,
        process::{ListeningPort, Process, ProcessInfo},
    },
    services::{sessions::Sessions, sleep::Sleep},
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::FakeProcess,
};

async fn stores(
    windows: Vec<WindowConfig>,
) -> (
    tempfile::TempDir,
    Arc<ConfigStore>,
    Arc<StateStore>,
    WorktreeId,
) {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let repos = home.join("repos");
    let worktree_path = home.join("worktrees/repo/feature");
    std::fs::create_dir_all(&repos).unwrap_or_else(|error| panic!("{error}"));
    std::fs::create_dir_all(&worktree_path).unwrap_or_else(|error| panic!("{error}"));
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [repos.clone(), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let mut effective = config
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    effective.windows = windows;
    effective.sleep.grace_ms = 0;
    config
        .save(effective)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(StateStore::new(&home, files, Arc::new(SystemClock)));
    let context = ContextId::try_from("team").unwrap_or_else(|error| panic!("{error}"));
    let repo = RepoId::try_from("owner/repo").unwrap_or_else(|error| panic!("{error}"));
    let worktree =
        WorktreeId::try_from("owner/repo#feature").unwrap_or_else(|error| panic!("{error}"));
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context.clone(),
        name: "Team".to_owned(),
        owners: Vec::new(),
        created_at: "2026-09-04T00:00:00Z".to_owned(),
    });
    persisted.repos.push(Repo {
        id: repo.clone(),
        owner: "owner".to_owned(),
        name: "repo".to_owned(),
        url: "https://example.invalid/owner/repo".to_owned(),
        context_id: context,
        default_branch: "main".to_owned(),
        path: repos.join("owner/repo").to_string_lossy().into_owned(),
        cloned_at: "2026-09-04T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree.clone(),
        repo_id: repo,
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.to_string_lossy().into_owned(),
        session: "repo/feature".to_owned(),
        host: None,
        created_at: "2026-09-04T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    (temp, config, state, worktree)
}

#[tokio::test]
async fn keeps_matching_process_and_closes_idle_terminal() {
    let (_temp, config, state, worktree) = stores(vec![
        WindowConfig {
            name: "worker".to_owned(),
            command: "/bin/sh -c 'sleep 30'".to_owned(),
        },
        WindowConfig {
            name: "idle".to_owned(),
            command: "/bin/sh -c 'sleep 30'".to_owned(),
        },
    ])
    .await;
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let session = sessions
        .ensure(Some(worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let worker_shell = session.terminals[0]
        .shell_pid
        .unwrap_or_else(|| panic!("shell pid"));
    let fake = Arc::new(FakeProcess::default());
    fake.set_snapshot(vec![ProcessInfo {
        pid: 50_001,
        parent_pid: worker_shell,
        command: "/usr/local/bin/claude --resume".to_owned(),
    }]);
    let process: Arc<dyn Process> = fake.clone();
    let sleep = Sleep::new(config, state, process, &sessions);
    let result = sleep
        .session(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.kept.len(), 1);
    assert_eq!(result.kept[0].window, "worker");
    assert_eq!(result.kept[0].reason, "claude");
    assert_eq!(result.closed, vec!["idle"]);
    assert!(!result.session_killed);
    assert_eq!(sessions.snapshot()[0].terminals.len(), 1);
    sessions
        .kill(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn listening_port_keeps_terminal_and_missing_session_is_empty() {
    let (_temp, config, state, worktree) = stores(vec![WindowConfig {
        name: "server".to_owned(),
        command: "/bin/sh -c 'sleep 30'".to_owned(),
    }])
    .await;
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let session = sessions
        .ensure(Some(worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let shell = session.terminals[0]
        .shell_pid
        .unwrap_or_else(|| panic!("shell pid"));
    let fake = Arc::new(FakeProcess::default());
    fake.set_snapshot(vec![ProcessInfo {
        pid: 50_002,
        parent_pid: shell,
        command: "/bin/sh -c server".to_owned(),
    }]);
    fake.set_ports(vec![ListeningPort {
        pid: 50_002,
        port: 4_321,
    }]);
    let process: Arc<dyn Process> = fake;
    let sleep = Sleep::new(config, state, process, &sessions);
    let result = sleep
        .session(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.kept[0].reason, ":4321");
    let matches = sleep
        .match_keep_alive_rules()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        matches
            .iter()
            .find(|entry| entry.rule_id == "servers")
            .map(|entry| entry.count),
        Some(1)
    );
    let missing = sleep
        .session(SessionId::try_from("repo/missing").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(missing.kept.is_empty());
    assert!(missing.closed.is_empty());
    assert!(!missing.session_killed);
    sessions
        .kill(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn no_matches_kills_session_once() {
    let (_temp, config, state, worktree) = stores(vec![WindowConfig {
        name: "idle".to_owned(),
        command: "/bin/sh -c 'sleep 30'".to_owned(),
    }])
    .await;
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    sessions
        .ensure(Some(worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let fake = Arc::new(FakeProcess::default());
    let process: Arc<dyn Process> = fake;
    let sleep = Sleep::new(config, state, process, &sessions);
    let result = sleep
        .session(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.closed, vec!["idle"]);
    assert!(result.session_killed);
    assert!(sessions.snapshot().is_empty());
}

/// Sleep treats a `fleet://` tab as idle: no process, so nothing can keep it alive.
///
/// A keep-alive rule matches process names, and there is no process; asking an editor to `:qa`
/// needs a PTY to type into, and there is none. It closes with the other idle tabs, exactly as
/// the `lazygit` PTY it replaces did, and `ensure` puts it back on the next wake.
#[tokio::test]
async fn a_native_tab_is_idle_and_never_keeps_a_session_awake() {
    let (_temp, config, state, worktree) = stores(vec![
        WindowConfig {
            name: "worker".to_owned(),
            command: "/bin/sh -c 'sleep 30'".to_owned(),
        },
        WindowConfig {
            name: "lg".to_owned(),
            command: NATIVE_LAZYGIT.to_owned(),
        },
    ])
    .await;
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let session = sessions
        .ensure(Some(worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(session.terminals[1].is_native());
    let worker_shell = session.terminals[0]
        .shell_pid
        .unwrap_or_else(|| panic!("shell pid"));

    let fake = Arc::new(FakeProcess::default());
    fake.set_snapshot(vec![ProcessInfo {
        pid: 50_003,
        parent_pid: worker_shell,
        command: "/usr/local/bin/claude".to_owned(),
    }]);
    // A port that belongs to nothing in this session must not be able to keep the native tab
    // awake by accident.
    fake.set_ports(vec![ListeningPort {
        pid: 50_003,
        port: 4_000,
    }]);
    let process: Arc<dyn Process> = fake;
    let sleep = Sleep::new(config, state, process, &sessions);
    let result = sleep
        .session(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        result
            .kept
            .iter()
            .map(|kept| kept.window.as_str())
            .collect::<Vec<_>>(),
        vec!["worker"]
    );
    assert_eq!(result.closed, vec!["lg"]);
    assert!(!result.session_killed);

    // A session that is *only* a native tab sleeps away completely.
    let remaining = sessions.snapshot();
    assert_eq!(remaining[0].terminals.len(), 1);
    sessions
        .kill(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn rule_counts_use_parser_diagnostics_and_refresh_after_config_changes() {
    let (_temp, config, state, _) = stores(Vec::new()).await;
    config
        .update(serde_json::json!({"sleep": {"keepAlive": [
            {"id": "same", "label": "same", "kind": "process", "pattern": "claude"},
            {"id": "same", "label": "same", "kind": "process", "pattern": "cargo"},
            {"id": "same", "label": "same", "kind": "process", "pattern": "\\q"},
            {"id": "off", "label": "off", "kind": "process", "pattern": "[", "enabled": false},
            {"id": "bracket", "label": "bracket", "kind": "process", "pattern": "]"}
        ]}}))
        .await
        .unwrap();
    let fake = Arc::new(FakeProcess::default());
    fake.set_snapshot(vec![
        ProcessInfo {
            pid: 50_010,
            parent_pid: 1,
            command: "CLAUDE ]".to_owned(),
        },
        ProcessInfo {
            pid: 50_011,
            parent_pid: 1,
            command: "cargo test".to_owned(),
        },
        ProcessInfo {
            pid: 50_012,
            parent_pid: 1,
            command: "cargo build".to_owned(),
        },
    ]);
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let sleep = Sleep::new(Arc::clone(&config), state, fake, &sessions);
    for _ in 0..2 {
        let matches = sleep.match_keep_alive_rules().await.unwrap();
        assert_eq!(
            matches.iter().map(|entry| entry.count).collect::<Vec<_>>(),
            [1, 2, 0, 0, 1]
        );
        let error = matches[2].error.as_deref().unwrap();
        assert!(error.contains("unrecognized escape sequence"), "{error}");
        assert!(
            matches
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != 2)
                .all(|(_, entry)| entry.error.is_none())
        );
    }
    config
        .update(serde_json::json!({"sleep": {"keepAlive": [
            {"id": "same", "label": "same", "kind": "process", "pattern": "cargo"}
        ]}}))
        .await
        .unwrap();
    let matches = sleep.clone().match_keep_alive_rules().await.unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].count, 2);
    assert!(matches[0].error.is_none());
}

#[tokio::test]
async fn sleeping_uses_changed_rules_without_recreating_the_service() {
    let (_temp, config, state, worktree) = stores(vec![WindowConfig {
        name: "worker".to_owned(),
        command: "/bin/sh -c 'sleep 30'".to_owned(),
    }])
    .await;
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let session = sessions.ensure(Some(worktree), None, false).await.unwrap();
    let fake = Arc::new(FakeProcess::default());
    fake.set_snapshot(vec![ProcessInfo {
        pid: 50_020,
        parent_pid: session.terminals[0].shell_pid.unwrap(),
        command: "claude".to_owned(),
    }]);
    let sleep = Sleep::new(Arc::clone(&config), state, fake, &sessions);
    let session_id = SessionId::try_from("repo/feature").unwrap();
    assert_eq!(
        sleep.session(session_id.clone()).await.unwrap().kept[0].reason,
        "claude"
    );
    config
        .update(serde_json::json!({"sleep": {"keepAlive": [
            {"id": "worker", "label": "renamed", "kind": "process", "pattern": "claude"}
        ]}}))
        .await
        .unwrap();
    assert_eq!(
        sleep.session(session_id.clone()).await.unwrap().kept[0].reason,
        "renamed"
    );
    config
        .update(serde_json::json!({"sleep": {"keepAlive": []}}))
        .await
        .unwrap();
    let result = sleep.session(session_id).await.unwrap();
    assert!(result.kept.is_empty());
    assert!(result.session_killed);
}
