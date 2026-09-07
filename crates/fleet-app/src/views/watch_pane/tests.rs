use super::*;
use async_channel::Sender;
use fleet_core::{
    ids::TerminalId,
    watches::{Watch, WatchSource, WatchStatus},
};
use gpui::TestAppContext;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

struct PendingRequest {
    body: RequestBody,
    reply: Sender<Result<ResponseBody, ProtoError>>,
}

#[derive(Clone, Default)]
struct RequestHarness(Arc<Mutex<VecDeque<PendingRequest>>>);

impl RequestHarness {
    fn requests(&self) -> WatchRequests {
        let pending = Arc::clone(&self.0);
        WatchRequests(Arc::new(move |body| {
            let (reply, response) = async_channel::bounded(1);
            pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_back(PendingRequest { body, reply });
            response
        }))
    }

    fn len(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    fn respond(&self, response: Result<ResponseBody, ProtoError>) -> RequestBody {
        let request = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or_else(|| panic!("expected a pending watch request"));
        request
            .reply
            .try_send(response)
            .unwrap_or_else(|_| panic!("watch reply receiver should still be live"));
        request.body
    }
}

fn watch(status: WatchStatus) -> Watch {
    Watch {
        id: WatchId(7),
        session: "acme/api#fix"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        terminal: TerminalId(1),
        label: "codex".into(),
        command: vec!["codex".into()],
        cwd: None,
        pid: Some(123),
        started_at: "2026-09-04T12:00:00Z".into(),
        status,
        source: WatchSource::Cooperative,
        log_file: None,
    }
}

fn offline(message: &str) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: message.into(),
    }
}

#[gpui::test]
fn failed_list_synchronization_retries(cx: &mut TestAppContext) {
    let harness = RequestHarness::default();
    let state = cx.new(|_| AppState::new("/tmp/fleet-watch-list-retry", Instant::now()));
    let session = watch(WatchStatus::Running).session;
    state.update(cx, |app, _| {
        app.screen = crate::state::Screen::Workspace {
            session: session.clone(),
        };
        app.snapshot = Some(fleet_proto::snapshot::Snapshot {
            boards: Vec::new(),
            generated_at: "2026-09-04T12:00:00Z".into(),
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: vec![fleet_core::sessions::Session {
                id: session,
                kind: fleet_core::sessions::SessionKind::Worktree(
                    "acme/api#fix"
                        .parse()
                        .unwrap_or_else(|error| panic!("{error}")),
                ),
                cwd: "/tmp".into(),
                terminals: Vec::new(),
                active_terminal: None,
                slept_at: None,
                kept_terminals: Vec::new(),
            }],
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "0.1.0".into(),
                pid: 42,
                started_at: "2026-09-04T12:00:00Z".into(),
                home: "/tmp/fleet".into(),
            },
        });
    });
    let _controller = cx.new(|cx| WatchController::new(&state, harness.requests(), cx));

    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Err(offline("list unavailable"))),
        RequestBody::ListWatches { .. }
    ));
    cx.run_until_parked();
    assert_eq!(harness.len(), 0, "failure must not retry without a delay");
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Err(offline("list still unavailable"))),
        RequestBody::ListWatches { .. }
    ));
    cx.run_until_parked();
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
    cx.run_until_parked();
    assert_eq!(
        harness.len(),
        0,
        "the second retry must use exponential backoff"
    );
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Err(offline("list remains unavailable"))),
        RequestBody::ListWatches { .. }
    ));
    cx.run_until_parked();
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY * 3);
    cx.run_until_parked();
    assert_eq!(harness.len(), 0, "the third retry waits four seconds");
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Err(offline("list permanently unavailable"))),
        RequestBody::ListWatches { .. }
    ));
    cx.run_until_parked();
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY * 60);
    cx.run_until_parked();
    assert_eq!(harness.len(), 0, "four failed attempts exhaust recovery");

    state.update(cx, |app, cx| {
        app.watches.invalidate();
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(harness.len(), 1, "invalidation resets the retry budget");
    assert!(matches!(
        harness.respond(Ok(ResponseBody::Watches(Vec::new()))),
        RequestBody::ListWatches { .. }
    ));
    cx.run_until_parked();
    assert_eq!(
        harness.len(),
        0,
        "acknowledgement completes synchronization"
    );
}

#[gpui::test]
fn failed_completed_tail_retries(cx: &mut TestAppContext) {
    let harness = RequestHarness::default();
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet-watch-tail-retry", Instant::now());
        let completed = watch(WatchStatus::Exited {
            code: Some(0),
            signal: None,
        });
        app.watches.exited(completed, Instant::now());
        app
    });
    let _controller = cx.new(|cx| WatchController::new(&state, harness.requests(), cx));

    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Err(offline("tail unavailable"))),
        RequestBody::TailWatch { .. }
    ));
    cx.run_until_parked();
    assert_eq!(harness.len(), 0, "failure must not retry without a delay");
    state.read_with(cx, |app, _| {
        assert_eq!(
            app.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("could not recover watch output (watch 7): tail unavailable")
        );
    });
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Ok(ResponseBody::WatchTail(fleet_proto::watch::WatchTail {
            watch: watch(WatchStatus::Exited {
                code: Some(0),
                signal: None,
            }),
            chunks: Vec::new(),
            first_retained_seq: 0,
            next_seq: 0,
        }))),
        RequestBody::TailWatch { .. }
    ));
    cx.run_until_parked();
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
    cx.run_until_parked();
    assert_eq!(harness.len(), 0, "successful recovery clears retry intent");
    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error.is_none(),
            "recovery clears its stale error"
        );
    });
}

#[gpui::test]
fn tail_recovery_is_bounded_and_surfaces_failure_once(cx: &mut TestAppContext) {
    let harness = RequestHarness::default();
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet-watch-tail-bounded", Instant::now());
        app.watches.exited(
            watch(WatchStatus::Exited {
                code: Some(0),
                signal: None,
            }),
            Instant::now(),
        );
        app
    });
    let _controller = cx.new(|cx| WatchController::new(&state, harness.requests(), cx));

    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    harness.respond(Err(offline("first failure")));
    cx.run_until_parked();
    state.read_with(cx, |app, _| {
        assert_eq!(
            app.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("could not recover watch output (watch 7): first failure")
        );
    });

    cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    harness.respond(Err(offline("second failure")));
    cx.run_until_parked();
    state.read_with(cx, |app, _| {
        assert_eq!(
            app.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("could not recover watch output (watch 7): first failure"),
            "later attempts must not rewrite the surfaced failure"
        );
    });

    state.update(cx, |app, _| app.sticky_error = None);
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY * 2);
    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    harness.respond(Err(offline("third failure")));
    cx.run_until_parked();
    state.read_with(cx, |app, _| {
        assert!(
            app.sticky_error.is_none(),
            "later attempts must not restore a dismissed failure"
        );
    });

    cx.executor().advance_clock(RECOVERY_RETRY_DELAY * 4);
    cx.run_until_parked();
    assert_eq!(harness.len(), 1);
    harness.respond(Err(offline("fourth failure")));
    cx.run_until_parked();
    cx.executor().advance_clock(RECOVERY_RETRY_DELAY * 60);
    cx.run_until_parked();
    assert_eq!(harness.len(), 0, "four failed attempts exhaust recovery");

    state.update(cx, |app, cx| {
        app.watches.invalidate();
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(harness.len(), 1, "invalidation restores tail intent");
}
