//! Cooperative child-process watches and bounded, sequenced output.

use crate::ids::{SessionId, TerminalId};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, path::PathBuf};

/// Stable daemon-local watch identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WatchId(pub u64);

impl std::fmt::Display for WatchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::str::FromStr for WatchId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// A read-only observation of a cooperatively wrapped child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watch {
    /// Stable identifier.
    pub id: WatchId,
    /// Owning session.
    pub session: SessionId,
    /// Parent terminal.
    pub terminal: TerminalId,
    /// Display label.
    pub label: String,
    /// Child argv.
    pub command: Vec<String>,
    /// Child working directory.
    pub cwd: Option<PathBuf>,
    /// Child process identifier.
    pub pid: Option<u32>,
    /// RFC3339 start time.
    pub started_at: String,
    /// Observed lifecycle.
    pub status: WatchStatus,
}

/// Observed completion; never authorizes process control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchStatus {
    /// Wrapper is connected and has not reported exit.
    Running,
    /// Completed or interrupted (signal 9 on owner disconnect).
    Exited {
        /// Normal process exit code.
        code: Option<i32>,
        /// Terminating Unix signal.
        signal: Option<i32>,
    },
}

/// Original output channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// One immutable piece of output in daemon arrival order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchChunk {
    /// Zero-based sequence number.
    pub seq: u64,
    /// Original channel.
    pub stream: WatchStream,
    /// Lossy UTF-8 display copy.
    pub text: String,
}

/// Bounded output history; cursors are inclusive and never renumbered.
#[derive(Debug)]
pub struct WatchBuffer {
    chunks: VecDeque<WatchChunk>,
    bytes: usize,
    byte_limit: usize,
    chunk_limit: usize,
    next_seq: u64,
}
impl Default for WatchBuffer {
    fn default() -> Self {
        Self::new(1024 * 1024, 20_000)
    }
}
impl WatchBuffer {
    /// Sets byte and chunk retention limits. Oversized chunks are dropped whole.
    #[must_use]
    pub fn new(byte_limit: usize, chunk_limit: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            byte_limit,
            chunk_limit,
            next_seq: 0,
        }
    }
    /// Appends a chunk and evicts oldest chunks until both limits hold.
    pub fn append(&mut self, stream: WatchStream, text: String) {
        let chunk = WatchChunk {
            seq: self.next_seq,
            stream,
            text,
        };
        self.next_seq += 1;
        self.bytes += chunk.text.len();
        self.chunks.push_back(chunk);
        while self.bytes > self.byte_limit || self.chunks.len() > self.chunk_limit {
            if let Some(chunk) = self.chunks.pop_front() {
                self.bytes -= chunk.text.len();
            }
        }
    }
    /// Oldest available cursor, or next_seq when empty.
    #[must_use]
    pub fn first_retained_seq(&self) -> u64 {
        self.chunks.front().map_or(self.next_seq, |c| c.seq)
    }
    /// Cursor to use for the next catch-up request.
    #[must_use]
    pub const fn next_seq(&self) -> u64 {
        self.next_seq
    }
    /// Copies retained chunks at or after the inclusive cursor.
    #[must_use]
    pub fn tail(&self, from_seq: Option<u64>) -> Vec<WatchChunk> {
        self.chunks
            .iter()
            .filter(|c| c.seq >= from_seq.unwrap_or(0))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_preserves_cursors_and_drops_oversized_chunks() {
        let mut b = WatchBuffer::new(5, 2);
        b.append(WatchStream::Stdout, "abc".into());
        b.append(WatchStream::Stderr, "def".into());
        assert_eq!(b.first_retained_seq(), 1);
        assert_eq!(b.tail(Some(1))[0].text, "def");
        assert!(b.tail(Some(2)).is_empty());
        b.append(WatchStream::Stdout, "123456".into());
        assert!(b.tail(None).is_empty());
        assert_eq!(b.first_retained_seq(), 3);
        assert_eq!(b.next_seq(), 3);
        for _ in 0..3 {
            b.append(WatchStream::Stdout, String::new());
        }
        assert_eq!(b.first_retained_seq(), 4);
        assert_eq!(b.tail(None).len(), 2);
    }
}
