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
