use super::{
    sessions::resolve_agent_status_target, watches::watch_tail_to, worktrees::parse_hooks,
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
            Ok(ResponseBody::Path(archived.display().to_string())),
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
        let mut snapshot = Snapshot {
            boards: Vec::new(),
            generated_at: "now".into(),
            contexts: vec![context],
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: Vec::new(),
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
