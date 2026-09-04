//! Fan-out of daemon events to subscribed clients.

use fleet_proto::event::Event;
use tokio::sync::broadcast;

/// Cloneable daemon-wide event fan-out bus.
#[derive(Clone)]
pub struct BroadcastBus {
    sender: broadcast::Sender<Event>,
}

impl BroadcastBus {
    /// Creates a bus with bounded per-subscriber buffering.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Publishes an event and returns the number of active receivers.
    pub fn publish(&self, event: Event) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    /// Creates an independent event receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

impl Default for BroadcastBus {
    fn default() -> Self {
        Self::new(256)
    }
}
