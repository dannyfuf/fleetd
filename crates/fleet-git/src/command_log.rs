//! Bounded command history and live command events.

use std::{collections::VecDeque, sync::Mutex, time::SystemTime};

use tokio::sync::broadcast;

use crate::model::CommandKind;

/// Outcome stored in a command-log record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// The child has started and has not completed yet.
    Running,
    /// The child exited with an accepted status.
    Success {
        /// Numeric exit status.
        status: Option<i32>,
        /// Capped stdout bytes.
        stdout_preview: Vec<u8>,
        /// Capped stderr bytes.
        stderr_preview: Vec<u8>,
    },
    /// The child exited with a rejected status.
    Failed {
        /// Numeric exit status.
        status: Option<i32>,
        /// Capped stdout bytes.
        stdout_preview: Vec<u8>,
        /// Capped stderr bytes.
        stderr_preview: Vec<u8>,
    },
    /// The process exceeded its deadline.
    TimedOut,
    /// The child could not be spawned or communicated with.
    SpawnFailed(String),
}

/// Safe, bounded metadata for one Git invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    /// Monotonic runner-local identifier.
    pub id: u64,
    /// Read/mutation/network marker.
    pub kind: CommandKind,
    /// Redacted argv suitable for display.
    pub display_argv: Vec<String>,
    /// Wall-clock start time.
    pub started_at: SystemTime,
    /// Elapsed time once finished.
    pub elapsed: Option<std::time::Duration>,
    /// Current/final outcome.
    pub outcome: CommandOutcome,
}

/// Live command-log event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEvent {
    /// A child is about to be spawned.
    Started(CommandRecord),
    /// The child completed, timed out, or failed to spawn.
    Finished(CommandRecord),
}

/// Shared bounded command-log store.
#[derive(Debug)]
pub(crate) struct CommandLog {
    recent: Mutex<VecDeque<CommandRecord>>,
    events: broadcast::Sender<CommandEvent>,
    capacity: usize,
}

impl CommandLog {
    pub(crate) fn new(capacity: usize, event_capacity: usize) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        Self {
            recent: Mutex::new(VecDeque::with_capacity(capacity)),
            events,
            capacity,
        }
    }

    pub(crate) fn started(&self, record: CommandRecord) {
        self.store(record.clone());
        let _ = self.events.send(CommandEvent::Started(record));
    }

    pub(crate) fn finished(&self, record: CommandRecord) {
        self.store(record.clone());
        let _ = self.events.send(CommandEvent::Finished(record));
    }

    /// Updates the record with the same id, or appends it, dropping the oldest
    /// record once the history is full.
    fn store(&self, record: CommandRecord) {
        if self.capacity == 0 {
            return;
        }
        let mut recent = self
            .recent
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(existing) = recent.iter_mut().find(|item| item.id == record.id) {
            *existing = record;
            return;
        }
        if recent.len() == self.capacity {
            recent.pop_front();
        }
        recent.push_back(record);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<CommandEvent> {
        self.events.subscribe()
    }

    pub(crate) fn recent(&self) -> Vec<CommandRecord> {
        self.recent
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

pub(crate) fn preview(bytes: &[u8], limit: usize) -> Vec<u8> {
    bytes[..bytes.len().min(limit)].to_vec()
}

pub(crate) fn redact_arg(argument: &str) -> String {
    if let Some(scheme) = argument.find("://") {
        let authority = scheme + 3;
        if let Some(at) = argument[authority..].find('@') {
            let suffix = &argument[authority + at..];
            return format!("{}://[REDACTED]{}", &argument[..scheme], suffix);
        }
    }
    argument.to_owned()
}

#[cfg(test)]
mod tests {
    use super::redact_arg;

    #[test]
    fn redacts_url_userinfo() {
        assert_eq!(
            redact_arg("https://user:secret@example.com/org/repo"),
            "https://[REDACTED]@example.com/org/repo"
        );
        assert_eq!(
            redact_arg("git@example.com:org/repo"),
            "git@example.com:org/repo"
        );
    }
}
