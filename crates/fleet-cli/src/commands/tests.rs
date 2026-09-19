use super::{
    sessions::resolve_agent_status_target,
    watches::watch_tail_to,
    worktrees::{parse_hooks, parse_host_or_default},
};
use crate::{
    args::{
        AgentStatusArgs, AgentStatusChoice, DoctorArgs, KillArgs, WatchListArgs, WatchTailArgs,
    },
    human,
};
use fleet_core::{
    ids::{JobId, SessionId, WorktreeId},
    validate::validate_slug,
    watches::{WatchStatus, WatchStream},
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    error::{ErrorKind, ProtoError},
    event::Event,
    job::{JobKind, JobRecord, JobStatus},
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use std::{ffi::OsString, path::Path, time::Duration};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio_util::codec::Framed;

use super::*;

struct BrokenPipe;

impl std::io::Write for BrokenPipe {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "closed pipeline",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn broken_stdout_pipe_returns_cleanly() {
    let mut stderr = Vec::new();
    for arguments in [
        vec![OsString::from("fleet"), OsString::from("--version")],
        vec![
            OsString::from("fleet"),
            OsString::from("--json"),
            OsString::from("not-a-command"),
        ],
    ] {
        assert_eq!(run_from(arguments, &mut BrokenPipe, &mut stderr), 0);
    }
    assert!(stderr.is_empty());
}

#[tokio::test]
async fn watch_tail_broken_pipe_returns_cleanly() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        assert_eq!(
            request.body,
            RequestBody::TailWatch {
                watch: sample_watch().id,
                from_seq: None,
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::WatchTail(fleet_proto::watch::WatchTail {
                watch: sample_watch(),
                chunks: vec![fleet_core::watches::WatchChunk {
                    seq: 0,
                    stream: WatchStream::Stdout,
                    text: "first line\n".into(),
                }],
                first_retained_seq: 0,
                next_seq: 1,
            })),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let mut stderr = Vec::new();
    watch_tail_to(
        &client,
        WatchTailArgs {
            id: sample_watch().id,
            follow: false,
        },
        &mut BrokenPipe,
        &mut stderr,
    )
    .await
    .unwrap();
    assert!(stderr.is_empty());
    server.await.unwrap();
}

#[test]
fn watch_session_prefers_explicit_and_requires_a_valid_fallback() {
    let explicit: SessionId = "repo/explicit".parse().unwrap();
    assert_eq!(
        watch_session(Some(explicit.clone()), Some("invalid")).unwrap(),
        explicit
    );
    assert_eq!(
        watch_session(None, Some("repo/main")).unwrap().as_str(),
        "repo/main"
    );
    for environment in [None, Some(""), Some(" ")] {
        let error = watch_session(None, environment).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
        assert!(error.message.contains("--session <id> or FLEET_SESSION"));
    }
    assert!(
        watch_session(None, Some("invalid"))
            .unwrap_err()
            .message
            .contains("invalid FLEET_SESSION")
    );
    let command = Command::Watch(WatchArgs {
        command: WatchCommand::List(WatchListArgs {
            session: None,
            json: true,
        }),
    });
    assert!(command_requests_json(&command));
}

type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

fn sample_watch() -> fleet_core::watches::Watch {
    fleet_core::watches::Watch {
        id: fleet_core::watches::WatchId(42),
        session: "repo/main".parse().unwrap(),
        terminal: fleet_core::ids::TerminalId(7),
        label: "review".into(),
        command: vec!["sh".into()],
        cwd: None,
        pid: None,
        started_at: "2026-09-05T12:00:00Z".into(),
        status: WatchStatus::Running,
        source: fleet_core::watches::WatchSource::Cooperative,
        log_file: None,
    }
}

fn sample_repo() -> fleet_core::model::Repo {
    fleet_core::model::Repo {
        id: "acme/api".parse().unwrap(),
        owner: "acme".into(),
        name: "api".into(),
        url: "https://example.test/acme/api".into(),
        context_id: "acme".parse().unwrap(),
        default_branch: "main".into(),
        path: "/repos/acme/api".into(),
        cloned_at: "now".into(),
        hooks: fleet_core::model::RepoHooks::default(),
    }
}

fn sample_worktree(host: Option<&str>) -> fleet_core::model::Worktree {
    fleet_core::model::Worktree {
        id: "acme/api#feature".parse().unwrap(),
        repo_id: "acme/api".parse().unwrap(),
        slug: "feature".into(),
        branch: "feature".into(),
        base_ref: "origin/main".into(),
        path: "/worktrees/acme/api/feature".into(),
        session: "api/feature".into(),
        host: host.map(|host| host.parse().unwrap()),
        created_at: "now".into(),
        last_opened_at: None,
        degraded: None,
    }
}

fn sample_snapshot(
    repos: Vec<fleet_core::model::Repo>,
    worktrees: Vec<fleet_core::model::Worktree>,
    agent_threads: Vec<fleet_core::agents::AgentThreadSummary>,
) -> fleet_proto::snapshot::Snapshot {
    fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: "now".into(),
        revision: None,
        contexts: Vec::new(),
        repos,
        clones: Vec::new(),
        worktrees,
        active_context: None,
        sessions: Vec::new(),
        agent_threads,
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "test".into(),
            pid: 1,
            started_at: "now".into(),
            home: "/fleet".into(),
        },
    }
}

#[tokio::test]
async fn doctor_reset_state_uses_typed_request_and_protocol_envelope() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let archived = home.path().join("state.json.broken-1");
    let expected = archived.display().to_string();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        assert_eq!(request.body, RequestBody::ResetState);
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Path {
                path: archived.display().to_string(),
                host: None,
            }),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let output = execute(
        &client,
        Command::Doctor(DoctorArgs {
            reset_state: true,
            json: true,
        }),
    )
    .await
    .unwrap();

    assert_eq!(output.exit_code, 0);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
        serde_json::json!({"protocol": 1, "archivedPath": expected})
    );
    server.await.unwrap();
}

#[tokio::test]
async fn watch_list_uses_the_requested_session_and_protocol_envelope() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let watch = sample_watch();
    let expected = watch.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        assert_eq!(
            request.body,
            RequestBody::ListWatches {
                session: watch.session.clone()
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Watches(vec![watch])),
        )
        .await;
    });
    let client = Client::connect(home.path()).await.unwrap();
    let output = execute(
        &client,
        Command::Watch(WatchArgs {
            command: WatchCommand::List(WatchListArgs {
                session: Some(expected.session.clone()),
                json: true,
            }),
        }),
    )
    .await
    .unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
        serde_json::json!({ "protocol": 1, "watches": [expected] })
    );
    server.await.unwrap();
}

#[test]
fn watch_list_human_rows_include_status_time_and_terminal() {
    let mut watch = sample_watch();
    for (status, label) in [
        (WatchStatus::Running, "running"),
        (
            WatchStatus::Exited {
                code: Some(3),
                signal: None,
            },
            "exited 3",
        ),
        (
            WatchStatus::Exited {
                code: None,
                signal: Some(9),
            },
            "interrupted",
        ),
    ] {
        watch.status = status;
        assert_eq!(
            human::watches(&[watch.clone()]),
            format!("42\tcooperative\treview\t{label}\t2026-09-05T12:00:00Z\t7")
        );
    }
    assert_eq!(human::watches(&[]), "");
}

#[test]
fn worktree_list_human_rows_include_host_and_session_columns() {
    let repo = sample_repo();
    let local = sample_worktree(None);
    let mut remote = sample_worktree(Some("dev-box"));
    remote.id = "acme/api#remote".parse().unwrap();
    remote.session = "dev-box/api/remote".into();

    assert_eq!(
        human::list(&[repo], &[local, remote]),
        "1 repos, 2 worktrees\nWORKTREE\tHOST\tSESSION\nacme/api#feature\tlocal\tapi/feature\nacme/api#remote\tdev-box\tdev-box/api/remote"
    );
}

#[test]
fn lifecycle_human_output_keeps_remote_per_item_errors() {
    let inspection = fleet_core::inspection::WorktreeInspection {
        worktree_id: "acme/api#feature".parse().unwrap(),
        repo_id: "acme/api".parse().unwrap(),
        host: "dev-box".into(),
        path: "/worktrees/acme/api/feature".into(),
        branch: "feature".into(),
        base_ref: "origin/main".into(),
        head: None,
        target_branch: "main".into(),
        upstream: None,
        ahead: None,
        behind: None,
        upstream_gone: false,
        dirty: false,
        dirty_files: None,
        merged_into_target: false,
        unique_commits: None,
        published: false,
        merged: false,
        pr: None,
        session: fleet_core::sessions::SessionState::Unknown,
        running: Vec::new(),
        inspected_at: "now".into(),
        warnings: Vec::new(),
        error: Some("host `dev-box` is unreachable".into()),
    };
    assert_eq!(
        human::inspect(&[inspection]),
        "acme/api#feature dev-box feature error: host `dev-box` is unreachable"
    );

    let deletion = fleet_proto::response::WorktreeDeleteResult {
        worktree_id: "acme/api#feature".parse().unwrap(),
        ok: false,
        reason: Some("host `dev-box` is unreachable".into()),
        trash_entry: None,
    };
    assert_eq!(
        human::delete(&[deletion]),
        "Failed acme/api#feature: host `dev-box` is unreachable"
    );

    let prune = fleet_proto::response::PruneResult {
        dry_run: false,
        deleted: vec!["acme/api#local".parse().unwrap()],
        skipped: vec![fleet_proto::response::PruneSkipped {
            worktree_id: "acme/api#feature".parse().unwrap(),
            reason: "host `dev-box` is unreachable".into(),
            merged: false,
            dirty: false,
            unique_commits: None,
            running: Vec::new(),
        }],
    };
    assert_eq!(
        human::prune(&prune),
        "Deleted acme/api#local\nSkipped acme/api#feature: host `dev-box` is unreachable"
    );
}

#[tokio::test]
async fn watch_tail_preserves_streams_and_follows_next_cursor_through_final_output() {
    for follow in [false, true] {
        let home = TempDir::new().unwrap();
        let listener = bind(home.path()).await;
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut transport = Framed::new(socket, FleetCodec::new());
            authenticate(&mut transport).await;
            for index in 0..=usize::from(follow) {
                let request = next_request(&mut transport).await;
                assert_eq!(
                    request.body,
                    RequestBody::TailWatch {
                        watch: sample_watch().id,
                        from_seq: (index == 1).then_some(5),
                    }
                );
                let mut watch = sample_watch();
                if index == 1 {
                    watch.status = WatchStatus::Exited {
                        code: None,
                        signal: Some(9),
                    };
                }
                send_result(
                    &mut transport,
                    request.id,
                    Ok(ResponseBody::WatchTail(fleet_proto::watch::WatchTail {
                        watch,
                        chunks: vec![fleet_core::watches::WatchChunk {
                            seq: 4 + index as u64,
                            stream: if index == 0 {
                                WatchStream::Stdout
                            } else {
                                WatchStream::Stderr
                            },
                            text: if index == 0 { "retained\n" } else { "final" }.into(),
                        }],
                        first_retained_seq: 4,
                        next_seq: 5 + index as u64,
                    })),
                )
                .await;
                if follow && index == 0 {
                    assert!(
                        tokio::time::timeout(Duration::from_millis(350), transport.next())
                            .await
                            .is_err()
                    );
                    let mut watch = sample_watch();
                    watch.status = WatchStatus::Exited {
                        code: None,
                        signal: Some(9),
                    };
                    send_event(&mut transport, Event::WatchExited(watch)).await;
                }
            }
        });
        let client = Client::connect(home.path()).await.unwrap();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        tokio::time::timeout(
            Duration::from_secs(3),
            watch_tail_to(
                &client,
                WatchTailArgs {
                    id: sample_watch().id,
                    follow,
                },
                &mut stdout,
                &mut stderr,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(stdout, b"retained\n");
        assert_eq!(
            stderr,
            if follow {
                b"final".as_slice()
            } else {
                b"".as_slice()
            }
        );
        server.await.unwrap();
    }
}

/// Runs `fleet kill acme/api#feature --json` against an in-process daemon answering `reply`.
async fn kill_worktree(
    reply: Result<ResponseBody, ProtoError>,
) -> Result<CommandOutput, ProtoError> {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        let RequestBody::KillWorktree { id } = request.body else {
            panic!("expected kill request");
        };
        assert_eq!(id.as_str(), "acme/api#feature");
        send_result(&mut transport, request.id, reply).await;
    });
    let client = Client::connect(home.path()).await.unwrap();
    let result = execute(
        &client,
        Command::Kill(KillArgs {
            id: "acme/api#feature".to_owned(),
            json: true,
        }),
    )
    .await;
    server.await.unwrap();
    result
}

#[tokio::test]
async fn formats_success_envelope_against_an_in_process_daemon() {
    let output = kill_worktree(Ok(ResponseBody::Ack)).await.unwrap();
    assert_eq!(output.text, r#"{"protocol":1,"ok":true}"#);
    assert_eq!(output.exit_code, 0);
}

#[tokio::test]
async fn preserves_proto_error_kind_from_an_in_process_daemon() {
    let error = kill_worktree(Err(ProtoError {
        kind: ErrorKind::NotFound,
        message: "missing worktree".to_owned(),
    }))
    .await
    .unwrap_err();
    assert_eq!(
        error_json(&error),
        r#"{"protocol":1,"error":{"kind":"not-found","message":"missing worktree"}}"#
    );
}

#[tokio::test]
async fn path_prefixes_remote_paths_with_their_host() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        assert_eq!(
            request.body,
            RequestBody::WorktreePath {
                id: "acme/api#feature".parse().unwrap()
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Path {
                path: "/srv/fleet/worktrees/acme/api/feature".to_owned(),
                host: Some("dev-box".parse().unwrap()),
            }),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let output = execute(
        &client,
        Command::Path(crate::args::PathArgs {
            id: "acme/api#feature".to_owned(),
            json: false,
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        output,
        CommandOutput::success("dev-box:/srv/fleet/worktrees/acme/api/feature".to_owned())
    );
    server.await.unwrap();
}

#[tokio::test]
async fn agent_list_includes_the_owning_worktree_host_column() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let thread = fleet_core::agents::AgentThreadSummary {
        thread: "00000000-0000-0000-0000-000000000001".parse().unwrap(),
        parent: None,
        worktree: "acme/api#feature".parse().unwrap(),
        host: Some("dev-box".parse().unwrap()),
        provider: fleet_core::agents::AgentKind::Claude,
        title: "Remote fix".into(),
        attention: fleet_core::agents::Attention::Idle,
        session: fleet_core::agents::SessionState::Ready,
        turn: fleet_core::agents::TurnState::None,
        last_seq: fleet_core::agents::Seq(0),
        last_activity: None,
        last_completed_seq: None,
        last_nonterminal_seq: None,
        exit_code: None,
    };
    let expected = thread.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let list = next_request(&mut transport).await;
        assert_eq!(list.body, RequestBody::AgentThreadList);
        send_result(
            &mut transport,
            list.id,
            Ok(ResponseBody::AgentThreads(vec![thread])),
        )
        .await;
        let snapshot = next_request(&mut transport).await;
        assert_eq!(snapshot.body, RequestBody::GetSnapshot);
        send_result(
            &mut transport,
            snapshot.id,
            Ok(ResponseBody::Snapshot(sample_snapshot(
                Vec::new(),
                vec![sample_worktree(Some("dev-box"))],
                Vec::new(),
            ))),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let output = execute(
        &client,
        Command::Agent(crate::args::AgentArgs {
            command: crate::args::AgentCommand::List,
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        output.text,
        format!(
            "{}\tclaude\tdev-box\tready\tidle\tacme/api#feature\tRemote fix",
            expected.thread
        )
    );
    server.await.unwrap();
}

/// One retained event that only advances the cursor, so a tail test can count printed lines.
fn sample_agent_event(seq: u64) -> fleet_core::agents::SeqEvent {
    fleet_core::agents::SeqEvent {
        seq: fleet_core::agents::Seq(seq),
        at: "2026-09-18T12:00:00Z".parse().unwrap(),
        raw: None,
        event: fleet_core::agents::AgentEvent::SessionActivity {
            phase: format!("phase-{seq}"),
        },
    }
}

/// Answers exactly one cursored `AgentThreadOpen` with a retained tail and nothing else.
fn tail_snapshot_server(
    listener: UnixListener,
    thread: fleet_core::agents::ThreadId,
    retained: Vec<fleet_core::agents::SeqEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        // A tail always opens with a cursor so reading a thread never resumes its provider.
        assert_eq!(
            request.body,
            RequestBody::AgentThreadOpen {
                thread,
                from_seq: Some(fleet_core::agents::Seq(0)),
                after_seq: None,
                turn_limit: None,
                before_cursor: None,
                request_sync_marker: false,
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::AgentThreadSnapshot {
                projection: fleet_core::agents::ThreadProjection::new(
                    thread,
                    "acme/api#feature".parse().unwrap(),
                    fleet_core::agents::AgentKind::Claude,
                ),
                events_after: retained,
            }),
        )
        .await;
    })
}

#[tokio::test]
async fn agent_tail_no_follow_prints_the_retained_snapshot_and_returns() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let thread: fleet_core::agents::ThreadId =
        "00000000-0000-4000-8000-000000000009".parse().unwrap();
    let retained = (1..=3).map(sample_agent_event).collect::<Vec<_>>();
    let server = tail_snapshot_server(listener, thread, retained.clone());

    let client = Client::connect(home.path()).await.unwrap();
    let mut output = Vec::new();
    // The reported failure: a live thread with retained history and no new event printed
    // nothing at all, because without `--replay` the history is only folded into the
    // projection. `--no-follow` implies the replay and returns instead of blocking.
    agents::tail_to(
        &client,
        crate::args::AgentTailArgs {
            thread,
            replay: false,
            no_follow: true,
            last: None,
        },
        &mut output,
    )
    .await
    .unwrap();

    let printed = String::from_utf8(output).unwrap();
    assert_eq!(
        printed,
        retained
            .iter()
            .map(|event| format!("{}\n", serde_json::to_string(event).unwrap()))
            .collect::<String>()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn agent_tail_last_trims_the_replay_to_its_newest_events() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let thread: fleet_core::agents::ThreadId =
        "00000000-0000-4000-8000-000000000009".parse().unwrap();
    let retained = (1..=5).map(sample_agent_event).collect::<Vec<_>>();
    let server = tail_snapshot_server(listener, thread, retained.clone());

    let client = Client::connect(home.path()).await.unwrap();
    let mut output = Vec::new();
    agents::tail_to(
        &client,
        crate::args::AgentTailArgs {
            thread,
            replay: false,
            no_follow: true,
            last: Some(2),
        },
        &mut output,
    )
    .await
    .unwrap();

    let printed = String::from_utf8(output).unwrap();
    assert_eq!(
        printed,
        retained[3..]
            .iter()
            .map(|event| format!("{}\n", serde_json::to_string(event).unwrap()))
            .collect::<String>()
    );
    // The trim is a print filter: asking for more than exists still prints everything rather
    // than erroring, which is what makes `--last` safe to hardcode in a script.
    let second_home = TempDir::new().unwrap();
    let listener = bind(second_home.path()).await;
    let server_all = tail_snapshot_server(listener, thread, retained.clone());
    let client = Client::connect(second_home.path()).await.unwrap();
    let mut all = Vec::new();
    agents::tail_to(
        &client,
        crate::args::AgentTailArgs {
            thread,
            replay: false,
            no_follow: true,
            last: Some(50),
        },
        &mut all,
    )
    .await
    .unwrap();
    assert_eq!(String::from_utf8(all).unwrap().lines().count(), 5);
    server.await.unwrap();
    server_all.await.unwrap();
}

#[test]
fn maps_parser_and_domain_validation_failures_to_validation_errors() {
    let duplicate = Cli::try_parse_from(["fleet", "list", "--json", "--json"]).unwrap_err();
    assert_eq!(clap_error(&duplicate).kind, ErrorKind::Validation);

    let id_error = parse_id::<WorktreeId>("not-a-worktree").unwrap_err();
    assert_eq!(id_error.kind, ErrorKind::Validation);

    let slug_error = validate_slug("Not Canonical")
        .map_err(|error| validation(error.to_string()))
        .unwrap_err();
    assert_eq!(slug_error.kind, ErrorKind::Validation);

    let hooks_error = parse_hooks(r#"{"prepare":[],"unexpected":true}"#).unwrap_err();
    assert_eq!(hooks_error.kind, ErrorKind::Validation);
}

#[test]
fn create_host_uses_default_host_only_when_flag_is_omitted() {
    let default: fleet_core::ids::HostId = "dev-box".parse().unwrap();

    assert_eq!(
        parse_host_or_default(None, Some(&default))
            .unwrap()
            .as_ref()
            .map(fleet_core::ids::HostId::as_str),
        Some("dev-box")
    );
    assert_eq!(
        parse_host_or_default(Some("build-box"), Some(&default))
            .unwrap()
            .unwrap()
            .as_str(),
        "build-box"
    );
    assert_eq!(
        parse_host_or_default(Some("local"), Some(&default)).unwrap(),
        None
    );
}

#[test]
fn host_prefixed_session_ids_parse_transparently() {
    let session = parse_id::<SessionId>("dev-box/api/feature").unwrap();
    assert_eq!(session.as_str(), "dev-box/api/feature");

    let arguments = AgentStatusArgs {
        activity: AgentStatusChoice::Working,
        session: Some(session.to_string()),
        terminal_id: Some(9),
        json: false,
    };
    assert_eq!(
        resolve_agent_status_target(&arguments, |_| None).unwrap().0,
        session
    );
}

#[test]
fn agent_status_resolves_flags_before_fleet_environment() {
    let arguments = AgentStatusArgs {
        activity: AgentStatusChoice::Working,
        session: Some("acme/api".to_owned()),
        terminal_id: Some(9),
        json: false,
    };
    let resolved = resolve_agent_status_target(&arguments, |_| None).unwrap();
    assert_eq!(resolved.0.as_str(), "acme/api");
    assert_eq!(resolved.1, fleet_core::ids::TerminalId(9));

    let arguments = AgentStatusArgs {
        activity: AgentStatusChoice::Finished,
        session: None,
        terminal_id: None,
        json: false,
    };
    let resolved = resolve_agent_status_target(&arguments, |name| match name {
        "FLEET_SESSION" => Some(OsString::from("repo/feature")),
        "FLEET_TERMINAL_ID" => Some(OsString::from("42")),
        _ => None,
    })
    .unwrap();
    assert_eq!(resolved.0.as_str(), "repo/feature");
    assert_eq!(resolved.1, fleet_core::ids::TerminalId(42));
}

#[test]
fn agent_status_reports_missing_environment() {
    let arguments = AgentStatusArgs {
        activity: AgentStatusChoice::Finished,
        session: None,
        terminal_id: None,
        json: false,
    };
    let error = resolve_agent_status_target(&arguments, |_| None).unwrap_err();
    assert!(error.message.contains("FLEET_SESSION"));
}

fn update_job(status: JobStatus) -> JobRecord {
    JobRecord {
        id: JobId::try_from("update-1").unwrap(),
        kind: JobKind::Update,
        target: "fleet".to_owned(),
        title: "Update Fleet".to_owned(),
        status,
        progress: None,
        log_path: "/tmp/update.log".to_owned(),
        started_at: "2026-09-04T12:00:00Z".to_owned(),
        finished_at: None,
        cancellable: true,
        retryable: false,
    }
}

#[test]
fn subagent_context_uses_environment_fallbacks_and_refuses_a_missing_caller() {
    let command = Cli::try_parse_from([
        "fleet",
        "subagent",
        "run",
        "--provider",
        "codex",
        "--expect",
        "done",
    ])
    .unwrap()
    .command
    .unwrap();
    let Command::Subagent(arguments) = command else {
        panic!("expected subagent command");
    };
    let error = subagents::validate_context(&arguments.command, &subagents::Environment::default())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(
        error.message,
        "fleet subagent run requires --caller <thread> or FLEET_SESSION"
    );

    let environment = subagents::Environment {
        session: Some("00000000-0000-4000-8000-000000000001".to_owned()),
        delegation: None,
        token: None,
    };
    subagents::validate_context(&arguments.command, &environment).unwrap();
}

/// Parses `fleet subagent <arguments>` and returns the verb clap built.
fn parse_subagent(arguments: &[&str]) -> crate::args::SubagentCommand {
    let line = ["fleet", "subagent"]
        .into_iter()
        .chain(arguments.iter().copied())
        .collect::<Vec<_>>();
    let Some(Command::Subagent(crate::args::SubagentArgs { command })) =
        Cli::try_parse_from(line).unwrap().command
    else {
        panic!("expected a subagent command");
    };
    command
}

#[test]
fn every_subagent_verb_parses_its_flags_and_defaults() {
    use crate::args::{
        AgentChoice, AgentModeChoice, SubagentArgs, SubagentCommand, SubagentCompleteArgs,
        SubagentIdArgs, SubagentListArgs, SubagentRunArgs, SubagentWaitArgs,
    };

    const CALLER: &str = "00000000-0000-4000-8000-000000000001";
    const DELEGATION: &str = "00000000-0000-4000-8000-000000000003";

    assert_eq!(
        parse_subagent(&[
            "run",
            "--provider",
            "claude",
            "--brief-file",
            "/tmp/brief.md",
            "--expect",
            "tests pass",
            "--worktree",
            "acme/api#feature",
            "--mode",
            "full-access",
            "--model",
            "opus",
            "--effort",
            "high",
            "--title",
            "worker",
            "--eager",
            "--caller",
            CALLER,
            "--json",
        ]),
        SubagentCommand::Run(SubagentRunArgs {
            provider: AgentChoice::Claude,
            brief_file: Some("/tmp/brief.md".into()),
            expectation: "tests pass".to_owned(),
            worktree: Some("acme/api#feature".parse().unwrap()),
            mode: Some(AgentModeChoice::FullAccess),
            model: Some("opus".to_owned()),
            effort: Some("high".to_owned()),
            title: Some("worker".to_owned()),
            eager: true,
            caller: Some(CALLER.parse().unwrap()),
            json: true,
        })
    );
    // Without a brief file the brief comes from stdin, and every override stays unset so the
    // daemon applies its own defaults rather than the CLI guessing them.
    assert_eq!(
        parse_subagent(&["run", "--provider", "codex", "--expect", "the file path"]),
        SubagentCommand::Run(SubagentRunArgs {
            provider: AgentChoice::Codex,
            brief_file: None,
            expectation: "the file path".to_owned(),
            worktree: None,
            mode: None,
            model: None,
            effort: None,
            title: None,
            eager: false,
            caller: None,
            json: false,
        })
    );

    assert_eq!(
        parse_subagent(&[
            "complete",
            DELEGATION,
            "--result-file",
            "/tmp/report.md",
            "--blocked",
            "--json-result",
            "--json",
        ]),
        SubagentCommand::Complete(SubagentCompleteArgs {
            id: Some(DELEGATION.parse().unwrap()),
            result_file: Some("/tmp/report.md".into()),
            blocked: true,
            json_result: true,
            json: true,
        })
    );
    assert_eq!(
        parse_subagent(&["complete"]),
        SubagentCommand::Complete(SubagentCompleteArgs {
            id: None,
            result_file: None,
            blocked: false,
            json_result: false,
            json: false,
        })
    );

    assert_eq!(
        parse_subagent(&["wait", DELEGATION]),
        SubagentCommand::Wait(SubagentWaitArgs {
            id: DELEGATION.parse().unwrap(),
            timeout: 540,
            json: false,
        })
    );
    assert_eq!(
        parse_subagent(&["wait", DELEGATION, "--timeout", "30", "--json"]),
        SubagentCommand::Wait(SubagentWaitArgs {
            id: DELEGATION.parse().unwrap(),
            timeout: 30,
            json: true,
        })
    );
    // 540 is a default, not a ceiling. A caller whose own tool timeout is longer than Claude
    // Code's — or who is not a tool call at all — may wait as long as it likes, and the CLI was
    // the only thing that ever said otherwise.
    assert_eq!(
        parse_subagent(&["wait", DELEGATION, "--timeout", "3600"]),
        SubagentCommand::Wait(SubagentWaitArgs {
            id: DELEGATION.parse().unwrap(),
            timeout: 3_600,
            json: false,
        })
    );
    // Zero still parses and still means "ask once and answer with whatever is recorded now".
    assert_eq!(
        parse_subagent(&["wait", DELEGATION, "--timeout", "0"]),
        SubagentCommand::Wait(SubagentWaitArgs {
            id: DELEGATION.parse().unwrap(),
            timeout: 0,
            json: false,
        })
    );

    assert_eq!(
        parse_subagent(&["status", DELEGATION]),
        SubagentCommand::Status(SubagentIdArgs {
            id: DELEGATION.parse().unwrap(),
            json: false,
        })
    );
    assert_eq!(
        parse_subagent(&["cancel", DELEGATION, "--json"]),
        SubagentCommand::Cancel(SubagentIdArgs {
            id: DELEGATION.parse().unwrap(),
            json: true,
        })
    );

    assert_eq!(
        parse_subagent(&["list"]),
        SubagentCommand::List(SubagentListArgs {
            caller: None,
            json: false,
        })
    );
    assert_eq!(
        parse_subagent(&["list", "--caller", CALLER, "--json"]),
        SubagentCommand::List(SubagentListArgs {
            caller: Some(CALLER.parse().unwrap()),
            json: true,
        })
    );

    // The JSON flag has to be visible to the error printer too, or a refused `--json` verb
    // answers a bare line on stderr instead of an error envelope.
    assert!(command_requests_json(&Command::Subagent(SubagentArgs {
        command: parse_subagent(&["status", DELEGATION, "--json"]),
    })));
    assert!(!command_requests_json(&Command::Subagent(SubagentArgs {
        command: parse_subagent(&["status", DELEGATION]),
    })));
}

#[test]
fn subagent_run_accepts_every_shared_permission_mode() {
    use crate::args::{AgentModeChoice, SubagentCommand};

    for (argument, expected) in [
        ("ask", AgentModeChoice::Ask),
        ("accept-edits", AgentModeChoice::AcceptEdits),
        ("plan", AgentModeChoice::Plan),
        ("auto", AgentModeChoice::Auto),
        ("dont-ask", AgentModeChoice::DontAsk),
        ("full-access", AgentModeChoice::FullAccess),
    ] {
        let SubagentCommand::Run(run) = parse_subagent(&[
            "run",
            "--provider",
            "claude",
            "--expect",
            "tests pass",
            "--mode",
            argument,
        ]) else {
            panic!("expected a subagent run command");
        };
        assert_eq!(run.mode, Some(expected));
    }
}

#[test]
fn subagent_complete_names_each_environment_variable_the_child_is_missing() {
    use crate::args::{SubagentCommand, SubagentCompleteArgs};

    let command = SubagentCommand::Complete(SubagentCompleteArgs {
        id: None,
        result_file: None,
        blocked: false,
        json_result: false,
        json: false,
    });
    let refusal = |environment: &subagents::Environment| {
        subagents::validate_context(&command, environment)
            .unwrap_err()
            .message
    };

    let mut environment = subagents::Environment::default();
    assert_eq!(
        refusal(&environment),
        "fleet subagent complete requires <id> or FLEET_DELEGATION"
    );
    environment.delegation = Some("00000000-0000-4000-8000-000000000003".to_owned());
    assert_eq!(
        refusal(&environment),
        "fleet subagent complete requires FLEET_SESSION"
    );
    environment.session = Some("00000000-0000-4000-8000-000000000002".to_owned());
    assert_eq!(
        refusal(&environment),
        "fleet subagent complete requires FLEET_DELEGATION_TOKEN"
    );
    environment.token = Some("secret-token".to_owned());
    subagents::validate_context(&command, &environment).unwrap();
}

#[tokio::test]
async fn subagent_complete_refuses_an_unusable_result_before_sending_anything() {
    use crate::args::{SubagentCommand, SubagentCompleteArgs};

    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        // A report the child cannot supply must never reach the daemon as an empty or malformed
        // one: both refusals below happen before a request is framed.
        assert!(
            tokio::time::timeout(Duration::from_millis(350), transport.next())
                .await
                .is_err()
        );
    });

    let client = Client::connect(home.path()).await.unwrap();
    let delegation = sample_delegation();
    let environment = subagents::Environment {
        session: Some(delegation.child.to_string()),
        delegation: Some(delegation.id.to_string()),
        token: Some("secret-token".to_owned()),
    };

    let missing = home.path().join("absent.md");
    let error = subagents::execute(
        &client,
        SubagentCommand::Complete(SubagentCompleteArgs {
            id: None,
            result_file: Some(missing.clone()),
            blocked: false,
            json_result: false,
            json: false,
        }),
        &environment,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Validation);
    assert!(error.message.contains(&missing.display().to_string()));

    let prose = home.path().join("result.txt");
    std::fs::write(&prose, "plain prose").unwrap();
    let error = subagents::execute(
        &client,
        SubagentCommand::Complete(SubagentCompleteArgs {
            id: None,
            result_file: Some(prose),
            blocked: false,
            json_result: true,
            json: false,
        }),
        &environment,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Validation);
    assert!(error.message.contains("result is not valid JSON"));

    server.await.unwrap();
}

#[tokio::test]
async fn subagent_verbs_use_typed_requests_and_render_human_and_json_output() {
    use crate::args::{
        AgentChoice, AgentModeChoice, SubagentCommand, SubagentCompleteArgs, SubagentIdArgs,
        SubagentListArgs, SubagentRunArgs, SubagentWaitArgs,
    };
    use fleet_core::agents::{AgentKind, ModelSelection, PermissionMode};

    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let brief_file = home.path().join("brief.md");
    std::fs::write(&brief_file, "inspect the parser").unwrap();
    let result_file = home.path().join("result.md");
    std::fs::write(
        &result_file,
        "x".repeat(fleet_proto::agents::ITEM_BODY_MAX_CHUNK_BYTES as usize + 10),
    )
    .unwrap();

    let delegation = sample_delegation();
    let expected = delegation.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;

        let request = next_request(&mut transport).await;
        // The hint is this test binary's own path, so it cannot be spelled out as a literal.
        // Pull it out, check it is the shape the daemon can use — present and absolute, because
        // a relative directory prepended to a child's `PATH` would resolve against whatever
        // working directory the child happened to get — and then pin the rest of the request.
        let RequestBody::DelegationRun { fleet_path, .. } = &request.body else {
            panic!("expected a delegation run request, got {:?}", request.body);
        };
        let fleet_path = fleet_path
            .clone()
            .expect("current_exe resolves for a test binary, so the CLI has a path to send");
        assert!(
            Path::new(&fleet_path).is_absolute(),
            "the daemon prepends the parent of this path to a child's PATH: {fleet_path}"
        );
        assert_eq!(
            request.body,
            RequestBody::DelegationRun {
                caller: expected.caller,
                provider: AgentKind::Codex,
                brief: "inspect the parser".to_owned(),
                expectation: "tests pass".to_owned(),
                worktree: Some("acme/api#feature".parse().unwrap()),
                mode: Some(PermissionMode::FullAccess),
                model: Some(ModelSelection {
                    model: "gpt-5".to_owned(),
                    effort: Some("high".to_owned()),
                    provider: None,
                }),
                title: Some("parser worker".to_owned()),
                fleet_path: Some(fleet_path),
                env: std::collections::BTreeMap::new(),
                eager: true,
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::DelegationStarted {
                delegation: expected.clone(),
                warning: Some("same worktree".to_owned()),
            }),
        )
        .await;

        let request = next_request(&mut transport).await;
        let RequestBody::DelegationComplete {
            delegation: id,
            child,
            token,
            result,
            blocked,
        } = request.body
        else {
            panic!("expected delegation complete request");
        };
        assert_eq!(id, expected.id);
        assert_eq!(child, expected.child);
        assert_eq!(token, "secret-token");
        assert!(blocked);
        assert_eq!(
            result.len(),
            fleet_proto::agents::ITEM_BODY_MAX_CHUNK_BYTES as usize
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Delegation(expected.clone())),
        )
        .await;

        for (timeout_ms, terminal) in [(1_000, false), (2_000, true)] {
            let request = next_request(&mut transport).await;
            assert_eq!(
                request.body,
                RequestBody::DelegationWait {
                    delegation: expected.id,
                    timeout_ms,
                }
            );
            let mut answer = expected.clone();
            if !terminal {
                answer.status = fleet_core::agents::DelegationStatus::Running;
                answer.finished = None;
                answer.result = None;
            }
            send_result(
                &mut transport,
                request.id,
                Ok(ResponseBody::Delegation(answer)),
            )
            .await;
        }

        let request = next_request(&mut transport).await;
        assert_eq!(
            request.body,
            RequestBody::DelegationGet {
                delegation: expected.id
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Delegation(expected.clone())),
        )
        .await;

        let request = next_request(&mut transport).await;
        assert_eq!(
            request.body,
            RequestBody::DelegationList {
                caller: Some(expected.caller)
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Delegations(vec![expected.clone()])),
        )
        .await;

        let request = next_request(&mut transport).await;
        assert_eq!(
            request.body,
            RequestBody::DelegationCancel {
                delegation: expected.id
            }
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Delegation(expected)),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let environment = subagents::Environment {
        session: Some(delegation.child.to_string()),
        delegation: Some(delegation.id.to_string()),
        token: Some("secret-token".to_owned()),
    };
    let run = subagents::execute(
        &client,
        SubagentCommand::Run(SubagentRunArgs {
            provider: AgentChoice::Codex,
            brief_file: Some(brief_file),
            expectation: "tests pass".to_owned(),
            worktree: Some("acme/api#feature".parse().unwrap()),
            mode: Some(AgentModeChoice::FullAccess),
            model: Some("gpt-5".to_owned()),
            effort: Some("high".to_owned()),
            title: Some("parser worker".to_owned()),
            eager: true,
            // The environment below is the child's (its session, delegation and token), so the
            // caller is named explicitly, exactly as a shell without FLEET_SESSION would.
            caller: Some(delegation.caller),
            json: false,
        }),
        &environment,
    )
    .await
    .unwrap();
    assert_eq!(
        run.text,
        format!(
            "delegation {} started, child thread {}\nsame worktree",
            delegation.id, delegation.child
        )
    );

    let complete = subagents::execute(
        &client,
        SubagentCommand::Complete(SubagentCompleteArgs {
            id: None,
            result_file: Some(result_file),
            blocked: true,
            json_result: false,
            json: true,
        }),
        &environment,
    )
    .await
    .unwrap();
    assert_eq!(
        complete.stderr.as_deref(),
        Some("result exceeded 262144 bytes and was truncated")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&complete.text).unwrap(),
        serde_json::json!({"protocol": 1, "delegation": delegation})
    );

    let timed_out = subagents::execute(
        &client,
        SubagentCommand::Wait(SubagentWaitArgs {
            id: delegation.id,
            timeout: 1,
            json: false,
        }),
        &environment,
    )
    .await
    .unwrap();
    assert_eq!(timed_out.exit_code, 2);
    assert!(
        !timed_out.text.contains("finished:"),
        "a timed-out wait must not claim the child finished: {}",
        timed_out.text
    );
    // A live delegation has no `finished`, so its elapsed time is measured against the wall
    // clock and cannot be pinned; everything around it can.
    assert!(
        timed_out.text.starts_with(&format!(
            "[fleet subagent {} still running after ",
            delegation.id
        )),
        "{}",
        timed_out.text
    );
    assert!(
        timed_out
            .text
            .ends_with(&format!(", status: running, thread: {}]", delegation.child)),
        "{}",
        timed_out.text
    );
    assert_eq!(timed_out.text.lines().count(), 1);

    let waited = subagents::execute(
        &client,
        SubagentCommand::Wait(SubagentWaitArgs {
            id: delegation.id,
            timeout: 2,
            json: false,
        }),
        &environment,
    )
    .await
    .unwrap();
    assert_eq!(waited.exit_code, 0);
    assert!(waited.text.contains("finished: succeeded"));
    assert!(waited.text.contains("duration: 14m 02s, files changed: 2"));
    assert!(waited.text.ends_with("verified"));

    let status = subagents::execute(
        &client,
        SubagentCommand::Status(SubagentIdArgs {
            id: delegation.id,
            json: false,
        }),
        &environment,
    )
    .await
    .unwrap();
    assert_eq!(
        status.text,
        format!(
            "{}\tsucceeded\tcodex\t{}\t14m 02s\tpending",
            delegation.id, delegation.child
        )
    );

    let listed = subagents::execute(
        &client,
        SubagentCommand::List(SubagentListArgs {
            caller: Some(delegation.caller),
            json: true,
        }),
        &environment,
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&listed.text).unwrap(),
        serde_json::json!({"protocol": 1, "delegations": [delegation]})
    );

    let cancelled = subagents::execute(
        &client,
        SubagentCommand::Cancel(SubagentIdArgs {
            id: delegation.id,
            json: false,
        }),
        &environment,
    )
    .await
    .unwrap();
    assert_eq!(cancelled.text, "cancelled");
    server.await.unwrap();
}

#[tokio::test]
async fn subagent_wait_json_bytes_are_unchanged_for_a_live_delegation() {
    use crate::args::{SubagentCommand, SubagentWaitArgs};

    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let delegation = sample_delegation();
    let expected = delegation.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        assert_eq!(
            request.body,
            RequestBody::DelegationWait {
                delegation: expected.id,
                timeout_ms: 1_000,
            }
        );
        let mut answer = expected;
        answer.status = fleet_core::agents::DelegationStatus::Running;
        answer.finished = None;
        answer.result = None;
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::Delegation(answer)),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let timed_out = subagents::execute(
        &client,
        SubagentCommand::Wait(SubagentWaitArgs {
            id: delegation.id,
            timeout: 1,
            json: true,
        }),
        &subagents::Environment::default(),
    )
    .await
    .unwrap();

    // The whole envelope, byte for byte. Rewording the human timeout line must not move a
    // comma here: the JSON branch is what other programs parse, and it already carries the
    // status they need to tell a timeout from a finished child.
    assert_eq!(
        timed_out.text,
        concat!(
            r#"{"protocol":1,"delegation":{"#,
            r#""id":"00000000-0000-4000-8000-000000000003","#,
            r#""caller":"00000000-0000-4000-8000-000000000001","#,
            r#""callerTurn":"00000000-0000-4000-8000-000000000004","#,
            r#""callerItem":"00000000-0000-4000-8000-000000000005","#,
            r#""child":"00000000-0000-4000-8000-000000000002","#,
            r#""provider":"codex","depth":1,"brief":"inspect the parser","#,
            r#""expectation":"tests pass","eager":true,"status":"running","#,
            r#""nudges":0,"recoveries":0,"delivery":{"type":"pending"},"#,
            r#""created":"2026-09-18T12:00:00Z"}}"#,
        )
    );
    assert_eq!(timed_out.exit_code, 2);
    server.await.unwrap();
}

/// `--effort` with no `--model` reaches the wire as the empty-model sentinel.
///
/// The regression this pins is a refusal, not a crash: the CLI used to reject the pairing
/// outright, so an orchestrator could not ask for a high-effort child without also pinning a
/// model id it had no reason to know.
#[tokio::test]
async fn subagent_run_sends_an_effort_without_a_model_as_the_default_model_sentinel() {
    use crate::args::{AgentChoice, SubagentCommand, SubagentRunArgs};
    use fleet_core::agents::ModelSelection;

    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let brief_file = home.path().join("brief.md");
    std::fs::write(&brief_file, "inspect the parser").unwrap();

    let delegation = sample_delegation();
    let expected = delegation.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = next_request(&mut transport).await;
        let RequestBody::DelegationRun { model, .. } = &request.body else {
            panic!("expected a delegation run request, got {:?}", request.body);
        };
        assert_eq!(
            model.clone(),
            Some(ModelSelection {
                model: String::new(),
                effort: Some("high".to_owned()),
                provider: None,
            })
        );
        send_result(
            &mut transport,
            request.id,
            Ok(ResponseBody::DelegationStarted {
                delegation: expected,
                warning: None,
            }),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    subagents::execute(
        &client,
        SubagentCommand::Run(SubagentRunArgs {
            provider: AgentChoice::Codex,
            brief_file: Some(brief_file),
            expectation: "tests pass".to_owned(),
            worktree: None,
            mode: None,
            model: None,
            effort: Some("high".to_owned()),
            title: None,
            eager: false,
            caller: Some(delegation.caller),
            json: false,
        }),
        &subagents::Environment::default(),
    )
    .await
    .unwrap();
    server.await.unwrap();
}

fn sample_delegation() -> fleet_core::agents::Delegation {
    serde_json::from_value(serde_json::json!({
        "id": "00000000-0000-4000-8000-000000000003",
        "caller": "00000000-0000-4000-8000-000000000001",
        "callerTurn": "00000000-0000-4000-8000-000000000004",
        "callerItem": "00000000-0000-4000-8000-000000000005",
        "child": "00000000-0000-4000-8000-000000000002",
        "provider": "codex",
        "depth": 1,
        "brief": "inspect the parser",
        "expectation": "tests pass",
        "eager": true,
        "status": "succeeded",
        "result": {
            "text": "verified",
            "filesChanged": ["src/parser.rs", "src/tests.rs"],
            "source": "reported"
        },
        "nudges": 0,
        "recoveries": 0,
        "delivery": {"type": "pending"},
        "created": "2026-09-18T12:00:00Z",
        "finished": "2026-09-18T12:14:02Z"
    }))
    .unwrap()
}

async fn bind(home: &Path) -> UnixListener {
    UnixListener::bind(home.join("fleetd.sock")).unwrap()
}

async fn authenticate(transport: &mut ServerTransport) {
    let hello = next_request(transport).await;
    assert!(matches!(
        hello.body,
        RequestBody::Hello {
            protocol: PROTOCOL_VERSION,
            ..
        }
    ));
    send_result(
        transport,
        hello.id,
        Ok(ResponseBody::Hello {
            protocol: PROTOCOL_VERSION,
            server: "test-daemon".to_owned(),
        }),
    )
    .await;
    let subscribe = next_request(transport).await;
    assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
    send_result(transport, subscribe.id, Ok(ResponseBody::Ack)).await;
}

async fn next_request(transport: &mut ServerTransport) -> Request {
    transport.next().await.expect("connection closed").unwrap()
}

async fn send_result(
    transport: &mut ServerTransport,
    id: u64,
    result: Result<ResponseBody, ProtoError>,
) {
    let response = serde_json::to_value(Response { id, result }).unwrap();
    transport.send(response).await.unwrap();
}

async fn send_event(transport: &mut ServerTransport, event: Event) {
    transport
        .send(serde_json::to_value(event).unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn job_wait_uses_one_baseline_and_events_even_when_completion_precedes_the_reply() {
    for before_reply in [false, true] {
        let home = TempDir::new().unwrap();
        let listener = bind(home.path()).await;
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut transport = Framed::new(socket, FleetCodec::new());
            authenticate(&mut transport).await;
            let update = next_request(&mut transport).await;
            assert_eq!(update.body, RequestBody::Update);
            send_result(
                &mut transport,
                update.id,
                Ok(ResponseBody::Job(update_job(JobStatus::Queued))),
            )
            .await;
            let baseline = next_request(&mut transport).await;
            assert_eq!(baseline.body, RequestBody::ListJobs);
            if before_reply {
                send_event(
                    &mut transport,
                    Event::JobUpdated(update_job(JobStatus::Succeeded)),
                )
                .await;
            }
            send_result(
                &mut transport,
                baseline.id,
                Ok(ResponseBody::Jobs(vec![update_job(JobStatus::Running)])),
            )
            .await;
            if !before_reply {
                assert!(
                    tokio::time::timeout(Duration::from_millis(350), transport.next())
                        .await
                        .is_err()
                );
                send_event(
                    &mut transport,
                    Event::JobUpdated(update_job(JobStatus::Succeeded)),
                )
                .await;
            }
        });
        let client = Client::connect(home.path()).await.unwrap();
        let output =
            tokio::time::timeout(Duration::from_secs(3), execute(&client, Command::Update))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            output,
            CommandOutput::with_exit_code("Updated update-1".into(), 75)
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn job_wait_reconciles_a_broadcast_gap() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let baseline = next_request(&mut transport).await;
        assert_eq!(baseline.body, RequestBody::ListJobs);
        for _ in 0..1100 {
            send_event(
                &mut transport,
                Event::JobUpdated(update_job(JobStatus::Running)),
            )
            .await;
        }
        send_result(
            &mut transport,
            baseline.id,
            Ok(ResponseBody::Jobs(vec![update_job(JobStatus::Running)])),
        )
        .await;
        let reconcile = next_request(&mut transport).await;
        assert_eq!(reconcile.body, RequestBody::ListJobs);
        send_result(
            &mut transport,
            reconcile.id,
            Ok(ResponseBody::Jobs(vec![update_job(JobStatus::Succeeded)])),
        )
        .await;
    });
    let client = Client::connect(home.path()).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        super::jobs::wait_for_job(&client, &update_job(JobStatus::Queued), "update"),
    )
    .await
    .unwrap()
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn watch_tail_reconciles_sequence_gaps_and_deduplicates_baseline_events() {
    use fleet_core::watches::WatchChunk;
    use fleet_proto::watch::WatchTail;

    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let initial = next_request(&mut transport).await;
        assert_eq!(
            initial.body,
            RequestBody::TailWatch {
                watch: sample_watch().id,
                from_seq: None
            }
        );
        let chunk = |seq, text: &str| WatchChunk {
            seq,
            stream: WatchStream::Stdout,
            text: text.into(),
        };
        send_event(
            &mut transport,
            Event::WatchOutput {
                watch: sample_watch().id,
                chunks: vec![chunk(4, "baseline"), chunk(5, "live"), chunk(7, "gap")],
            },
        )
        .await;
        send_result(
            &mut transport,
            initial.id,
            Ok(ResponseBody::WatchTail(WatchTail {
                watch: sample_watch(),
                chunks: vec![chunk(4, "baseline")],
                first_retained_seq: 4,
                next_seq: 5,
            })),
        )
        .await;
        let reconcile = next_request(&mut transport).await;
        assert_eq!(
            reconcile.body,
            RequestBody::TailWatch {
                watch: sample_watch().id,
                from_seq: Some(6)
            }
        );
        let mut watch = sample_watch();
        watch.status = WatchStatus::Exited {
            code: Some(0),
            signal: None,
        };
        send_result(
            &mut transport,
            reconcile.id,
            Ok(ResponseBody::WatchTail(WatchTail {
                watch,
                chunks: vec![chunk(6, "missing"), chunk(7, "gap")],
                first_retained_seq: 4,
                next_seq: 8,
            })),
        )
        .await;
    });
    let client = Client::connect(home.path()).await.unwrap();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    tokio::time::timeout(
        Duration::from_secs(3),
        watch_tail_to(
            &client,
            WatchTailArgs {
                id: sample_watch().id,
                follow: true,
            },
            &mut stdout,
            &mut stderr,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stdout, b"baselinelivemissinggap");
    assert!(stderr.is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn create_waits_for_clone_snapshot_without_polling_and_preserves_the_envelope() {
    use fleet_core::model::{Context, Repo, RepoHooks, Worktree};
    use fleet_proto::snapshot::{DaemonInfo, Snapshot};

    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let context = Context {
        id: "acme".parse().unwrap(),
        name: "Acme".into(),
        owners: vec!["acme".into()],
        created_at: "now".into(),
    };
    let repo = Repo {
        id: "acme/api".parse().unwrap(),
        owner: "acme".into(),
        name: "api".into(),
        url: "https://example.test/acme/api".into(),
        context_id: context.id.clone(),
        default_branch: "main".into(),
        path: "/repo".into(),
        cloned_at: "now".into(),
        hooks: RepoHooks::default(),
    };
    let worktree = Worktree {
        id: "acme/api#feature".parse().unwrap(),
        repo_id: repo.id.clone(),
        slug: "feature".into(),
        branch: "feature".into(),
        base_ref: "origin/main".into(),
        path: "/worktree".into(),
        session: "api/feature".into(),
        host: None,
        created_at: "now".into(),
        last_opened_at: None,
        degraded: None,
    };
    let expected = worktree.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let config = next_request(&mut transport).await;
        assert_eq!(config.body, RequestBody::GetConfig);
        send_result(
            &mut transport,
            config.id,
            Ok(ResponseBody::Config(fleet_core::config::default_config(
                "/fleet",
            ))),
        )
        .await;
        let mut snapshot = Snapshot {
            boards: Vec::new(),
            generated_at: "now".into(),
            revision: None,
            contexts: vec![context],
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: Vec::new(),
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: DaemonInfo {
                version: "test".into(),
                pid: 1,
                started_at: "now".into(),
                home: "/fleet".into(),
            },
        };
        let initial = next_request(&mut transport).await;
        assert_eq!(initial.body, RequestBody::GetSnapshot);
        send_result(
            &mut transport,
            initial.id,
            Ok(ResponseBody::Snapshot(snapshot.clone())),
        )
        .await;
        let clone = next_request(&mut transport).await;
        assert!(matches!(clone.body, RequestBody::CloneRepo { .. }));
        let mut job = update_job(JobStatus::Running);
        job.kind = JobKind::Clone;
        send_result(
            &mut transport,
            clone.id,
            Ok(ResponseBody::CloneStarted(job)),
        )
        .await;
        let baseline = next_request(&mut transport).await;
        assert_eq!(baseline.body, RequestBody::GetSnapshot);
        send_result(
            &mut transport,
            baseline.id,
            Ok(ResponseBody::Snapshot(snapshot.clone())),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(350), transport.next())
                .await
                .is_err()
        );
        snapshot.repos.push(repo);
        send_event(&mut transport, Event::SnapshotChanged(snapshot)).await;
        let create = next_request(&mut transport).await;
        assert!(
            matches!(create.body, RequestBody::CreateWorktree { ref base, .. } if base.as_deref() == Some("origin/main"))
        );
        send_result(
            &mut transport,
            create.id,
            Ok(ResponseBody::Worktree {
                created: true,
                worktree,
                post_create_job: None,
            }),
        )
        .await;
    });
    let client = Client::connect(home.path()).await.unwrap();
    let command = Cli::try_parse_from([
        "fleet",
        "create",
        "acme/api",
        "feature",
        "--url",
        "https://example.test/acme/api",
        "--json",
    ])
    .unwrap()
    .command
    .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(3), execute(&client, command))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
        serde_json::json!({ "protocol": 1, "created": true, "worktree": expected })
    );
    server.await.unwrap();
}

#[tokio::test]
async fn create_uses_default_remote_host_without_ensuring_the_repo_locally() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let repo = sample_repo();
    let mut worktree = sample_worktree(Some("dev-box"));
    worktree.session = "dev-box/api/feature".into();
    let expected = worktree.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;

        let config_request = next_request(&mut transport).await;
        assert_eq!(config_request.body, RequestBody::GetConfig);
        let mut config = fleet_core::config::default_config("/fleet");
        let host = "dev-box".parse().unwrap();
        config.default_host = "dev-box".into();
        config.hosts.insert(
            host,
            fleet_core::model::HostConfigEntry::Command {
                run: vec!["remote".into()],
                fleetd: "fleetd".into(),
                fleet_home: Some("/remote/fleet".into()),
                display: Some("dev-box".into()),
            },
        );
        send_result(
            &mut transport,
            config_request.id,
            Ok(ResponseBody::Config(config)),
        )
        .await;

        let snapshot = next_request(&mut transport).await;
        assert_eq!(snapshot.body, RequestBody::GetSnapshot);
        send_result(
            &mut transport,
            snapshot.id,
            Ok(ResponseBody::Snapshot(sample_snapshot(
                vec![repo],
                Vec::new(),
                Vec::new(),
            ))),
        )
        .await;

        let create = next_request(&mut transport).await;
        let RequestBody::CreateWorktree {
            repo,
            slug,
            host,
            base,
            ..
        } = create.body
        else {
            panic!("expected create worktree request");
        };
        assert_eq!(repo.as_str(), "acme/api");
        assert_eq!(slug, "feature");
        assert_eq!(host.unwrap().as_str(), "dev-box");
        assert_eq!(base.as_deref(), Some("origin/main"));
        send_result(
            &mut transport,
            create.id,
            Ok(ResponseBody::Worktree {
                created: true,
                worktree,
                post_create_job: None,
            }),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let command = Cli::try_parse_from(["fleet", "create", "acme/api", "feature", "--json"])
        .unwrap()
        .command
        .unwrap();
    let output = execute(&client, command).await.unwrap();

    assert_eq!(output.exit_code, 0);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.text).unwrap(),
        serde_json::json!({ "protocol": 1, "created": true, "worktree": expected })
    );
    server.await.unwrap();
}
