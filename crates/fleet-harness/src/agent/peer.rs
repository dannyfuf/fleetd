//! The line wire both scripted providers share: one JSON object per line, in both directions.
//!
//! Blocking `std::io` rather than tokio, and that is deliberate. The scripted agent is a strictly
//! sequential peer — it answers one frame, plays the steps that frame released, and only reads
//! again when it needs the next request or a gate answer — so there is nothing for an executor to
//! interleave. `run_transcript` fences the whole loop behind `spawn_blocking`, which is where
//! blocking IO belongs; making it async would also need tokio's `io-std` feature, which this
//! crate's manifest does not take.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::Value;

/// Whether text deltas really wait between frames.
///
/// Tests use [`Pace::Instant`] so a transcript with a 40 ms pace costs no wall time and cannot
/// make an assertion flaky; the real binary uses [`Pace::Real`] because the pacing is the point —
/// it is what lets a scenario `await` a half-streamed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pace {
    /// Sleep for the transcript's `pace_ms`.
    Real,
    /// Never sleep.
    Instant,
}

/// A peer's two halves plus its pacing policy.
pub(crate) struct Peer<R, W> {
    reader: R,
    writer: W,
    pace: Pace,
    written: usize,
}

impl<R: BufRead, W: Write> Peer<R, W> {
    /// A peer reading from `reader` and answering on `writer`.
    pub(crate) const fn new(reader: R, writer: W, pace: Pace) -> Self {
        Self {
            reader,
            writer,
            pace,
            written: 0,
        }
    }

    /// The next frame the client wrote, or `None` at end of input.
    ///
    /// A line that is not JSON is skipped rather than fatal: a real CLI reading a corrupt line
    /// does not take the session down with it, and neither should the stand-in.
    pub(crate) fn read_frame(&mut self) -> anyhow::Result<Option<Value>> {
        loop {
            let mut line = String::new();
            let read = self
                .reader
                .read_line(&mut line)
                .map_err(|error| anyhow::anyhow!("read a frame from the client: {error}"))?;
            if read == 0 {
                return Ok(None);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(frame) => return Ok(Some(frame)),
                Err(_) => continue,
            }
        }
    }

    /// Writes one compact JSON frame and flushes it.
    ///
    /// Flushing every frame is what makes pacing observable: a buffered stream would deliver a
    /// whole streamed message in one burst and the pacing would be a lie.
    pub(crate) fn emit(&mut self, frame: &Value) -> anyhow::Result<()> {
        let line = serde_json::to_string(frame)
            .map_err(|error| anyhow::anyhow!("serialize a scripted frame: {error}"))?;
        writeln!(self.writer, "{line}")
            .map_err(|error| anyhow::anyhow!("write a scripted frame: {error}"))?;
        self.writer
            .flush()
            .map_err(|error| anyhow::anyhow!("flush a scripted frame: {error}"))?;
        self.written = self.written.saturating_add(1);
        Ok(())
    }

    /// Waits `millis` between two text deltas, unless the peer is running instantly.
    pub(crate) fn pause(&self, millis: u64) {
        if millis == 0 || self.pace == Pace::Instant {
            return;
        }
        std::thread::sleep(Duration::from_millis(millis));
    }

    /// How many frames this peer has written.
    #[cfg(test)]
    pub(crate) const fn written(&self) -> usize {
        self.written
    }
}
