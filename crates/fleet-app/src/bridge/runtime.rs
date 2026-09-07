use super::{
    connection::{Failure, Link, daemon_identity, is_alive, open},
    requests, *,
};
use std::{collections::VecDeque, future::Future, pin::Pin};

type Opening<'a> = Pin<Box<dyn Future<Output = Result<(Link, Snapshot), Failure>> + Send + 'a>>;
type HealthCheck = Pin<Box<dyn Future<Output = (u32, bool)> + Send>>;
type IdentityCheck =
    Pin<Box<dyn Future<Output = (u32, Option<String>, Option<(u32, String)>)> + Send>>;

#[derive(Clone, Copy)]
pub(super) enum Backoff {
    Idle,
    Reconnecting {
        previous_pid: u32,
        attempt: u32,
        retry_at: tokio::time::Instant,
    },
}

#[derive(Clone, Copy)]
pub(super) enum OpeningReason {
    Initial,
    Manual,
    Retry { previous_pid: u32, attempt: u32 },
}

async fn wait<T>(operation: &mut Option<impl Future<Output = T> + Unpin>) -> T {
    match operation {
        Some(operation) => operation.await,
        None => std::future::pending().await,
    }
}

async fn at(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

pub(super) fn reconnecting(previous_pid: u32, attempt: u32) -> Backoff {
    Backoff::Reconnecting {
        previous_pid,
        attempt,
        retry_at: tokio::time::Instant::now() + reconnect_backoff(attempt),
    }
}

pub(super) async fn run(
    home: &Path,
    commands: &Receiver<Command>,
    events: &Sender<BridgeEvent>,
    resync_pending: &AtomicBool,
) {
    run_with_intervals(
        home,
        commands,
        events,
        resync_pending,
        HEALTH_INTERVAL,
        IDENTITY_INTERVAL,
    )
    .await;
}

pub(super) async fn run_with_intervals(
    home: &Path,
    commands: &Receiver<Command>,
    events: &Sender<BridgeEvent>,
    resync_pending: &AtomicBool,
    health_interval: Duration,
    identity_interval: Duration,
) {
    let (requests, request_rx) = async_channel::bounded(COMMAND_CAPACITY);
    let _request_task = tokio::spawn(requests::run(request_rx, events.clone()));
    let mut link: Option<Link> = None;
    let mut opening: Option<Opening<'_>> = Some(Box::pin(open(home, events)));
    let mut reason = OpeningReason::Initial;
    let mut health: Option<HealthCheck> = None;
    let mut identity: Option<IdentityCheck> = None;
    let mut waiting = VecDeque::new();
    let mut backoff = Backoff::Idle;
    let mut ticker = tokio::time::interval(health_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut identity_ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + identity_interval,
        identity_interval,
    );
    identity_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let retry_at = opening.is_none().then(|| retry_deadline(backoff)).flatten();
        tokio::select! {
            command = commands.recv() => match command {
                Ok(Command::Request { body, reply }) => {
                    // Opening previously held the command loop. Keep requests in that same
                    // admission window while allowing shutdown and connection progress.
                    if opening.is_some() {
                        queue_while_opening(&mut waiting, body, reply, resync_pending);
                    } else if requests.send(requests::Request::Command {
                        client: link.as_ref().map(|link| link.client.clone()), body, reply,
                    }).await.is_err() {
                        return;
                    }
                }
                Ok(Command::Reconnect) => {
                    if link.is_none() && opening.is_none() {
                        reason = manual_opening_reason(backoff);
                        opening = Some(Box::pin(open(home, events)));
                    }
                }
                Ok(Command::Shutdown) | Err(_) => return,
            },
            result = wait(&mut opening) => {
                opening = None;
                match result {
                    Ok((mut connected, snapshot)) => {
                        let event = opened_event(reason, connected.pid, snapshot);
                        if events.send(event).await.is_err() { return; }
                        connected.start_forwarding(events.clone());
                        link = Some(connected);
                        backoff = Backoff::Idle;
                    }
                    Err(failure) => {
                        let event = match reason {
                        OpeningReason::Initial | OpeningReason::Manual => failure.into_event(),
                        OpeningReason::Retry { previous_pid, attempt } => {
                            let attempt = attempt.saturating_add(1);
                            backoff = reconnecting(previous_pid, attempt);
                            BridgeEvent::Disconnected { attempt }
                        }
                        };
                        if events.send(event).await.is_err() { return; }
                    }
                }
                for (body, reply) in waiting.drain(..) {
                    if requests.send(requests::Request::Command {
                        client: link.as_ref().map(|link| link.client.clone()), body, reply,
                    }).await.is_err() { return; }
                }
                if !dispatch_resync(&link, &requests, resync_pending) {
                    return;
                }
            },
            (previous_pid, alive) = wait(&mut health) => {
                health = None;
                if !alive {
                    link = None;
                    identity = None;
                    backoff = reconnecting(previous_pid, 0);
                    if events.send(BridgeEvent::Disconnected { attempt: 0 }).await.is_err() { return; }
                }
            },
            (expected_pid, expected_boot_id, actual_identity) = wait(&mut identity) => {
                identity = None;
                if let Some((actual_pid, actual_boot_id)) = actual_identity
                    && let Some(current) = link.as_mut().filter(|current| current.pid == expected_pid)
                {
                    let restarted = actual_pid != expected_pid
                        || expected_boot_id.as_ref().is_some_and(|expected| expected != &actual_boot_id);
                    if restarted {
                        link = None;
                        health = None;
                        backoff = reconnecting(expected_pid, 0);
                        if events.send(BridgeEvent::Disconnected { attempt: 0 }).await.is_err() { return; }
                    } else if expected_boot_id.is_none() {
                        current.boot_id = Some(actual_boot_id);
                    }
                }
            },
            () = at(retry_at) => {
                if let Backoff::Reconnecting { previous_pid, attempt, .. } = backoff {
                    reason = OpeningReason::Retry { previous_pid, attempt };
                    opening = Some(Box::pin(open(home, events)));
                }
            },
            _ = ticker.tick() => {
                if health.is_none() && let Some(current) = link.as_ref() {
                    let client = current.client.clone();
                    let pid = current.pid;
                    health = Some(Box::pin(async move { (pid, is_alive(&client).await) }));
                }
                if !dispatch_resync(&link, &requests, resync_pending) {
                    return;
                }
            },
            _ = identity_ticker.tick() => {
                if identity.is_none() && let Some(current) = link.as_ref() {
                    let client = current.client.clone();
                    let pid = current.pid;
                    let boot_id = current.boot_id.clone();
                    identity = Some(Box::pin(async move {
                        (pid, boot_id, daemon_identity(&client).await)
                    }));
                }
            }
        }
    }
}

type WaitingRequest = (
    Box<RequestBody>,
    Option<Sender<Result<ResponseBody, ProtoError>>>,
);

fn queue_while_opening(
    waiting: &mut VecDeque<WaitingRequest>,
    body: Box<RequestBody>,
    reply: Option<Sender<Result<ResponseBody, ProtoError>>>,
    resync_pending: &AtomicBool,
) {
    let displaced = waiting.len() == COMMAND_CAPACITY;
    if displaced && let Some((_, displaced_reply)) = waiting.pop_front() {
        if let Some(displaced_reply) = displaced_reply {
            let _ignored = displaced_reply
                .try_send(Err(offline("the Fleet daemon bridge queue was saturated")));
        } else {
            resync_pending.store(true, Ordering::Release);
        }
    }
    waiting.push_back((body, reply));
}

fn dispatch_resync(
    link: &Option<Link>,
    requests: &Sender<requests::Request>,
    resync_pending: &AtomicBool,
) -> bool {
    let Some(link) = link else {
        return true;
    };
    if !resync_pending.swap(false, Ordering::AcqRel) {
        return true;
    }
    match requests.try_send(requests::Request::Resynchronize {
        client: link.client.clone(),
    }) {
        Ok(()) => true,
        Err(async_channel::TrySendError::Full(_)) => {
            resync_pending.store(true, Ordering::Release);
            true
        }
        Err(async_channel::TrySendError::Closed(_)) => {
            resync_pending.store(true, Ordering::Release);
            false
        }
    }
}

pub(super) fn retry_deadline(backoff: Backoff) -> Option<tokio::time::Instant> {
    match backoff {
        Backoff::Reconnecting { retry_at, .. } => Some(retry_at),
        Backoff::Idle => None,
    }
}

pub(super) fn manual_opening_reason(backoff: Backoff) -> OpeningReason {
    match backoff {
        Backoff::Reconnecting {
            previous_pid,
            attempt,
            ..
        } => OpeningReason::Retry {
            previous_pid,
            attempt,
        },
        Backoff::Idle => OpeningReason::Manual,
    }
}

pub(super) fn opened_event(reason: OpeningReason, pid: u32, snapshot: Snapshot) -> BridgeEvent {
    match reason {
        OpeningReason::Initial | OpeningReason::Manual => {
            BridgeEvent::Connected(Box::new(snapshot))
        }
        OpeningReason::Retry { previous_pid, .. } => BridgeEvent::Reconnected {
            restarted: pid != previous_pid,
            snapshot: Box::new(snapshot),
        },
    }
}
