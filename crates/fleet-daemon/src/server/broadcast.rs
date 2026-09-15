//! Fan-out of daemon events to subscribed clients, including coalesced snapshots.

use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use fleet_proto::event::Event;
use tokio::sync::broadcast;

use crate::services::Services;

const SNAPSHOT_COALESCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(50);

struct BroadcastInner {
    sender: broadcast::Sender<Event>,
    snapshot_pending: AtomicBool,
    snapshot_revision: AtomicU64,
    services: Mutex<Weak<Services>>,
    runtime: Mutex<Option<tokio::runtime::Handle>>,
}

/// Cloneable daemon-wide event fan-out bus.
///
/// Every call to [`Self::request_snapshot`] or [`Self::request_snapshot_current`] must happen
/// after the announced state change is visible to [`Services::snapshot`]. This ordering makes a
/// response revision a causal lower bound for the snapshot that covers its request.
#[derive(Clone)]
pub struct BroadcastBus {
    inner: Arc<BroadcastInner>,
}

impl BroadcastBus {
    /// Creates a bus with bounded per-subscriber buffering.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity);
        Self {
            inner: Arc::new(BroadcastInner {
                sender,
                snapshot_pending: AtomicBool::new(false),
                snapshot_revision: AtomicU64::new(0),
                services: Mutex::new(Weak::new()),
                runtime: Mutex::new(None),
            }),
        }
    }

    /// Publishes an event and returns the number of active receivers.
    pub fn publish(&self, event: Event) -> usize {
        self.inner.sender.send(event).unwrap_or(0)
    }

    pub(crate) fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Creates an independent event receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.sender.subscribe()
    }

    /// Returns the latest requested snapshot revision.
    #[must_use]
    pub fn snapshot_revision(&self) -> u64 {
        self.inner.snapshot_revision.load(Ordering::Acquire)
    }

    /// Connects the bus to the authoritative snapshot assembler.
    pub fn attach_services(&self, services: Weak<Services>) {
        *self
            .inner
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = services;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            *self
                .inner
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(runtime);
        }
    }

    /// Requests a snapshot from the attached daemon facade, when available.
    pub fn request_snapshot_current(&self) {
        let services = self
            .inner
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade();
        if let Some(services) = services {
            self.request_snapshot(services);
        }
    }

    /// Requests an authoritative snapshot event, batching bursts into 50 ms windows.
    pub fn request_snapshot(&self, services: Arc<Services>) {
        self.inner.snapshot_revision.fetch_add(1, Ordering::AcqRel);
        if self
            .inner
            .snapshot_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        // PTY event threads have no Tokio reactor; use the runtime attached by the daemon.
        let runtime = tokio::runtime::Handle::try_current().ok().or_else(|| {
            self.inner
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        });
        let Some(runtime) = runtime else {
            self.inner.snapshot_pending.store(false, Ordering::Release);
            tracing::warn!("snapshot requested before a Tokio runtime was attached");
            return;
        };
        let events = self.clone();
        runtime.spawn(async move {
            tokio::time::sleep(SNAPSHOT_COALESCE_WINDOW).await;
            let assembled_revision = events.snapshot_revision();
            match services.snapshot().await {
                Ok(mut snapshot) => {
                    snapshot.revision = Some(assembled_revision);
                    events.publish(Event::SnapshotChanged(snapshot));
                }
                Err(error) => tracing::warn!(%error, "failed to assemble snapshot event"),
            }
            events
                .inner
                .snapshot_pending
                .store(false, Ordering::Release);
            if events.snapshot_revision() != assembled_revision {
                events.request_snapshot(services);
            }
        });
    }
}

impl Default for BroadcastBus {
    fn default() -> Self {
        Self::new(256)
    }
}
