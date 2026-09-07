use super::{
    connection::{Failure, Link, is_alive, open},
    requests, *,
};
use std::{collections::VecDeque, future::Future, pin::Pin};

type Opening<'a> = Pin<Box<dyn Future<Output = Result<(Link, Snapshot), Failure>> + Send + 'a>>;
type HealthCheck = Pin<Box<dyn Future<Output = (u32, bool)> + Send>>;

#[derive(Clone, Copy)]
enum Backoff {
    Idle,
    Reconnecting { previous_pid: u32, attempt: u32 },
}

#[derive(Clone, Copy)]
enum OpeningReason {
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

async fn after(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending().await,
    }
}

pub(super) async fn run(home: &Path, commands: &Receiver<Command>, events: &Sender<BridgeEvent>) {
    let (requests, request_rx) = async_channel::unbounded();
    let _request_task = tokio::spawn(requests::run(request_rx, events.clone()));
    let mut link: Option<Link> = None;
    let mut opening: Option<Opening<'_>> = Some(Box::pin(open(home, events)));
    let mut reason = OpeningReason::Initial;
    let mut health: Option<HealthCheck> = None;
    let mut waiting = VecDeque::new();
    let mut backoff = Backoff::Idle;
    let mut ticker = tokio::time::interval(HEALTH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let retry_in = match backoff {
            Backoff::Reconnecting { attempt, .. } if opening.is_none() => {
                Some(reconnect_backoff(attempt))
            }
            _ => None,
        };
        tokio::select! {
            command = commands.recv() => match command {
                Ok(Command::Request { body, reply }) => {
                    // Opening previously held the command loop. Keep requests in that same
                    // admission window while allowing shutdown and connection progress.
                    if opening.is_some() {
                        waiting.push_back((body, reply));
                    } else if requests.try_send(requests::Request {
                        client: link.as_ref().map(|link| link.client.clone()), body, reply,
                    }).is_err() {
                        return;
                    }
                }
                Ok(Command::Reconnect) => {
                    if link.is_none() && opening.is_none() {
                        reason = OpeningReason::Manual;
                        opening = Some(Box::pin(open(home, events)));
                    }
                }
                Ok(Command::Shutdown) | Err(_) => return,
            },
            result = wait(&mut opening) => {
                opening = None;
                let event = match result {
                    Ok((connected, snapshot)) => {
                        let event = match reason {
                            OpeningReason::Initial | OpeningReason::Manual => BridgeEvent::Connected(Box::new(snapshot)),
                            OpeningReason::Retry { previous_pid, .. } => BridgeEvent::Reconnected {
                                restarted: connected.pid != previous_pid,
                                snapshot: Box::new(snapshot),
                            },
                        };
                        link = Some(connected);
                        backoff = Backoff::Idle;
                        event
                    }
                    Err(failure) => match reason {
                        OpeningReason::Initial | OpeningReason::Manual => failure.into_event(),
                        OpeningReason::Retry { previous_pid, attempt } => {
                            let attempt = attempt.saturating_add(1);
                            backoff = Backoff::Reconnecting { previous_pid, attempt };
                            BridgeEvent::Disconnected { attempt }
                        }
                    },
                };
                if events.try_send(event).is_err() { return; }
                for (body, reply) in waiting.drain(..) {
                    if requests.try_send(requests::Request {
                        client: link.as_ref().map(|link| link.client.clone()), body, reply,
                    }).is_err() { return; }
                }
            },
            (previous_pid, alive) = wait(&mut health) => {
                health = None;
                if !alive {
                    link = None;
                    backoff = Backoff::Reconnecting { previous_pid, attempt: 0 };
                    if events.try_send(BridgeEvent::Disconnected { attempt: 0 }).is_err() { return; }
                }
            },
            () = after(retry_in) => {
                if let Backoff::Reconnecting { previous_pid, attempt } = backoff {
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
            }
        }
    }
}
