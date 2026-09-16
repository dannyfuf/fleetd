//! Bounded, UTF-8-safe smoothing for bursty native-agent text delivery.

use std::collections::VecDeque;

use fleet_core::agents::{ItemId, StreamKind};

#[derive(Debug)]
struct PendingReveal {
    item: ItemId,
    stream: StreamKind,
    text: String,
    offset: usize,
}

/// One suffix ready to enter the presentation projection and its incremental row parser.
#[derive(Debug)]
pub(super) struct RevealChunk {
    pub(super) item: ItemId,
    pub(super) stream: StreamKind,
    pub(super) text: String,
}

/// Suffixes waiting to be painted for one thread view.
///
/// The authoritative projection is updated before anything enters this queue. This buffer owns
/// only the small suffixes whose rows deliberately trail it, never a second truth.
#[derive(Debug, Default)]
pub(super) struct RevealBuffer {
    pending: VecDeque<PendingReveal>,
    pending_bytes: usize,
    bytes_per_tick: usize,
}

impl RevealBuffer {
    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub(super) fn push(
        &mut self,
        item: ItemId,
        stream: StreamKind,
        text: String,
        tick_ms: u64,
        horizon_ms: u64,
    ) {
        if text.is_empty() {
            return;
        }
        self.pending_bytes = self.pending_bytes.saturating_add(text.len());
        let tick = usize::try_from(tick_ms).unwrap_or(usize::MAX);
        let numerator = self.pending_bytes.saturating_mul(tick);
        let horizon = usize::try_from(horizon_ms.max(1)).unwrap_or(usize::MAX);
        let rate = numerator.saturating_add(horizon - 1) / horizon;
        self.bytes_per_tick = self.bytes_per_tick.max(rate.max(1));
        self.pending.push_back(PendingReveal {
            item,
            stream,
            text,
            offset: 0,
        });
    }

    pub(super) fn take_tick(&mut self) -> Vec<RevealChunk> {
        self.take(self.bytes_per_tick.max(1))
    }

    pub(super) fn drain(&mut self) -> Vec<RevealChunk> {
        self.take(usize::MAX)
    }

    fn take(&mut self, mut budget: usize) -> Vec<RevealChunk> {
        let mut chunks = Vec::new();
        while budget > 0 {
            let Some(front) = self.pending.front_mut() else {
                break;
            };
            let available = front.text.len().saturating_sub(front.offset);
            if available == 0 {
                self.pending.pop_front();
                continue;
            }
            let mut length = available.min(budget);
            while length < available && !front.text.is_char_boundary(front.offset + length) {
                length += 1;
            }
            let end = front.offset + length;
            chunks.push(RevealChunk {
                item: front.item,
                stream: front.stream,
                text: front.text[front.offset..end].to_owned(),
            });
            front.offset = end;
            self.pending_bytes = self.pending_bytes.saturating_sub(length);
            budget = budget.saturating_sub(length);
            if front.offset == front.text.len() {
                self.pending.pop_front();
            }
        }
        if self.pending.is_empty() {
            self.pending_bytes = 0;
            self.bytes_per_tick = 0;
        }
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_chunks_end_on_utf8_boundaries_and_drain_within_the_horizon() {
        let mut buffer = RevealBuffer::default();
        buffer.push(
            ItemId::new(),
            StreamKind::AssistantText,
            "aé日z".to_owned(),
            16,
            200,
        );
        let mut revealed = String::new();
        for _ in 0..13 {
            for chunk in buffer.take_tick() {
                revealed.push_str(&chunk.text);
            }
        }
        assert_eq!(revealed, "aé日z");
        assert!(buffer.is_empty());
    }
}
