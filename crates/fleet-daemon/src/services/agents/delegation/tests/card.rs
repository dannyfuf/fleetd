//! Card callers: the run a column starts, the refusals it keeps, and the outcome it records.
//!
//! A card-called delegation shares the whole child lifecycle with a thread-called one and
//! differs in nothing but its delivery: there is no caller transcript to patch and no turn to
//! inject into, so the board hook *is* the delivery and its `Ok` is the only thing that closes
//! the `Deliver` row.
//! These tests drive that half against the same scripted provider harness the sibling worker and
//! delivery suites use, so the rules are proved on the real store, the real outbox and the real
//! drain rather than on a mock of them.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use chrono::Utc;
use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationCaller, DelegationId, DelegationResult, DelegationStatus,
        DeliveryState, ItemKind, PermissionMode, ResultSource, ThreadId,
    },
    ids::{BoardId, CardId, WorktreeId},
};
use fleet_proto::response::ResponseBody;
use sha2::{Digest, Sha256};

use crate::{
    DaemonError, DaemonResult,
    services::agents::store::{OutboxAction, delegations},
};

use super::{
    super::{CardRunRequest, CompleteRequest, RunDeliveryHook, worker::drain},
    worker::Harness,
};

/// The plaintext only the child knows, for the one test that reports through `complete`.
const TOKEN: &str = "only-the-card-child-knows-this";

/// A [`RunDeliveryHook`] that records what it was asked to write, and can refuse a fixed number
/// of times first.
///
/// The two lists are kept apart on purpose: `calls` counts every time the worker reached the
/// board, `recorded` counts every time a board write actually committed. A retry that reaches the
/// hook twice and writes once is the difference between them, and is exactly what the retry test
/// has to see.
#[derive(Default)]
struct RecordingHook {
    state: Mutex<HookState>,
}

#[derive(Default)]
struct HookState {
    calls: Vec<(BoardId, CardId, DelegationId)>,
    recorded: Vec<DelegationId>,
    failures_left: usize,
}

impl RecordingHook {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A hook that refuses its first `failures` calls and writes on every call after them.
    fn failing(failures: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(HookState {
                failures_left: failures,
                ..HookState::default()
            }),
        })
    }

    fn calls(&self) -> Vec<(BoardId, CardId, DelegationId)> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .clone()
    }

    fn recorded(&self) -> Vec<DelegationId> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recorded
            .clone()
    }
}

#[async_trait::async_trait]
impl RunDeliveryHook for RecordingHook {
    async fn on_run_delivered(
        &self,
        board: &BoardId,
        card: &CardId,
        delegation: &Delegation,
    ) -> DaemonResult<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .calls
            .push((board.clone(), card.clone(), delegation.id));
        if state.failures_left > 0 {
            state.failures_left -= 1;
            return Err(DaemonError::Protocol(
                "the board document could not be written".to_owned(),
            ));
        }
        state.recorded.push(delegation.id);
        Ok(())
    }
}

/// Installs `hook` on the harness's service and keeps it upgradeable for the test's lifetime.
///
/// The service holds a `Weak`, so the `Arc` the test keeps is what makes the upgrade succeed;
/// the unsized clone made here may be dropped immediately because both point at one allocation.
fn install(harness: &Harness, hook: &Arc<RecordingHook>) {
    let owned: Arc<RecordingHook> = Arc::clone(hook);
    let installed: Arc<dyn RunDeliveryHook> = owned;
    harness
        .service
        .set_run_delivery_hook(Arc::downgrade(&installed));
}

fn board_id(board: &str) -> BoardId {
    BoardId::try_from(board.to_owned()).unwrap_or_else(|error| panic!("board id {board}: {error}"))
}

fn card_id(card: &str) -> CardId {
    CardId::try_from(card.to_owned()).unwrap_or_else(|error| panic!("card id {card}: {error}"))
}

/// A card-called record in whatever state the test needs, with a fresh child thread id.
fn card_delegation(board: &str, card: &str, status: DelegationStatus) -> Delegation {
    card_delegation_for(board, card, ThreadId::new(), status)
}

fn card_delegation_for(
    board: &str,
    card: &str,
    child: ThreadId,
    status: DelegationStatus,
) -> Delegation {
    let now = Utc::now();
    let terminal = status.is_terminal();
    Delegation {
        id: DelegationId::new(),
        caller: DelegationCaller::Card {
            board: board_id(board),
            card: card_id(card),
        },
        // The whole point of the shape: a card has no turn and no transcript item.
        caller_turn: None,
        caller_item: None,
        child,
        provider: AgentKind::Claude,
        depth: 1,
        brief: format!("work {card} on {board}"),
        expectation: "make lint and make test pass".to_owned(),
        eager: false,
        status,
        status_payload: None,
        result: terminal.then(|| DelegationResult {
            text: format!("finished {card}"),
            files_changed: Vec::new(),
            source: ResultSource::Reported,
            elided: false,
        }),
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: now,
        finished: terminal.then_some(now),
        headline: None,
        usage: None,
    }
}

/// A thread-called record whose caller thread was never created, so the repair sweep must claim it.
fn orphaned_thread_delegation() -> Delegation {
    Delegation {
        caller: DelegationCaller::Thread(ThreadId::new()),
        caller_turn: Some(fleet_core::agents::TurnId::new()),
        caller_item: Some(fleet_core::agents::ItemId::new()),
        ..card_delegation("work", "orphan", DelegationStatus::Succeeded)
    }
}

/// Inserts a record with a token hash the test can report against.
async fn insert_with_token(harness: &Harness, delegation: &Delegation, token: &str) {
    let stored = delegation.clone();
    let token_sha256 = format!("{:x}", Sha256::digest(token.as_bytes()));
    harness
        .store
        .delegation_write("insert card-test delegation", move |tx| {
            delegations::insert(tx, &stored, &token_sha256, &BTreeMap::new())?;
            Ok(((), false))
        })
        .await
        .expect("insert card-test delegation");
}

fn delivery_of(delegation: &Delegation) -> &DeliveryState {
    &delegation.delivery
}

async fn stored(harness: &Harness, id: DelegationId) -> Delegation {
    harness
        .store
        .delegation(id)
        .await
        .expect("read card-test delegation")
        .expect("the card-test delegation exists")
}

async fn open_rows(harness: &Harness) -> Vec<DelegationId> {
    harness
        .store
        .delegation_outbox()
        .await
        .expect("read the delegation outbox")
        .into_iter()
        .map(|row| row.delegation)
        .collect()
}

/// The whole card path: a child reports, the record ends `Succeeded`, and the board hook is what
/// closes the delivery. The order matters more than any single state — the `Deliver` row is open
/// and the delivery still `Pending` right up to the hook's `Ok`.
#[tokio::test(start_paused = true)]
async fn a_card_called_delegation_completes_and_records_through_the_hook() {
    let harness = Harness::start().await;
    let hook = RecordingHook::new();
    install(&harness, &hook);

    let child = harness.create_thread().await;
    let delegation = card_delegation_for("work", "FLT-7", child, DelegationStatus::Settling);
    insert_with_token(&harness, &delegation, TOKEN).await;

    let reported = harness
        .service
        .complete(CompleteRequest {
            delegation: delegation.id,
            child,
            token: TOKEN.to_owned(),
            result: "the card is done".to_owned(),
            blocked: false,
        })
        .await
        .expect("the child reports its result");
    let ResponseBody::Delegation(reported) = reported else {
        panic!("expected one delegation, got {reported:?}");
    };
    assert_eq!(reported.status, DelegationStatus::Succeeded);
    assert_eq!(delivery_of(&reported), &DeliveryState::Pending);
    assert_eq!(
        open_rows(&harness).await,
        vec![delegation.id],
        "the report opens exactly one delivery row, and the board has not been written yet",
    );
    assert!(
        hook.calls().is_empty(),
        "nothing reaches the board before the drain",
    );

    drain(&harness.service).await.expect("drain the card run");

    assert_eq!(
        hook.calls(),
        vec![(board_id("work"), card_id("FLT-7"), delegation.id)],
        "the hook is called once, with the caller the record names",
    );
    assert_eq!(hook.recorded(), vec![delegation.id]);
    assert_eq!(
        delivery_of(&stored(&harness, delegation.id).await),
        &DeliveryState::Recorded,
    );
    assert!(
        open_rows(&harness).await.is_empty(),
        "the board write is what closes the row",
    );
}

/// A daemon composed without a boards service — or one whose service has been dropped — can never
/// grow one while it runs, so the delivery is durably failed instead of retried forever.
#[tokio::test(start_paused = true)]
async fn an_unset_hook_makes_a_card_delivery_undeliverable() {
    let harness = Harness::start().await;
    let delegation = card_delegation("work", "FLT-7", DelegationStatus::Succeeded);
    harness
        .insert(delegation.clone(), OutboxAction::Deliver)
        .await;

    drain(&harness.service)
        .await
        .expect("drain with no board service");

    let stored = stored(&harness, delegation.id).await;
    assert_eq!(
        delivery_of(&stored),
        &DeliveryState::Undeliverable {
            reason: "no board service".to_owned(),
        },
    );
    assert!(open_rows(&harness).await.is_empty());
    assert!(
        stored.result.is_some(),
        "an undeliverable card run keeps its result, as a thread-called one does",
    );
}

/// The hook's `Err` is a retry, not a loss: the row stays open, the delivery stays `Pending`, and
/// the next drain asks again. The hook is reached twice and the board is written once, which is
/// why the contract asks the hook to be idempotent by delegation id.
#[tokio::test(start_paused = true)]
async fn a_failing_hook_leaves_the_deliver_row_open_for_the_next_drain() {
    let harness = Harness::start().await;
    let hook = RecordingHook::failing(1);
    install(&harness, &hook);
    let delegation = card_delegation("work", "FLT-7", DelegationStatus::Succeeded);
    harness
        .insert(delegation.clone(), OutboxAction::Deliver)
        .await;

    drain(&harness.service)
        .await
        .expect("a refused board write does not abort the pass");

    assert_eq!(hook.calls().len(), 1);
    assert!(hook.recorded().is_empty());
    assert_eq!(
        delivery_of(&stored(&harness, delegation.id).await),
        &DeliveryState::Pending,
    );
    assert_eq!(open_rows(&harness).await, vec![delegation.id]);

    drain(&harness.service).await.expect("the retry drain");

    assert_eq!(hook.calls().len(), 2, "the worker asked the board twice");
    assert_eq!(
        hook.recorded(),
        vec![delegation.id],
        "and the board was written exactly once",
    );
    assert_eq!(
        delivery_of(&stored(&harness, delegation.id).await),
        &DeliveryState::Recorded,
    );
    assert!(open_rows(&harness).await.is_empty());
}

/// The sweep that rescues a thread-called delivery whose caller was deleted must not touch a card
/// caller, whose `caller_thread` is NULL and so matches its `NOT EXISTS` by construction. Without
/// the `caller_kind = 'thread'` half of the predicate, the first drain after a restart would mark
/// every pending card delivery `caller deleted` before the hook ever saw it.
#[tokio::test(start_paused = true)]
async fn the_repair_sweep_ignores_card_callers() {
    let harness = Harness::start().await;
    // A hook that always refuses keeps the card row exactly where the sweep left it, so this test
    // observes the sweep rather than the delivery that would otherwise follow it.
    let hook = RecordingHook::failing(usize::MAX);
    install(&harness, &hook);
    let card_run = card_delegation("work", "FLT-7", DelegationStatus::Succeeded);
    let thread_run = orphaned_thread_delegation();
    harness
        .insert(card_run.clone(), OutboxAction::Deliver)
        .await;
    harness
        .insert(thread_run.clone(), OutboxAction::Deliver)
        .await;

    drain(&harness.service).await.expect("drain both callers");

    assert_eq!(
        delivery_of(&stored(&harness, thread_run.id).await),
        &DeliveryState::Undeliverable {
            reason: "caller deleted".to_owned(),
        },
        "the sweep still claims a thread caller that no longer exists",
    );
    assert_eq!(
        delivery_of(&stored(&harness, card_run.id).await),
        &DeliveryState::Pending,
        "and leaves the card caller for its board hook",
    );
    assert_eq!(open_rows(&harness).await, vec![card_run.id]);
}

/// `wait` consumes only for the delegation's own *thread* caller. A card run has no thread, so no
/// waiter can close its delivery early and steal the outcome from the board.
#[tokio::test(start_paused = true)]
async fn wait_never_consumes_a_card_run() {
    let harness = Harness::start().await;
    let delegation = card_delegation("work", "FLT-7", DelegationStatus::Succeeded);
    harness
        .insert(delegation.clone(), OutboxAction::Deliver)
        .await;

    for waiter in [None, Some(ThreadId::new())] {
        let answered = harness
            .service
            .wait(delegation.id, 0, waiter)
            .await
            .expect("wait on a terminal card run");
        let ResponseBody::Delegation(answered) = answered else {
            panic!("expected one delegation, got {answered:?}");
        };
        assert_eq!(
            delivery_of(&answered),
            &DeliveryState::Pending,
            "{waiter:?}"
        );
        assert_eq!(
            delivery_of(&stored(&harness, delegation.id).await),
            &DeliveryState::Pending,
            "{waiter:?}",
        );
    }

    assert_eq!(open_rows(&harness).await, vec![delegation.id]);
}

/// The drain performs at most one row per caller per pass, and a card caller's key is its
/// **board**: the board document is what two cards' deliveries contend for. Two cards of one board
/// therefore record one after the other, while a second board records in the same pass.
#[tokio::test(start_paused = true)]
async fn the_drain_throttle_keys_card_runs_on_their_board() {
    let harness = Harness::start().await;
    let hook = RecordingHook::new();
    install(&harness, &hook);
    let first = card_delegation("work", "FLT-1", DelegationStatus::Succeeded);
    let second = card_delegation("work", "FLT-2", DelegationStatus::Succeeded);
    let other_board = card_delegation("other", "OTH-1", DelegationStatus::Succeeded);
    for delegation in [&first, &second, &other_board] {
        harness
            .insert(delegation.clone(), OutboxAction::Deliver)
            .await;
    }

    drain(&harness.service).await.expect("the first pass");

    assert_eq!(
        hook.recorded(),
        vec![first.id, other_board.id],
        "one row per board, in row order",
    );
    assert_eq!(
        delivery_of(&stored(&harness, second.id).await),
        &DeliveryState::Pending,
        "the board's second card waits for the next pass",
    );
    assert_eq!(open_rows(&harness).await, vec![second.id]);

    drain(&harness.service).await.expect("the second pass");

    assert_eq!(hook.recorded(), vec![first.id, other_board.id, second.id]);
    assert!(open_rows(&harness).await.is_empty());
}

/// The production entry point, end to end: `run_for_card` mints a depth-1 record whose caller is
/// the board and the card, with neither caller-turn nor caller-item, and appends nothing to any
/// transcript — there is no caller thread whose transcript could hold a delegation row.
#[tokio::test(start_paused = true)]
async fn a_card_run_carries_no_caller_turn_or_item() {
    let harness = Harness::start().await;
    let threads_before = harness.manager.summaries().await.len();

    // Creating the child starts a real process, which a paused clock would time out before the
    // executable is scheduled. Only this fixture step runs against real time.
    tokio::time::resume();
    let (delegation, warning) = harness
        .service
        .run_for_card(CardRunRequest {
            board: board_id("work"),
            card: card_id("FLT-7"),
            key: "FLT-7".to_owned(),
            worktree: WorktreeId::try_from("owner/repo#feature").expect("worktree id"),
            provider: AgentKind::Claude,
            brief: "implement the card".to_owned(),
            expectation: "make lint and make test pass".to_owned(),
            mode: PermissionMode::FullAccess,
            model: None,
            title: "↳ FLT-7 — implement the card".to_owned(),
            env: Vec::new(),
        })
        .await
        .expect("start a card run");
    tokio::time::pause();

    assert_eq!(
        warning, None,
        "a card run names its worktree, so it can never inherit a caller's",
    );
    assert_eq!(
        delegation.caller,
        DelegationCaller::Card {
            board: board_id("work"),
            card: card_id("FLT-7"),
        },
    );
    assert_eq!(delegation.caller_turn, None);
    assert_eq!(delegation.caller_item, None);
    assert_eq!(delegation.depth, 1, "a card is the root of its chain");

    let stored = stored(&harness, delegation.id).await;
    assert_eq!(stored.caller, delegation.caller, "and the store agrees");
    assert_eq!(stored.caller_turn, None);
    assert_eq!(stored.caller_item, None);

    let child = harness
        .manager
        .projection(stored.child)
        .await
        .expect("read the card child's projection");
    assert_eq!(
        child.parent, None,
        "the child of a card is nobody's subthread"
    );

    let summaries = harness.manager.summaries().await;
    assert_eq!(
        summaries.len(),
        threads_before + 1,
        "exactly one thread was created: the child",
    );
    for summary in summaries {
        let projection = harness
            .manager
            .projection(summary.thread)
            .await
            .expect("read a thread projection");
        assert!(
            !projection.items.iter().any(
                |item| matches!(item.kind, ItemKind::Delegation { id, .. } if id == delegation.id)
            ),
            "no transcript carries a delegation row for a card run",
        );
    }
}
