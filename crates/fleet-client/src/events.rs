//! Subscription and delivery of daemon event streams.

use fleet_proto::{event::Event, job::JobRecord, snapshot::Snapshot};
use tokio::sync::{broadcast, mpsc};

const FILTER_CAPACITY: usize = 128;

/// Filtering helpers for a daemon event receiver.
pub trait EventReceiverExt {
    /// Produces a channel containing only job updates.
    fn jobs(self) -> mpsc::Receiver<JobRecord>;

    /// Produces a channel containing only changed snapshots.
    fn snapshots(self) -> mpsc::Receiver<Snapshot>;
}

impl EventReceiverExt for broadcast::Receiver<Event> {
    fn jobs(self) -> mpsc::Receiver<JobRecord> {
        jobs(self)
    }

    fn snapshots(self) -> mpsc::Receiver<Snapshot> {
        snapshots(self)
    }
}

/// Filters an event receiver down to background-job updates.
#[must_use]
pub fn jobs(mut events: broadcast::Receiver<Event>) -> mpsc::Receiver<JobRecord> {
    let (sender, receiver) = mpsc::channel(FILTER_CAPACITY);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(Event::JobUpdated(job)) => {
                    if sender.send(job).await.is_err() {
                        return;
                    }
                }
                Ok(Event::DaemonShuttingDown) | Err(broadcast::error::RecvError::Closed) => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    });
    receiver
}

/// Filters an event receiver down to authoritative snapshot changes.
#[must_use]
pub fn snapshots(mut events: broadcast::Receiver<Event>) -> mpsc::Receiver<Snapshot> {
    let (sender, receiver) = mpsc::channel(FILTER_CAPACITY);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(Event::SnapshotChanged(snapshot)) => {
                    if sender.send(snapshot).await.is_err() {
                        return;
                    }
                }
                Ok(Event::DaemonShuttingDown) | Err(broadcast::error::RecvError::Closed) => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    });
    receiver
}
