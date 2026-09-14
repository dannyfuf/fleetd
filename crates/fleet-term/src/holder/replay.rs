//! The bounded tail of child output a holder keeps so a reconnecting daemon can re-render.

use std::collections::VecDeque;

/// The most recent bytes written by a held child, capped at a byte budget.
///
/// The buffer is a raw byte tail, not parsed terminal state: a reattaching daemon feeds it to a
/// fresh emulator, which is why one buffer serves every client size and every emulator version.
/// Truncation can cut an escape sequence in half; the emulator resynchronises at the next one, so
/// at most the first sequence of a replay is lost.
pub(crate) struct ReplayBuffer {
    capacity: usize,
    bytes: VecDeque<u8>,
}

impl ReplayBuffer {
    /// Creates a buffer retaining at most `capacity` bytes.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            bytes: VecDeque::new(),
        }
    }

    /// Appends a chunk, evicting the oldest bytes past the budget.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.capacity == 0 {
            return;
        }
        // A chunk larger than the whole budget replaces it: only its own tail can survive.
        let chunk = match chunk.len().checked_sub(self.capacity) {
            Some(excess) if excess > 0 => &chunk[excess..],
            _ => chunk,
        };
        let overflow = (self.bytes.len() + chunk.len()).saturating_sub(self.capacity);
        self.bytes.drain(..overflow.min(self.bytes.len()));
        self.bytes.extend(chunk);
    }

    /// Returns the retained bytes, oldest first.
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_everything_below_the_budget() {
        let mut buffer = ReplayBuffer::new(16);
        buffer.push(b"hello ");
        buffer.push(b"world");
        assert_eq!(buffer.snapshot(), b"hello world".to_vec());
    }

    #[test]
    fn keeps_the_newest_bytes_when_the_budget_is_exceeded() {
        let mut buffer = ReplayBuffer::new(8);
        buffer.push(b"0123456789");
        assert_eq!(buffer.snapshot(), b"23456789".to_vec());
        buffer.push(b"abc");
        assert_eq!(buffer.snapshot(), b"56789abc".to_vec());
    }

    #[test]
    fn a_chunk_larger_than_the_budget_keeps_only_its_own_tail() {
        let mut buffer = ReplayBuffer::new(4);
        buffer.push(b"old");
        buffer.push(b"0123456789");
        assert_eq!(buffer.snapshot(), b"6789".to_vec());
    }

    #[test]
    fn a_zero_budget_retains_nothing() {
        let mut buffer = ReplayBuffer::new(0);
        buffer.push(b"anything");
        assert!(buffer.snapshot().is_empty());
    }
}
