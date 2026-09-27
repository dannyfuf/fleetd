use std::sync::{Arc, Mutex};

use fleet_core::{
    ids::TerminalId,
    sessions::{Session, SessionKind, Terminal, TerminalKind, TerminalStatus},
};
use fleet_proto::{
    media::MAX_ACTIVE_UPLOADS_PER_OWNER,
    request::{StageOp, StagedFile},
    snapshot::{DaemonInfo, HostStatus},
};
use gpui::{AppContext as _, TestAppContext};

use std::{collections::VecDeque, fs::File};

use fleet_proto::{
    error::ErrorKind,
    media::{CHUNK_BYTES, MAX_UPLOAD_BYTES, MAX_UPLOAD_FILES},
    request::StageEntry,
};

use super::*;
use super::{
    driver::{INITIAL_LINK_WAIT, LINK_STATE_SETTLE},
    prepare::{PreparedItem, PreparedUpload, entry_name, validate_gesture_limits},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FakeMode {
    Automatic,
    BeginNotFound,
    FailBadNames,
    HoldBegin,
    HoldChunks,
    HoldFinish,
    NotFoundOnce,
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    anchor: MediaAnchor,
    upload: UploadId,
    op: StageOp,
}

struct HeldChunk {
    sender: Sender<Result<ResponseBody, ProtoError>>,
    offset: u64,
}

struct FakeState {
    mode: FakeMode,
    requests: Vec<RecordedRequest>,
    held_begins: VecDeque<Sender<Result<ResponseBody, ProtoError>>>,
    held: VecDeque<HeldChunk>,
    held_finish: Option<Sender<Result<ResponseBody, ProtoError>>>,
    names: HashMap<UploadId, String>,
    not_found_sent: bool,
    in_flight: usize,
    max_in_flight: usize,
}

#[derive(Clone)]
struct FakeBridge {
    inner: Arc<Mutex<FakeState>>,
}

impl FakeBridge {
    fn new(mode: FakeMode) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeState {
                mode,
                requests: Vec::new(),
                held_begins: VecDeque::new(),
                held: VecDeque::new(),
                held_finish: None,
                names: HashMap::new(),
                not_found_sent: false,
                in_flight: 0,
                max_in_flight: 0,
            })),
        }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .requests
            .clone()
    }

    fn held_len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .held
            .len()
    }

    fn held_begin_len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .held_begins
            .len()
    }

    fn max_in_flight(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .max_in_flight
    }

    fn set_mode(&self, mode: FakeMode) {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .mode = mode;
    }

    fn acknowledge_front(&self) {
        let held = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let held = inner.held.pop_front();
            if held.is_some() {
                inner.in_flight = inner.in_flight.saturating_sub(1);
            }
            held
        };
        if let Some(held) = held {
            send_fake(&held.sender, Ok(ResponseBody::Ack));
        }
    }

    fn acknowledge_held_at(&self, index: usize) -> Option<u64> {
        let held = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let held = inner.held.remove(index);
            if held.is_some() {
                inner.in_flight = inner.in_flight.saturating_sub(1);
            }
            held
        }?;
        let offset = held.offset;
        send_fake(&held.sender, Ok(ResponseBody::Ack));
        Some(offset)
    }

    fn fail_front_remote(&self) {
        let held = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let held = inner.held.pop_front();
            if held.is_some() {
                inner.in_flight = inner.in_flight.saturating_sub(1);
            }
            held
        };
        if let Some(held) = held {
            send_fake(
                &held.sender,
                Err(ProtoError {
                    kind: ErrorKind::Remote,
                    message: "devbox link dropped".to_owned(),
                }),
            );
        }
    }

    fn fail_finish_remote(&self) {
        let sender = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .held_finish
            .take();
        if let Some(sender) = sender {
            send_fake(
                &sender,
                Err(ProtoError {
                    kind: ErrorKind::Remote,
                    message: "devbox link dropped".to_owned(),
                }),
            );
        }
    }

    fn discard_closed_held(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.held.retain(|held| !held.sender.is_closed());
        inner.in_flight = inner.held.len();
    }
}

impl MediaTransport for FakeBridge {
    fn stage_media(&self, anchor: MediaAnchor, upload: UploadId, op: MediaStageOp) -> StageReply {
        let (sender, receiver) = async_channel::bounded(1);
        let proto_op = match op.clone() {
            MediaStageOp::Begin { entry } => StageOp::Begin { entry },
            MediaStageOp::Chunk {
                file,
                offset,
                bytes,
            } => StageOp::Chunk {
                file,
                offset,
                data: fleet_proto::media::encode(&bytes),
            },
            MediaStageOp::Finish { sha256 } => StageOp::Finish { sha256 },
            MediaStageOp::Cancel => StageOp::Cancel,
        };
        let response = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.requests.push(RecordedRequest {
                anchor,
                upload,
                op: proto_op,
            });
            match op {
                MediaStageOp::Begin { entry } => {
                    inner.names.insert(upload, entry_name(&entry).to_owned());
                    match inner.mode {
                        FakeMode::BeginNotFound => Some(Err(ProtoError {
                            kind: ErrorKind::NotFound,
                            message: "terminal 7 is not available".to_owned(),
                        })),
                        FakeMode::HoldBegin => {
                            inner.held_begins.push_back(sender.clone());
                            None
                        }
                        FakeMode::Automatic
                        | FakeMode::FailBadNames
                        | FakeMode::HoldChunks
                        | FakeMode::HoldFinish
                        | FakeMode::NotFoundOnce => Some(Ok(ResponseBody::Ack)),
                    }
                }
                MediaStageOp::Chunk { offset, .. } => {
                    inner.in_flight = inner.in_flight.saturating_add(1);
                    inner.max_in_flight = inner.max_in_flight.max(inner.in_flight);
                    match inner.mode {
                        FakeMode::HoldChunks => {
                            inner.held.push_back(HeldChunk {
                                sender: sender.clone(),
                                offset,
                            });
                            None
                        }
                        FakeMode::NotFoundOnce if !inner.not_found_sent => {
                            inner.not_found_sent = true;
                            inner.in_flight = inner.in_flight.saturating_sub(1);
                            Some(Err(ProtoError {
                                kind: ErrorKind::NotFound,
                                message: "upload expired".to_owned(),
                            }))
                        }
                        FakeMode::Automatic
                        | FakeMode::BeginNotFound
                        | FakeMode::FailBadNames
                        | FakeMode::HoldBegin
                        | FakeMode::HoldFinish
                        | FakeMode::NotFoundOnce => {
                            inner.in_flight = inner.in_flight.saturating_sub(1);
                            Some(Ok(ResponseBody::Ack))
                        }
                    }
                }
                MediaStageOp::Finish { .. } => {
                    let name = inner
                        .names
                        .get(&upload)
                        .cloned()
                        .unwrap_or_else(|| "paste".to_owned());
                    match inner.mode {
                        FakeMode::HoldFinish => {
                            inner.held_finish = Some(sender.clone());
                            None
                        }
                        FakeMode::FailBadNames if name.starts_with("bad") => {
                            Some(Err(ProtoError {
                                kind: ErrorKind::Validation,
                                message: "receiver rejected the file".to_owned(),
                            }))
                        }
                        FakeMode::Automatic
                        | FakeMode::BeginNotFound
                        | FakeMode::FailBadNames
                        | FakeMode::HoldBegin
                        | FakeMode::HoldChunks
                        | FakeMode::NotFoundOnce => Some(Ok(ResponseBody::Path {
                            path: format!("/staged/{name}"),
                            host: None,
                        })),
                    }
                }
                MediaStageOp::Cancel => Some(Ok(ResponseBody::Ack)),
            }
        };
        if let Some(response) = response {
            send_fake(&sender, response);
        }
        receiver
    }
}

#[track_caller]
fn send_fake(
    sender: &Sender<Result<ResponseBody, ProtoError>>,
    response: Result<ResponseBody, ProtoError>,
) {
    match sender.try_send(response) {
        Ok(()) | Err(async_channel::TrySendError::Closed(_)) => {}
        Err(async_channel::TrySendError::Full(_)) => {
            panic!("fake media reply channel was unexpectedly full")
        }
    }
}

fn snapshot(host: Option<LinkState>, terminal_host: bool) -> Snapshot {
    let host_id = host_id();
    Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-24T12:00:00Z".to_owned(),
        revision: None,
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: None,
        sessions: vec![Session {
            id: "media/session"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            host: terminal_host.then_some(host_id.clone()),
            kind: SessionKind::Agent(fleet_core::config::Agent::Claude),
            cwd: "/tmp".to_owned(),
            terminals: vec![Terminal {
                id: TerminalId(7),
                name: "media".to_owned(),
                command: "shell".to_owned(),
                cwd: "/tmp".to_owned(),
                shell_pid: None,
                foreground_command: None,
                status: TerminalStatus::Running,
                title: None,
                keep_alive: Vec::new(),
                has_unseen_output: false,
                agent_attention: None,
                kind: TerminalKind::Pty,
            }],
            active_terminal: Some(TerminalId(7)),
            slept_at: None,
            kept_terminals: Vec::new(),
        }],
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: host
            .map(|link| HostStatus {
                id: host_id,
                provider: "tailscale".to_owned(),
                version: Some("0.1.0".to_owned()),
                link,
                address: None,
                agent_binaries: None,
                reachable: link == LinkState::Ready,
                checked_at: "2026-09-24T12:00:00Z".to_owned(),
                error: None,
            })
            .into_iter()
            .collect(),
        jobs: Vec::new(),
        daemon: DaemonInfo {
            version: "0.1.0".to_owned(),
            pid: 42,
            started_at: "2026-09-24T09:00:00Z".to_owned(),
            home: "/tmp/fleet".to_owned(),
        },
    }
}

fn host_id() -> HostId {
    "devbox".parse().unwrap_or_else(|error| panic!("{error}"))
}

fn state(cx: &mut TestAppContext, host: Option<LinkState>) -> Entity<AppState> {
    let now = Instant::now();
    let terminal_host = host.is_some();
    cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet-media-test", now);
        app.apply_snapshot(snapshot(host, terminal_host), now);
        app
    })
}

fn drain_ready(cx: &TestAppContext) {
    while cx.executor().tick() {}
}

fn recorded_paths() -> Arc<Mutex<Vec<PathBuf>>> {
    Arc::new(Mutex::new(Vec::new()))
}

#[gpui::test]
fn local_paths_short_circuit_without_a_request(cx: &mut TestAppContext) {
    let state = state(cx, None);
    let bridge = FakeBridge::new(FakeMode::Automatic);
    let paths = vec![PathBuf::from("/tmp/original image.png")];
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Terminal {
                terminal: TerminalId(7),
            },
            Attachment::Paths(paths.clone()),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });

    assert!(bridge.requests().is_empty());
    assert_eq!(
        *received.lock().unwrap_or_else(|error| error.into_inner()),
        paths
    );
}

#[gpui::test]
fn multi_chunk_upload_obeys_the_host_window_and_advances_progress(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("large.bin");
    std::fs::write(&path, vec![7_u8; CHUNK_BYTES * (UPLOAD_WINDOW + 1)])
        .unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::HoldChunks);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    drain_ready(cx);
    assert_eq!(bridge.held_len(), UPLOAD_WINDOW);
    assert_eq!(bridge.max_in_flight(), UPLOAD_WINDOW);

    let upload = bridge
        .requests()
        .iter()
        .find_map(|request| matches!(request.op, StageOp::Begin { .. }).then_some(request.upload))
        .unwrap_or_else(|| panic!("the fake observed Begin"));
    bridge.acknowledge_front();
    drain_ready(cx);
    state.read_with(cx, |app, _| {
        let record = app
            .media_uploads
            .record(upload)
            .unwrap_or_else(|| panic!("the upload remains live"));
        assert!(record.sent > 0, "an acknowledged chunk advances progress");
    });

    bridge.set_mode(FakeMode::Automatic);
    for _ in 0..UPLOAD_WINDOW {
        bridge.acknowledge_front();
    }
    cx.run_until_parked();
    assert!(bridge.max_in_flight() <= UPLOAD_WINDOW);
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [PathBuf::from("/staged/large.bin")]
    );
}

#[gpui::test]
fn a_folder_sends_its_manifest_chunks_and_digests(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let root = temporary.path().join("folder");
    std::fs::create_dir_all(root.join("empty")).unwrap_or_else(|error| panic!("{error}"));
    std::fs::create_dir_all(root.join("nested")).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(root.join("a.txt"), b"alpha").unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(root.join("nested/b.txt"), b"beta").unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::Automatic);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![root]),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    cx.run_until_parked();

    let requests = bridge.requests();
    assert!(matches!(
        &requests[0].op,
        StageOp::Begin {
            entry: StageEntry::Directory { files, dirs, .. }
        } if files == &vec![
            StagedFile { relative: "a.txt".to_owned(), size: 5 },
            StagedFile { relative: "nested/b.txt".to_owned(), size: 4 },
        ] && dirs == &vec!["empty".to_owned(), "nested".to_owned()]
    ));
    assert!(requests.iter().any(|request| matches!(
        &request.op,
        StageOp::Finish { sha256 } if sha256.len() == 2
    )));
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [PathBuf::from("/staged/folder")]
    );
}

#[gpui::test]
fn a_link_blip_resends_only_chunks_without_an_acknowledgement(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("blip.bin");
    std::fs::write(&path, vec![3_u8; CHUNK_BYTES * UPLOAD_WINDOW])
        .unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::HoldChunks);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    drain_ready(cx);
    let acknowledged_offset = bridge
        .acknowledge_held_at(1)
        .unwrap_or_else(|| panic!("the second chunk was in flight"));
    cx.update(|cx| {
        state.update(cx, |app, _| {
            app.apply_host_link(&host_id(), LinkState::Down, None, Some("blip".to_owned()));
        });
    });
    bridge.fail_front_remote();
    drain_ready(cx);
    cx.executor().advance_clock(LINK_STATE_SETTLE);
    drain_ready(cx);
    bridge.discard_closed_held();
    bridge.set_mode(FakeMode::Automatic);
    cx.update(|cx| {
        state.update(cx, |app, _| {
            app.apply_host_link(&host_id(), LinkState::Ready, None, None);
        });
    });
    cx.run_until_parked();

    let offsets = bridge
        .requests()
        .into_iter()
        .filter_map(|request| match request.op {
            StageOp::Chunk { offset, .. } => Some(offset),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        offsets
            .iter()
            .filter(|offset| **offset == acknowledged_offset)
            .count(),
        1,
        "an out-of-order acknowledgement must not be resent"
    );
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [PathBuf::from("/staged/blip.bin")]
    );
}

#[gpui::test]
fn a_lost_finish_response_is_retried_after_the_link_recovers(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("finish.bin");
    std::fs::write(&path, b"finish").unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::HoldFinish);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    drain_ready(cx);
    assert_eq!(
        bridge
            .requests()
            .iter()
            .filter(|request| matches!(request.op, StageOp::Finish { .. }))
            .count(),
        1
    );

    cx.update(|cx| {
        state.update(cx, |app, _| {
            app.apply_host_link(&host_id(), LinkState::Down, None, Some("blip".to_owned()));
        });
    });
    bridge.fail_finish_remote();
    drain_ready(cx);
    cx.executor().advance_clock(LINK_STATE_SETTLE);
    drain_ready(cx);
    bridge.set_mode(FakeMode::Automatic);
    cx.update(|cx| {
        state.update(cx, |app, _| {
            app.apply_host_link(&host_id(), LinkState::Ready, None, None);
        });
    });
    cx.run_until_parked();

    let requests = bridge.requests();
    let finishes = requests
        .iter()
        .filter(|request| matches!(request.op, StageOp::Finish { .. }))
        .collect::<Vec<_>>();
    assert_eq!(finishes.len(), 2);
    assert_eq!(finishes[0].upload, finishes[1].upload);
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [PathBuf::from("/staged/finish.bin")]
    );
}

#[gpui::test]
fn not_found_restarts_once_with_the_stable_upload_id(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("restart.bin");
    std::fs::write(&path, b"restart").unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::NotFoundOnce);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    cx.run_until_parked();

    let begins = bridge
        .requests()
        .into_iter()
        .filter_map(|request| matches!(request.op, StageOp::Begin { .. }).then_some(request.upload))
        .collect::<Vec<_>>();
    assert_eq!(begins.len(), 2);
    assert_eq!(begins[0], begins[1]);
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [PathBuf::from("/staged/restart.bin")]
    );
}

#[gpui::test]
fn stage_outcome_uses_the_returned_stable_handle_after_restart(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("handled-restart.bin");
    std::fs::write(&path, b"restart").unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::NotFoundOnce);
    let completed = Arc::new(Mutex::new(Vec::new()));
    let output = Arc::clone(&completed);

    let uploads = cx.update(|cx| {
        stage_with_outcomes(
            &state,
            bridge,
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            move |outcomes, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = outcomes;
            },
            cx,
        )
    });
    cx.run_until_parked();

    let mut completed = completed.lock().unwrap_or_else(|error| error.into_inner());
    let outcome = completed
        .pop()
        .unwrap_or_else(|| panic!("the outcome callback ran"));
    assert_eq!(uploads, vec![outcome.upload]);
    assert_eq!(
        outcome.result,
        Ok(PathBuf::from("/staged/handled-restart.bin"))
    );
}

#[gpui::test]
fn cancel_drops_the_task_and_sends_cancel(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("cancel.bin");
    std::fs::write(&path, vec![1_u8; CHUNK_BYTES * 2]).unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::HoldChunks);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            |_, _| {},
            cx,
        );
    });
    drain_ready(cx);
    let upload = bridge
        .requests()
        .iter()
        .find_map(|request| matches!(request.op, StageOp::Begin { .. }).then_some(request.upload))
        .unwrap_or_else(|| panic!("the fake observed Begin"));
    state.read_with(cx, |app, _| {
        let record = app
            .media_uploads
            .record(upload)
            .unwrap_or_else(|| panic!("the upload remains live"));
        let toast = app
            .toasts
            .iter()
            .find(|toast| toast.id == record.toast)
            .unwrap_or_else(|| panic!("the progress toast remains live"));
        assert_eq!(toast.target, Some(ToastTarget::MediaUpload(upload)));
        assert_eq!(toast.toast.action.as_deref(), Some("Cancel"));
    });
    bridge.set_mode(FakeMode::Automatic);
    cx.update(|cx| cancel_with(&state, bridge.clone(), upload, cx));
    cx.run_until_parked();

    state.read_with(cx, |app, _| {
        assert!(app.media_uploads.record(upload).is_none())
    });
    assert!(
        bridge
            .requests()
            .iter()
            .any(|request| { request.upload == upload && matches!(request.op, StageOp::Cancel) })
    );
}

#[gpui::test]
fn over_limit_is_refused_before_any_request(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("too-large.bin");
    File::create(&path)
        .and_then(|file| file.set_len(MAX_UPLOAD_BYTES + 1))
        .unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::Automatic);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            |_, _| panic!("an over-limit upload must not finish"),
            cx,
        );
    });
    cx.run_until_parked();

    assert!(bridge.requests().is_empty());
    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error
                .as_ref()
                .is_some_and(|error| error.text.contains("1 GiB"))
        );
    });
}

#[gpui::test]
fn one_bad_source_does_not_discard_successful_siblings(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let missing = temporary.path().join("missing.bin");
    let good = temporary.path().join("good.bin");
    std::fs::write(&good, b"good").unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::Automatic);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge,
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![missing, good]),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    cx.run_until_parked();

    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [PathBuf::from("/staged/good.bin")]
    );
    state.read_with(cx, |app, _| {
        assert!(app.sticky_error.as_ref().is_some_and(|error| {
            error.text.contains("missing.bin") && error.text.contains("inspect media path")
        }));
    });
}

#[gpui::test]
fn worker_failures_are_named_and_aggregated_in_source_order(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let paths = ["bad-one.bin", "good.bin", "bad-two.bin"]
        .into_iter()
        .map(|name| {
            let path = temporary.path().join(name);
            std::fs::write(&path, name).unwrap_or_else(|error| panic!("{error}"));
            path
        })
        .collect::<Vec<_>>();
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::FailBadNames);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge,
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(paths),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    cx.run_until_parked();

    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [PathBuf::from("/staged/good.bin")]
    );
    state.read_with(cx, |app, _| {
        let message = app
            .sticky_error
            .as_ref()
            .map(|error| error.text.as_ref())
            .unwrap_or("");
        let first = message
            .find("bad-one.bin")
            .unwrap_or_else(|| panic!("first failure is named: {message}"));
        let second = message
            .find("bad-two.bin")
            .unwrap_or_else(|| panic!("second failure is named: {message}"));
        assert!(first < second, "failures retain source order: {message}");
    });
}

#[gpui::test]
fn skipped_entry_warning_requires_a_successful_copy(cx: &mut TestAppContext) {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let root = temporary.path().join("bad-folder");
    std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(root.join("good.txt"), b"good").unwrap_or_else(|error| panic!("{error}"));
    symlink(root.join("good.txt"), root.join("skipped.txt"))
        .unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::FailBadNames);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge,
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![root]),
            false,
            |_, _| panic!("a rejected folder must not produce a path"),
            cx,
        );
    });
    cx.run_until_parked();

    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error
                .as_ref()
                .is_some_and(|error| { error.text.contains("receiver rejected the file") })
        );
        assert!(
            app.toasts
                .iter()
                .all(|toast| { !toast.toast.text.starts_with("Skipped ") })
        );
    });
}

#[test]
fn top_level_empty_directories_consume_the_aggregate_entry_budget() {
    let prepared = (0..=MAX_UPLOAD_FILES)
        .map(|index| PreparedItem {
            index,
            id: UploadId::new(),
            upload: PreparedUpload {
                name: format!("empty-{index}"),
                entry: StageEntry::Directory {
                    name: format!("empty-{index}"),
                    files: Vec::new(),
                    dirs: Vec::new(),
                },
                files: Vec::new(),
                total_bytes: 0,
                entry_count: 0,
                skipped: 0,
            },
        })
        .collect::<Vec<_>>();

    let error = validate_gesture_limits(&prepared)
        .expect_err("empty roots cannot bypass the aggregate entry limit");
    assert!(error.to_string().contains("10,000 files and folders"));
}

#[gpui::test]
fn an_initially_down_host_times_out_after_thirty_seconds(cx: &mut TestAppContext) {
    let state = state(cx, Some(LinkState::Down));
    let bridge = FakeBridge::new(FakeMode::Automatic);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Blob {
                name: "clipboard.png".to_owned(),
                format: gpui::ImageFormat::Png,
                bytes: vec![1, 2, 3],
            },
            false,
            |_, _| panic!("a timed-out upload must not finish"),
            cx,
        );
    });
    drain_ready(cx);
    cx.executor()
        .advance_clock(INITIAL_LINK_WAIT + Duration::from_secs(1));
    cx.run_until_parked();

    assert!(bridge.requests().is_empty());
    state.read_with(cx, |app, _| {
        assert!(app.sticky_error.as_ref().is_some_and(|error| {
            error.text.contains("Timed out") && error.text.contains("devbox")
        }));
    });
}

#[gpui::test]
fn concurrent_uploads_share_one_host_window(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let first = temporary.path().join("first.bin");
    let second = temporary.path().join("second.bin");
    std::fs::write(&first, vec![1_u8; CHUNK_BYTES * 3]).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(&second, vec![2_u8; CHUNK_BYTES * 3]).unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::HoldChunks);
    let received = recorded_paths();
    let output = Arc::clone(&received);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![first, second]),
            false,
            move |paths, _| {
                *output.lock().unwrap_or_else(|error| error.into_inner()) = paths;
            },
            cx,
        );
    });
    drain_ready(cx);
    assert_eq!(bridge.held_len(), UPLOAD_WINDOW);
    assert_eq!(bridge.max_in_flight(), UPLOAD_WINDOW);

    bridge.set_mode(FakeMode::Automatic);
    for _ in 0..UPLOAD_WINDOW {
        bridge.acknowledge_front();
    }
    cx.run_until_parked();
    assert_eq!(bridge.max_in_flight(), UPLOAD_WINDOW);
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        [
            PathBuf::from("/staged/first.bin"),
            PathBuf::from("/staged/second.bin"),
        ]
    );
}

#[gpui::test]
fn upload_workers_queue_before_the_receiver_owner_limit(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let paths = (0..MAX_ACTIVE_UPLOADS_PER_OWNER + 1)
        .map(|index| {
            let path = temporary.path().join(format!("queued-{index}.bin"));
            std::fs::write(&path, []).unwrap_or_else(|error| panic!("{error}"));
            path
        })
        .collect::<Vec<_>>();
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::HoldBegin);

    let uploads = cx.update(|cx| {
        stage_with_outcomes(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(paths),
            false,
            |_, _| {},
            cx,
        )
    });
    drain_ready(cx);

    assert_eq!(bridge.held_begin_len(), UPLOAD_CONCURRENCY);
    assert_eq!(uploads.len(), MAX_ACTIVE_UPLOADS_PER_OWNER + 1);
    state.read_with(cx, |app, _| {
        let waiting = uploads
            .iter()
            .filter(|upload| {
                progress(app, **upload)
                    .is_some_and(|progress| progress.state == UploadState::Waiting)
            })
            .count();
        assert_eq!(
            waiting,
            MAX_ACTIVE_UPLOADS_PER_OWNER + 1 - UPLOAD_CONCURRENCY
        );
    });
}

#[gpui::test]
fn begin_not_found_surfaces_the_anchor_error_without_restart(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("anchor-gone.bin");
    std::fs::write(&path, b"gone").unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::BeginNotFound);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Host { host: host_id() },
            Attachment::Paths(vec![path]),
            false,
            |_, _| panic!("an unavailable anchor must not produce a path"),
            cx,
        );
    });
    cx.run_until_parked();

    assert_eq!(
        bridge
            .requests()
            .iter()
            .filter(|request| matches!(request.op, StageOp::Begin { .. }))
            .count(),
        1
    );
    state.read_with(cx, |app, _| {
        let message = app
            .sticky_error
            .as_ref()
            .map(|error| error.text.as_ref())
            .unwrap_or("");
        assert!(message.contains("terminal 7 is not available"));
        assert!(!message.contains("expired twice"));
    });
}

#[gpui::test]
fn an_unknown_terminal_never_short_circuits_as_local(cx: &mut TestAppContext) {
    let state = state(cx, None);
    let bridge = FakeBridge::new(FakeMode::Automatic);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Terminal {
                terminal: TerminalId(99),
            },
            Attachment::Paths(vec![PathBuf::from("/tmp/local-only.bin")]),
            false,
            |_, _| panic!("an unknown terminal must not receive a local path"),
            cx,
        );
    });

    assert!(bridge.requests().is_empty());
    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error
                .as_ref()
                .is_some_and(|error| { error.text.contains("Terminal 99 is not available") })
        );
    });
}

#[test]
fn terminal_host_and_link_accessors_follow_the_snapshot_owner() {
    let now = Instant::now();
    let mut app = AppState::new("/tmp/fleet-media-test", now);
    app.apply_snapshot(snapshot(Some(LinkState::Connecting), true), now);

    assert_eq!(app.host_of_terminal(TerminalId(7)), Some(&host_id()));
    assert_eq!(app.host_link_state(&host_id()), Some(LinkState::Connecting));
    assert_eq!(app.host_of_terminal(TerminalId(99)), None);
}

#[test]
fn recorded_requests_keep_the_anchor_on_every_operation() {
    let bridge = FakeBridge::new(FakeMode::Automatic);
    let anchor = MediaAnchor::Host { host: host_id() };
    let upload = UploadId::new();
    let reply = bridge.stage_media(
        anchor.clone(),
        upload,
        MediaStageOp::Begin {
            entry: StageEntry::File {
                name: "one.png".to_owned(),
                size: 0,
            },
        },
    );
    drop(reply);

    assert_eq!(bridge.requests()[0].anchor, anchor);
}

#[gpui::test]
fn a_remote_terminal_resolves_to_a_stable_host_anchor_once(cx: &mut TestAppContext) {
    let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = temporary.path().join("stable-host.bin");
    std::fs::write(&path, b"stable").unwrap_or_else(|error| panic!("{error}"));
    let state = state(cx, Some(LinkState::Ready));
    let bridge = FakeBridge::new(FakeMode::Automatic);

    cx.update(|cx| {
        stage_with(
            &state,
            bridge.clone(),
            MediaAnchor::Terminal {
                terminal: TerminalId(7),
            },
            Attachment::Paths(vec![path]),
            false,
            |_, _| {},
            cx,
        );
    });
    cx.run_until_parked();

    assert!(
        bridge
            .requests()
            .iter()
            .all(|request| { request.anchor == MediaAnchor::Host { host: host_id() } })
    );
}
