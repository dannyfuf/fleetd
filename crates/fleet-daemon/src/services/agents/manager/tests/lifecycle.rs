//! Live-session lifecycle: create, stream, steer, gate, interrupt, stop.

use super::*;

#[tokio::test]
async fn remote_worktree_guard_precedes_path_provider_and_store_access() {
    let harness = Harness::start(Capabilities::default()).await;
    let host = HostId::try_from("dev-box").expect("host");
    let guarded_worktree = harness.worktree.clone();
    harness
        .manager
        .set_remote_host_resolver(Arc::new(move |worktree| {
            (worktree == &guarded_worktree).then(|| host.clone())
        }));

    let error = harness
        .manager
        .create(
            harness.worktree.clone(),
            AgentKind::Claude,
            None,
            PermissionMode::Ask,
            None,
            None,
        )
        .await
        .expect_err("remote worktree must not reach the local manager");

    assert_eq!(error.kind, ErrorKind::Remote);
    assert!(error.message.contains("dev-box"));
    assert!(
        harness.script.calls().is_empty(),
        "provider was not started"
    );
    // The database is opened when the manager is built, so its existence proves nothing. What
    // the guard has to prove is that the refused request wrote no thread into it.
    assert!(
        harness.manager.summaries().await.is_empty(),
        "the refused remote request wrote a thread into the local store"
    );
}

#[tokio::test]
async fn create_starts_the_provider_and_persists_the_thread() {
    let harness = Harness::start(full()).await;
    let summary = harness.create(None).await;

    assert_eq!(summary.provider, AgentKind::Claude);
    assert_eq!(summary.session, SessionState::Starting);
    assert_eq!(summary.worktree, harness.worktree);
    assert_eq!(
        harness.script.calls(),
        vec![FakeCall::Start(summary.thread, None)]
    );
    // The row is durable: a fresh manager over the same database lists it without replaying it.
    let restarted = harness.restart().await;
    assert_eq!(
        restarted
            .summaries()
            .await
            .into_iter()
            .map(|summary| summary.thread)
            .collect::<Vec<_>>(),
        vec![summary.thread]
    );
    assert_eq!(
        harness
            .manager
            .summaries()
            .await
            .into_iter()
            .map(|summary| summary.thread)
            .collect::<Vec<_>>(),
        vec![summary.thread]
    );
}

#[tokio::test]
async fn a_turn_streams_items_and_moves_attention_to_finished() {
    let harness = Harness::start(full()).await;
    let mut receiver = harness.events.subscribe();
    let thread = harness.create(None).await.thread;
    harness
        .script
        .emit(AgentEvent::SessionStarted {
            provider: AgentKind::Claude,
            resume_cursor: None,
            model: None,
            mode: PermissionMode::Ask,
            tools: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
        })
        .await;
    let ready = harness
        .settle(thread, "ready", |projection| {
            projection.session == SessionState::Ready
        })
        .await;
    harness
        .manager
        .mark_seen(thread, ready.last_seq)
        .await
        .expect("mark seen");

    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "ship it".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");
    let Some(FakeCall::Send(turn, sent)) = harness.script.calls().last().cloned() else {
        panic!("send must reach the provider with the allocated turn");
    };
    assert_eq!(sent, "ship it");
    let user_item = ItemId::new();
    let assistant = ItemId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted { turn, user_item })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;
    harness
        .script
        .emit(AgentEvent::ItemStarted {
            turn,
            item: assistant,
            kind: ItemKind::AssistantText,
            parent: None,
        })
        .await;
    for delta in ["hel", "lo"] {
        harness
            .script
            .emit(AgentEvent::ContentDelta {
                item: assistant,
                stream: StreamKind::AssistantText,
                delta: delta.to_owned(),
            })
            .await;
    }
    harness
        .script
        .emit(AgentEvent::ItemCompleted {
            item: assistant,
            status: ItemStatus::Done,
        })
        .await;
    harness.script.emit(completed(turn)).await;

    let projection = harness
        .settle(thread, "a completed turn", |projection| {
            matches!(projection.turn, TurnState::Completed(finished, _) if finished == turn)
        })
        .await;
    assert_eq!(assistant_text(&projection), Some("hello"));
    assert!(projection.items.iter().any(|item| matches!(
        &item.kind,
        ItemKind::UserMessage { text, .. } if text == "ship it"
    )));
    let summary = harness
        .manager
        .summaries()
        .await
        .pop()
        .expect("one thread summary");
    assert_eq!(
        summary.attention,
        Attention::NeedsYou(AttentionKind::Finished)
    );
    let observed = attentions(&drain(&mut receiver));
    assert!(observed.contains(&Attention::Working), "{observed:?}");
    assert!(
        observed.contains(&Attention::NeedsYou(AttentionKind::Finished)),
        "{observed:?}"
    );

    // §3.3 keeps the seen cursor "per client, not persisted daemon-side": one broadcast
    // summary reaches every subscriber, so it keeps reporting the unread completion and the
    // client that read the thread narrows it against the cursor it holds itself.
    harness
        .manager
        .mark_seen(thread, projection.last_seq)
        .await
        .expect("mark seen");
    let summary = harness
        .manager
        .summaries()
        .await
        .pop()
        .expect("one thread summary");
    assert_eq!(
        summary.attention,
        Attention::NeedsYou(AttentionKind::Finished),
        "the daemon must not fold one client's cursor into the shared summary"
    );
    assert_eq!(
        summary.attention_for(projection.last_seq),
        Attention::Idle,
        "a fully seen idle thread has no attention for the client that saw it"
    );
}

#[tokio::test]
async fn steering_a_running_turn_records_a_user_message_in_that_turn() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "also update the docs".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("steer");

    let projection = harness.projection(thread).await;
    let steered = projection
        .items
        .iter()
        .find(|item| matches!(&item.kind, ItemKind::UserMessage { text, .. } if text == "also update the docs"))
        .expect("the steered message is recorded in the running turn");
    assert_eq!(steered.turn, turn);
    assert_eq!(steered.status, ItemStatus::Done);
    assert!(matches!(
        harness.script.calls().last(),
        Some(FakeCall::Send(sent, _)) if *sent == turn
    ));
}

#[tokio::test]
async fn gates_open_for_attention_and_respond_reaches_the_provider() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let gate = GateId::new();
    harness.script.emit(permission_gate(gate)).await;
    harness
        .settle(thread, "an open gate", |projection| {
            !projection.gates.is_empty()
        })
        .await;
    assert_eq!(
        harness.manager.summaries().await[0].attention,
        Attention::NeedsYou(AttentionKind::Permission)
    );

    let answer = GateAnswer::Permission {
        choice: PermissionChoice::AllowOnce,
        edited_payload: None,
    };
    let unknown = harness
        .manager
        .respond(thread, GateId::new(), answer.clone())
        .await
        .expect_err("an unknown gate is rejected");
    assert_eq!(unknown.kind, ErrorKind::Conflict);

    harness
        .manager
        .respond(thread, gate, answer)
        .await
        .expect("respond");
    assert!(harness.script.calls().contains(&FakeCall::Respond(gate)));

    harness
        .script
        .emit(AgentEvent::GateResolved {
            gate,
            answer: GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
            by: fleet_core::agents::GateResolver::User,
        })
        .await;
    harness
        .settle(thread, "a closed gate", |projection| {
            projection.gates.is_empty()
        })
        .await;
}

#[tokio::test]
async fn interrupting_a_settled_turn_acks_while_a_live_one_reaches_the_provider() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    // `esc` and the `result` frame that ends a turn race by milliseconds, and the client cannot
    // see the settle coming: an interrupt with nothing to interrupt is a no-op, not a conflict
    // the status bar has to show the user.
    assert!(matches!(
        harness.manager.interrupt(thread).await,
        Ok(ResponseBody::AgentAck)
    ));
    assert!(
        !harness
            .script
            .calls()
            .iter()
            .any(|call| matches!(call, FakeCall::Interrupt(_))),
        "an idle thread must not reach the provider at all"
    );

    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;
    harness.manager.interrupt(thread).await.expect("interrupt");
    assert!(harness.script.calls().contains(&FakeCall::Interrupt(turn)));

    harness
        .script
        .emit(AgentEvent::TurnAborted {
            turn,
            reason: AbortReason::User,
        })
        .await;
    let projection = harness
        .settle(thread, "an interrupted turn", |projection| {
            projection.turn == TurnState::Interrupted(turn)
        })
        .await;
    assert!(projection.gates.is_empty());
}

#[tokio::test]
async fn stop_exits_the_session_and_refuses_later_sends() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness.manager.stop(thread).await.expect("stop");
    assert!(harness.script.calls().contains(&FakeCall::Stop));
    let projection = harness.projection(thread).await;
    assert_eq!(projection.session, SessionState::Stopped);
    assert_eq!(projection.turn, TurnState::Interrupted(turn));

    let refused = harness
        .manager
        .send(
            thread,
            UserInput {
                text: "hello".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect_err("a stopped thread cannot be sent to");
    assert_eq!(refused.kind, ErrorKind::Conflict);
    assert_eq!(
        harness.manager.summaries().await[0].session,
        SessionState::Stopped
    );
}

#[tokio::test]
async fn open_returns_only_events_after_the_requested_cursor() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    for message in ["one", "two", "three"] {
        harness
            .script
            .emit(AgentEvent::Notice(message.to_owned()))
            .await;
    }
    harness
        .settle(thread, "three notices", |projection| {
            projection.last_seq == Seq(3)
        })
        .await;

    let ResponseBody::AgentThreadSnapshot {
        projection,
        events_after,
    } = harness
        .manager
        .open(thread, Some(Seq(1)))
        .await
        .expect("open from a cursor")
    else {
        panic!("expected a snapshot");
    };
    assert_eq!(projection.last_seq, Seq(1));
    assert_eq!(
        events_after
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![Seq(2), Seq(3)]
    );
    let ahead = harness
        .manager
        .open(thread, Some(Seq(9)))
        .await
        .expect_err("a cursor beyond the log is rejected");
    assert_eq!(ahead.kind, ErrorKind::Validation);
}

#[tokio::test]
async fn a_catch_up_open_never_starts_a_provider() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-gap".to_owned())).await.thread;
    harness.manager.stop(thread).await.expect("stop the thread");
    assert_eq!(
        harness
            .manager
            .summaries()
            .await
            .into_iter()
            .find(|summary| summary.thread == thread)
            .map(|summary| summary.session),
        Some(SessionState::Stopped)
    );
    let starts = harness.script.starts();

    // A client repairing a sequence gap asks from its last applied cursor. That is not a user
    // opening a tab, so it must not relaunch the provider (§6).
    harness
        .manager
        .open(thread, Some(Seq(1)))
        .await
        .expect("catch-up open");
    assert_eq!(harness.script.starts(), starts);
}

#[tokio::test]
async fn a_mode_change_is_a_durable_event_every_mirror_sees() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let mut events = harness.events.subscribe();
    harness
        .manager
        .set_mode(thread, PermissionMode::AcceptEdits)
        .await
        .expect("set mode");

    let broadcast = drain(&mut events);
    assert!(
        broadcast.iter().any(|event| matches!(
            event,
            Event::Agent {
                event: SeqEvent {
                    event: AgentEvent::MetadataChanged {
                        mode: Some(PermissionMode::AcceptEdits),
                        ..
                    },
                    ..
                },
                ..
            }
        )),
        "a mode change reaches clients as an event: {broadcast:?}"
    );
    assert_eq!(
        harness.projection(thread).await.mode,
        PermissionMode::AcceptEdits
    );
}

#[tokio::test]
async fn answering_one_gate_twice_writes_one_response() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let gate = GateId::new();
    harness
        .script
        .emit(AgentEvent::GateOpened {
            gate,
            turn: None,
            kind: GateKind::Permission {
                tool: ToolKind::Bash,
                title: "Run command?".to_owned(),
                payload: "cargo test".to_owned(),
                rationale: None,
                options: Vec::new(),
            },
        })
        .await;
    harness
        .settle(thread, "an open gate", |projection| {
            projection.gates.len() == 1
        })
        .await;

    let answer = GateAnswer::Permission {
        choice: PermissionChoice::AllowOnce,
        edited_payload: None,
    };
    for _ in 0..2 {
        harness
            .manager
            .respond(thread, gate, answer.clone())
            .await
            .expect("a repeated answer is idempotent");
    }
    assert_eq!(
        harness
            .script
            .calls()
            .iter()
            .filter(|call| **call == FakeCall::Respond(gate))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_session_that_ends_settles_the_gates_it_leaves_open() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let gate = GateId::new();
    harness.script.emit(permission_gate(gate)).await;
    harness
        .settle(thread, "an open gate", |projection| {
            !projection.gates.is_empty()
        })
        .await;

    harness
        .script
        .emit(AgentEvent::SessionExited {
            code: Some(137),
            expected: false,
        })
        .await;
    let projection = harness
        .settle(thread, "a dead session", |projection| {
            projection.session == SessionState::Error
        })
        .await;

    // §3.3 rule 4 and §6: the card cannot outlive the adapter that could answer it.
    assert!(
        projection.gates.is_empty(),
        "a dead session leaves no unanswerable card behind"
    );
    assert_eq!(
        harness.manager.summaries().await[0].attention,
        Attention::Failed,
        "the tab reports the dead session, not the gate it left"
    );
}

#[tokio::test]
async fn send_resumes_a_stopped_thread_rather_than_refusing_it() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-2".to_owned())).await.thread;
    harness
        .manager
        .stop(thread)
        .await
        .expect("stop the live thread");

    // §6 resumes lazily on use; §7 makes `send` one of those uses.
    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "carry on".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send resumes the stopped thread");
    assert_eq!(
        harness.script.starts(),
        2,
        "send started a resumed provider"
    );
    assert!(
        harness
            .script
            .calls()
            .contains(&FakeCall::Start(thread, Some("cursor-2".to_owned())))
    );
}

/// BH1: a `kill -9` mid-turn left the dead adapter in the slot, and every later send failed
/// with `turn … was sent while turn … is active` until the daemon was restarted.
#[tokio::test]
async fn a_crashed_provider_is_dropped_so_the_next_send_resumes_the_thread() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-crash".to_owned())).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness
        .script
        .emit(AgentEvent::SessionExited {
            code: Some(137),
            expected: false,
        })
        .await;
    // Read through `summaries`, not `open`: opening the tab is itself one of the lazy resume
    // points, and it would hide the state this test is about.
    let mut updates = harness.events.subscribe();
    tokio::time::timeout(SETTLE, async {
        loop {
            let summary = harness
                .manager
                .summaries()
                .await
                .into_iter()
                .find(|summary| summary.thread == thread)
                .expect("the thread exists");
            if summary.session == SessionState::Error {
                return;
            }
            next_update(&mut updates, "a dead session").await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("agent thread never reached a dead session"));

    // §6: the thread is resumed lazily with a *new* adapter, not refused by the dead one.
    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "carry on".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send resumes a thread whose provider crashed");
    assert_eq!(
        harness.script.starts(),
        2,
        "the crashed adapter was replaced rather than reused"
    );
}

/// BH: `pending_claude_inputs` was drained only while the *front* entry matched the turn that
/// had just started, so one prompt whose `TurnStarted` never arrived blocked the queue: every
/// later prompt was written to Claude and never recorded, and §6's log stopped being the
/// transcript — the model answering a question the tab does not show.
#[tokio::test]
async fn a_prompt_whose_turn_never_started_does_not_swallow_the_next_one() {
    let harness = Harness::start(full()).await;
    let thread = harness
        .create(Some("cursor-strand".to_owned()))
        .await
        .thread;

    // The fake provider echoes nothing, so this prompt's `TurnStarted` never lands.
    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "one".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send the first prompt");
    harness
        .script
        .emit(AgentEvent::SessionExited {
            code: None,
            expected: false,
        })
        .await;
    // The session dying is the last chance to write that prompt down, so the transcript has it
    // before anything else happens. Reading the thread is also what resumes it (§6).
    harness
        .settle(
            thread,
            "the stranded prompt in the transcript",
            |projection| user_messages(projection) == ["one"],
        )
        .await;

    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "two".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send resumes the thread");
    let resumed = harness
        .script
        .calls()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            FakeCall::Send(turn, text) if text == "two" => Some(turn),
            _ => None,
        })
        .expect("the second prompt reached a provider");
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn: resumed,
            user_item: ItemId::new(),
        })
        .await;

    let projection = harness
        .settle(
            thread,
            "the second prompt in the transcript",
            |projection| user_messages(projection).len() == 2,
        )
        .await;
    assert_eq!(
        user_messages(&projection),
        ["one", "two"],
        "the log is the transcript: every prompt Claude was given is in it, in order",
    );
}

/// BH: a stop that races the child's own exit answers `agent provider exited`, and `stop`
/// returned on it before settling anything — the turn stayed `Running` on a dead process, so
/// §2's tab spun forever and `attention()` never left `Working`.
#[tokio::test]
async fn stop_settles_the_turn_even_when_the_provider_will_not_stop() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness.script.stop_fails.store(true, Ordering::SeqCst);
    harness
        .manager
        .stop(thread)
        .await
        .expect("a provider that will not stop still settles the thread");

    let projection = harness.projection(thread).await;
    assert_eq!(projection.turn, TurnState::Interrupted(turn));
    assert_eq!(projection.session, SessionState::Stopped);
}

#[tokio::test]
async fn an_unavailable_provider_reports_the_terminal_fallback_hint() {
    let harness = Harness::start(full()).await;
    harness.script.unavailable.store(true, Ordering::SeqCst);
    let error = harness
        .manager
        .create(
            harness.worktree.clone(),
            AgentKind::Claude,
            None,
            PermissionMode::Ask,
            None,
            None,
        )
        .await
        .expect_err("an unavailable provider fails creation");

    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(error.message.contains("terminal fallback"), "{error:?}");
    assert!(harness.manager.summaries().await.is_empty());
}

/// The list row carries the title the *reducer* derived, not the one creation guessed.
///
/// The store records only the title a record hands it, and the reducer renames a default-titled
/// thread after its first user message. Since `AgentThreadList` is now answered from the database
/// rather than from memory, the append that derives the title has to persist the record in the same
/// breath — otherwise the Hub shows "Claude" for the life of the thread while the tab shows the
/// prompt.
#[tokio::test]
async fn the_derived_title_reaches_the_thread_list_and_not_only_the_tab() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    let user_item = ItemId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted { turn, user_item })
        .await;
    harness
        .script
        .emit(AgentEvent::ItemStarted {
            turn,
            item: user_item,
            kind: ItemKind::UserMessage {
                text: "rewrite the storage docs".to_owned(),
                attachments: Vec::new(),
            },
            parent: None,
        })
        .await;
    harness
        .settle(thread, "the derived title", |projection| {
            projection.title == "rewrite the storage docs"
        })
        .await;

    let listed = harness.manager.summaries().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "rewrite the storage docs");

    // And it survives the restart, because the record was written and not only the projection.
    let restarted = harness.restart().await;
    assert_eq!(
        restarted.summaries().await[0].title,
        "rewrite the storage docs"
    );
}
