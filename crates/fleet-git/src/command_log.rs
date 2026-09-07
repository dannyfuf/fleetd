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
    /// The caller stopped waiting and the child was killed and reaped.
    Cancelled,
    /// The child produced more output than the runner is allowed to retain.
    OutputLimitExceeded,
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
    let visible = String::from_utf8_lossy(&bytes[..bytes.len().min(limit)]);
    let redacted = redact_arg(&visible);
    redacted.as_bytes()[..redacted.len().min(limit)].to_vec()
}

pub(crate) fn redact_arg(argument: &str) -> String {
    let mut redacted = String::with_capacity(argument.len());
    let mut cursor = 0;
    while let Some(relative_scheme) = argument[cursor..].find("://") {
        let authority_start = cursor + relative_scheme + 3;
        let authority_end = argument[authority_start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '/' | '?' | '#')
            })
            .map_or(argument.len(), |end| authority_start + end);
        let Some(relative_at) = argument[authority_start..authority_end].rfind('@') else {
            redacted.push_str(&argument[cursor..authority_end]);
            cursor = authority_end;
            continue;
        };
        redacted.push_str(&argument[cursor..authority_start]);
        redacted.push_str("[REDACTED]");
        cursor = authority_start + relative_at;
    }
    redacted.push_str(&argument[cursor..]);
    redacted
}

#[cfg(test)]
mod tests {
    use super::{preview, redact_arg};

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

    #[test]
    fn preview_redacts_without_mutating_output() {
        let output = b"remote: https://user:secret@example.com/org/repo\n\
                       mirror: ssh://token@git.example.test/org/repo\n"
            .to_vec();

        let displayed = preview(&output, 1024);

        assert_eq!(
            displayed,
            b"remote: https://[REDACTED]@example.com/org/repo\n\
              mirror: ssh://[REDACTED]@git.example.test/org/repo\n"
        );
        assert_eq!(
            output,
            b"remote: https://user:secret@example.com/org/repo\n\
              mirror: ssh://token@git.example.test/org/repo\n"
        );
    }
}
