//! The four authority rules of the durable read-through mirror (`docs/NATIVE-AGENTS.md` §9.3).
//!
//! Every test here drives the *real* store through the manager's mirror seam, with the scripted
//! provider watching: `script.starts()` is what proves no harness process was started for a
//! thread this daemon does not own, and it is zero in every test in this file.

use fleet_core::{
    agents::{
        GateAnswer, ItemKind, PermissionChoice, PermissionOption, ProviderOptionId, TurnOutcome,
        Usage,
    },
    ids::HostId,
};
use fleet_proto::response::ResponseBody;

use super::*;
use crate::services::agents::manager::mirror::MirrorIngest;

fn host(value: &str) -> HostId {
    value.parse().unwrap_or_else(|error| panic!("{error}"))
}

/// One owner-side thread: the events it logged and the summary it broadcasts.
struct Owner {
    thread: ThreadId,
    projection: ThreadProjection,
    events: Vec<SeqEvent>,
    turn: TurnId,
    gate: GateId,
}

impl Owner {
    fn new(worktree: WorktreeId) -> Self {
        let thread = ThreadId::new();
        Self {
            thread,
            projection: ThreadProjection::new(thread, worktree, AgentKind::Claude),
            events: Vec::new(),
            turn: TurnId::new(),
            gate: GateId::new(),
        }
    }

    /// Logs one event on the owner's side, at the owner's own next sequence.
    fn log(&mut self, event: AgentEvent) -> SeqEvent {
        let sequenced = SeqEvent {
            seq: self.projection.last_seq.next(),
            at: chrono::DateTime::from_timestamp_millis(
                1_700_000_000_000 + i64::try_from(self.projection.last_seq.0).unwrap_or_default(),
            )
            .unwrap_or_else(chrono::Utc::now),
            raw: None,
            event,
        };
        self.projection
            .apply(&sequenced)
            .unwrap_or_else(|error| panic!("the owner's own reducer accepts its event: {error}"));
        self.events.push(sequenced.clone());
        sequenced
    }

    /// A prompt, an answer, and a permission card still open: the shape of a live remote turn.
    fn live_turn(&mut self) {
        let user = ItemId::new();
        let assistant = ItemId::new();
        self.log(AgentEvent::SessionConfigured {
            provider: AgentKind::Claude,
            resume_cursor: Some("owner-cursor".to_owned()),
            model: None,
            mode: PermissionMode::Ask,
            tools: vec!["Bash".to_owned()],
            commands: Vec::new(),
            skills: Vec::new(),
        });
        self.log(AgentEvent::TurnStarted {
            turn: self.turn,
            user_item: user,
        });
        self.log(AgentEvent::ItemStarted {
            turn: self.turn,
            item: user,
            kind: ItemKind::UserMessage {
                text: "what changed?".to_owned(),
                attachments: Vec::new(),
                steered: false,
            },
            parent: None,
        });
        self.log(AgentEvent::ItemStarted {
            turn: self.turn,
            item: assistant,
            kind: ItemKind::AssistantText {
                text: "the remote answer".to_owned(),
            },
            parent: None,
        });
        self.log(AgentEvent::GateOpened {
            gate: self.gate,
            turn: Some(self.turn),
            kind: GateKind::Permission {
                tool: ToolKind::Bash,
                title: "Run ls".to_owned(),
                payload: "ls".to_owned(),
                rationale: None,
                options: vec![PermissionOption {
                    id: ProviderOptionId("allow".to_owned()),
                    label: PermissionChoice::AllowOnce,
                }],
            },
        });
    }

    /// A long stretch of cheap events, for the admission ladder.
    fn notices(&mut self, count: usize) {
        for index in 0..count {
            self.log(AgentEvent::Notice(format!("notice {index}")));
        }
    }

    fn summary(&self) -> AgentThreadSummary {
        let mut summary = self.projection.summary(Seq::default());
        summary.host = None;
        summary
    }
}

/// A harness whose store already mirrors one thread of `owner_host`.
async fn mirrored(harness: &Harness, owner_host: &HostId) -> Owner {
    let mut owner = Owner::new(harness.worktree.clone());
    owner.live_turn();
    harness
        .manager
        .mirror_adopt(owner_host, std::slice::from_ref(&owner.summary()))
        .await;
    let ingested = harness
        .manager
        .mirror_append(owner_host, owner.thread, &owner.events, None)
        .await;
    assert_eq!(ingested, MirrorIngest::Applied);
    owner
}

/// The mirrored prefix this daemon holds.
async fn held(harness: &Harness, thread: ThreadId) -> Seq {
    harness
        .manager
        .ownership(thread)
        .await
        .expect("read ownership")
        .map(|ownership| ownership.head_seq)
        .unwrap_or_default()
}

#[tokio::test]
async fn only_the_owning_hosts_link_may_extend_a_mirrored_sequence() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let intruder = host("laptop");
    let mut owner = mirrored(&harness, &owner_host).await;
    let held_before = held(&harness, owner.thread).await;
    let next = owner.log(AgentEvent::Notice("from somewhere else".to_owned()));

    // Two writers on one sequence space is silent corruption, so the second one is refused
    // rather than merged.
    let refused = harness
        .manager
        .mirror_append(&intruder, owner.thread, std::slice::from_ref(&next), None)
        .await;

    assert_eq!(refused, MirrorIngest::Refused);
    assert_eq!(held(&harness, owner.thread).await, held_before);
    let projection = harness
        .manager
        .ownership(owner.thread)
        .await
        .expect("read ownership")
        .expect("the thread is still mirrored");
    assert_eq!(projection.owner, Some(owner_host.clone()));
    // The owner's own next event still lands.
    assert_eq!(
        harness
            .manager
            .mirror_append(&owner_host, owner.thread, &[next], None)
            .await,
        MirrorIngest::Applied
    );
    assert_eq!(held(&harness, owner.thread).await, held_before.next());
}

#[tokio::test]
async fn no_harness_process_is_started_for_a_mirrored_thread() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let owner = mirrored(&harness, &owner_host).await;
    assert_eq!(harness.script.starts(), 0);

    // The owner's session is `ready` with a resume cursor and a running turn, which is exactly
    // the state that makes a *local* thread resumable on open. Mirrored, it must not be.
    let answer = harness
        .manager
        .open(&window_body(owner.thread, Some(10)))
        .await
        .expect("a mirrored open is answered from the mirror");
    let ResponseBody::AgentThreadWindow(_) = &answer else {
        panic!("expected a window, got {answer:?}");
    };
    assert_eq!(
        harness.script.starts(),
        0,
        "an open resumed a remote thread"
    );

    let send = harness
        .manager
        .send(
            owner.thread,
            UserInput {
                text: "not from here".to_owned(),
                attachments: Vec::new(),
                item: None,
            },
        )
        .await
        .expect_err("a send to a mirrored thread must be refused");
    assert_eq!(send.kind, ErrorKind::Remote);
    assert!(send.message.contains("dev-box"));
    assert_eq!(harness.script.starts(), 0, "a send resumed a remote thread");
    assert!(harness.script.calls().is_empty());
}

#[tokio::test]
async fn a_mutation_for_a_mirrored_thread_is_never_applied_locally() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let owner = mirrored(&harness, &owner_host).await;
    let held_before = held(&harness, owner.thread).await;

    // The owner's reducer is the only thing that decides whether a gate answer was accepted, so
    // an answer that reaches this daemon is refused rather than recorded optimistically.
    let refused = harness
        .manager
        .respond(
            owner.thread,
            owner.gate,
            GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
        )
        .await
        .expect_err("a gate answer for a mirrored thread must be refused");

    assert_eq!(refused.kind, ErrorKind::Remote);
    assert_eq!(
        held(&harness, owner.thread).await,
        held_before,
        "the refusal appended an event to the owner's sequence"
    );
    let summary = harness
        .manager
        .summaries()
        .await
        .into_iter()
        .find(|summary| summary.thread == owner.thread)
        .expect("the mirrored thread is listed");
    assert_eq!(
        summary.attention,
        Attention::NeedsYou(AttentionKind::Permission),
        "the card stays open until the owner says otherwise"
    );

    for refusal in [
        harness.manager.interrupt(owner.thread).await,
        harness.manager.stop(owner.thread).await,
        harness
            .manager
            .set_mode(owner.thread, PermissionMode::AcceptEdits)
            .await,
        harness
            .manager
            .set_model(
                owner.thread,
                ModelSelection {
                    model: "sonnet".to_owned(),
                    effort: None,
                    provider: None,
                },
            )
            .await,
    ] {
        let error = refusal.expect_err("every mutation on a mirrored thread is refused");
        assert_eq!(error.kind, ErrorKind::Remote);
    }
    assert_eq!(held(&harness, owner.thread).await, held_before);
}

#[tokio::test]
async fn the_mirror_never_fabricates_a_synchronized_marker() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let owner = mirrored(&harness, &owner_host).await;
    let mut events = harness.events.subscribe();

    let answer = harness
        .manager
        .open(&fleet_proto::request::RequestBody::AgentThreadOpen {
            thread: owner.thread,
            from_seq: None,
            after_seq: None,
            turn_limit: Some(10),
            before_cursor: None,
            // Asking for the marker is what a client does when it wants to know it is live.
            request_sync_marker: true,
        })
        .await
        .expect("a mirrored open");

    let ResponseBody::AgentThreadWindow(window) = answer else {
        panic!("expected a window");
    };
    assert!(
        !window.synchronized,
        "a mirrored answer is never presented as confirmed by its owner"
    );
    assert!(
        !drain(&mut events)
            .iter()
            .any(|event| matches!(event, Event::AgentSynchronized { .. })),
        "the mirror fabricated a synchronization the owner never sent"
    );

    // A thread this daemon *does* own is the thing whose word `Live` means, so its marker is
    // published.
    let local = harness.create(None).await.thread;
    let mut local_events = harness.events.subscribe();
    harness
        .manager
        .open(&fleet_proto::request::RequestBody::AgentThreadOpen {
            thread: local,
            from_seq: None,
            after_seq: None,
            turn_limit: Some(10),
            before_cursor: None,
            request_sync_marker: true,
        })
        .await
        .expect("a local windowed open");
    assert!(
        drain(&mut local_events)
            .iter()
            .any(|event| matches!(event, Event::AgentSynchronized { thread } if *thread == local)),
        "a locally owned thread owes its client the marker it asked for"
    );
}

#[tokio::test]
async fn a_warm_mirror_answers_the_whole_transcript_from_the_local_database() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let owner = mirrored(&harness, &owner_host).await;

    let ResponseBody::AgentThreadWindow(window) = harness
        .manager
        .open(&window_body(owner.thread, Some(10)))
        .await
        .expect("a mirrored open")
    else {
        panic!("expected a window");
    };

    // The owner's own sequence values, the owner's transcript, read out of this daemon's file.
    assert_eq!(window.head_seq, owner.projection.last_seq);
    assert_eq!(window.summary.thread, owner.thread);
    // A mirrored answer skips the router's id translation, so it has to name the owner itself or
    // the list and the window would disagree about where the thread lives.
    assert_eq!(window.summary.host, Some(owner_host.clone()));
    assert!(window.window.items.iter().any(
        |item| matches!(&item.kind, ItemKind::AssistantText { text } if text == "the remote answer")
    ));
    // A decision the user has to answer is never paged out.
    assert_eq!(
        window
            .window
            .gates
            .iter()
            .map(|gate| gate.id)
            .collect::<Vec<_>>(),
        vec![owner.gate]
    );
    // The harness runtime the owner reported survives into the window's session view.
    assert_eq!(window.session.tools, vec!["Bash".to_owned()]);
    assert!(!window.synchronized);
}

#[tokio::test]
async fn the_delta_request_asks_only_for_what_the_mirror_is_missing() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let mut owner = mirrored(&harness, &owner_host).await;
    // The owner moved on; its summary says so before any transcript follows.
    owner.log(AgentEvent::TurnSettled {
        turn: owner.turn,
        outcome: TurnOutcome::Completed,
        usage: Usage::default(),
        duration_ms: 12,
        files_changed: Vec::new(),
    });
    harness
        .manager
        .mirror_adopt(&owner_host, std::slice::from_ref(&owner.summary()))
        .await;

    let delta = crate::services::router::agents::AgentMirror::delta(
        &harness.manager,
        &owner_host,
        &window_body(owner.thread, Some(10)),
    )
    .await
    .expect("a mirrored thread owes a delta request");

    let fleet_proto::request::RequestBody::AgentThreadOpen {
        thread,
        after_seq,
        request_sync_marker,
        ..
    } = delta
    else {
        panic!("the delta is an open");
    };
    assert_eq!(thread, owner.thread);
    // Exactly the prefix this daemon holds: a caught-up mirror would transfer nothing at all.
    assert_eq!(after_seq, Some(held(&harness, owner.thread).await));
    assert!(request_sync_marker, "only the owner may say `live`");
}

#[tokio::test]
async fn a_gap_asks_for_a_window_instead_of_leaving_a_hole() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let mut owner = mirrored(&harness, &owner_host).await;
    let held_before = held(&harness, owner.thread).await;
    owner.log(AgentEvent::Notice("first".to_owned()));
    let after_the_gap = owner.log(AgentEvent::Notice("second".to_owned()));

    let outcome = harness
        .manager
        .mirror_ingest(&owner_host, owner.thread, &after_the_gap)
        .await;

    assert_eq!(outcome, MirrorIngest::NeedsWindow);
    assert_eq!(
        held(&harness, owner.thread).await,
        held_before,
        "a hole in the prefix would make every later read a lie"
    );
    // A sequence already held is a duplicate, not a gap: catch-up and live delivery overlap.
    assert_eq!(
        harness
            .manager
            .mirror_append(&owner_host, owner.thread, &owner.events[..2], None)
            .await,
        MirrorIngest::Duplicate
    );
}

#[tokio::test]
async fn an_owner_whose_head_went_backwards_discards_the_mirror() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let owner = mirrored(&harness, &owner_host).await;
    assert!(held(&harness, owner.thread).await > Seq(1));

    // The owner's database was restored or replaced. The mirror is never right against the
    // owner, so the cached transcript goes rather than being reconciled.
    let restored = ThreadProjection::new(owner.thread, harness.worktree.clone(), AgentKind::Claude)
        .summary(Seq::default());
    harness
        .manager
        .mirror_adopt(&owner_host, std::slice::from_ref(&restored))
        .await;

    assert_eq!(held(&harness, owner.thread).await, Seq(0));
    let ownership = harness
        .manager
        .ownership(owner.thread)
        .await
        .expect("read ownership")
        .expect("the header survives");
    assert_eq!(ownership.owner, Some(owner_host));
    assert!(!ownership.is_warm(), "the cache was kept after a rollback");
    // A cold mirror is proxied, not answered.
    assert!(
        harness
            .manager
            .mirror_open(
                &crate::services::agents::manager::window::OpenRequest::from_body(&window_body(
                    owner.thread,
                    Some(10)
                ))
                .expect("an open request")
            )
            .await
            .is_none()
    );
}

#[tokio::test]
async fn a_restart_never_settles_a_mirrored_thread_as_its_own_orphan() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let owner = mirrored(&harness, &owner_host).await;
    let held_before = held(&harness, owner.thread).await;

    // The mirrored prefix ends mid-turn with a session the owner still has. Settling that would
    // mean appending `TurnAborted` and `SessionExited` into the owner's sequence space.
    let restarted = harness.restart().await;

    assert_eq!(held(&harness, owner.thread).await, held_before);
    assert_eq!(
        restarted.mirrored_threads(),
        vec![(owner.thread, owner_host)],
        "the durable census is what routes a mirrored thread upstream before any link is up"
    );
    let summary = restarted
        .summaries()
        .await
        .into_iter()
        .find(|summary| summary.thread == owner.thread)
        .expect("the mirrored thread is listed after a restart");
    assert_eq!(summary.turn, TurnState::Running(owner.turn));
    assert_eq!(harness.script.starts(), 0);
}

#[tokio::test]
async fn the_ladder_sends_a_window_instead_of_a_giant_replay() {
    let harness = Harness::start(full()).await;
    let owner_host = host("dev-box");
    let mut owner = mirrored(&harness, &owner_host).await;
    let already_held = owner.events.len();
    // Ten minutes of a busy remote turn. Replaying all of it into one frame is the failure the
    // ladder exists to prevent, and the ladder's own threshold is the boundary asserted here.
    owner.notices(super::super::window::REPLAY_MAX_EVENTS as usize + 10);
    let tail = owner.events[already_held..].to_vec();
    assert_eq!(
        harness
            .manager
            .mirror_append(&owner_host, owner.thread, &tail, None)
            .await,
        MirrorIngest::Applied
    );

    let ResponseBody::AgentThreadWindow(over) = harness
        .manager
        .open(&fleet_proto::request::RequestBody::AgentThreadOpen {
            thread: owner.thread,
            from_seq: None,
            after_seq: Some(Seq(1)),
            turn_limit: Some(10),
            before_cursor: None,
            request_sync_marker: false,
        })
        .await
        .expect("an open past the ladder")
    else {
        panic!("expected a window");
    };
    assert!(
        over.events_after.is_empty(),
        "the ladder tripped and still replayed {} events",
        over.events_after.len()
    );
    assert_eq!(over.head_seq, owner.projection.last_seq);

    // Just inside the ladder, the same open replays rather than windowing: the cheap case is not
    // paying for the expensive one.
    let inside = Seq(owner.projection.last_seq.0 - 4);
    let ResponseBody::AgentThreadWindow(under) = harness
        .manager
        .open(&fleet_proto::request::RequestBody::AgentThreadOpen {
            thread: owner.thread,
            from_seq: None,
            after_seq: Some(inside),
            turn_limit: Some(10),
            before_cursor: None,
            request_sync_marker: false,
        })
        .await
        .expect("an open inside the ladder")
    else {
        panic!("expected a window");
    };
    assert_eq!(
        under
            .events_after
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        (inside.0 + 1..=owner.projection.last_seq.0)
            .map(Seq)
            .collect::<Vec<_>>()
    );
}
