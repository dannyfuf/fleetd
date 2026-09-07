use super::*;
use std::{collections::VecDeque, time::Duration};

const BYTE_CAP: usize = 1024 * 1024;
const LINE_CAP: usize = 20_000;

mod partial;

use partial::PartialLine;

/// A completed output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WatchLine {
    /// Original child output channel.
    pub(super) stream: WatchStream,
    /// Line text without its newline.
    pub(super) text: gpui::SharedString,
}

/// stderr reads back dimmer than stdout wherever retained output is shown.
const fn tone_for(stream: WatchStream) -> fleet_ui_kit::Tone {
    match stream {
        WatchStream::Stderr => fleet_ui_kit::Tone::Secondary,
        WatchStream::Stdout => fleet_ui_kit::Tone::Default,
    }
}

/// Prepared log lines and their tones, shared by every pane that shows one watch.
pub type WatchDisplay = (
    std::sync::Arc<[gpui::SharedString]>,
    std::sync::Arc<[fleet_ui_kit::Tone]>,
);

/// One child and its retained display history.
#[derive(Debug)]
pub struct WatchMirror {
    /// Latest known metadata. Completion never regresses to Running.
    pub watch: Watch,
    /// Completed newline-delimited lines.
    pub(super) lines: VecDeque<WatchLine>,
    /// Inclusive next expected chunk cursor.
    pub next_seq: u64,
    /// Earlier output is unavailable locally or on the daemon.
    pub trimmed: bool,
    partial: [PartialLine; 2],
    /// Prepared display arrays, built on the first read after output changed.
    display: std::cell::RefCell<Option<WatchDisplay>>,
    line_bytes: usize,
    pub(super) observed_next: u64,
    observed_at: Instant,
    initial_age: Duration,
    final_age: Option<Duration>,
}

impl WatchMirror {
    pub(super) fn new(watch: Watch, now: Instant) -> Self {
        let initial_age = chrono::DateTime::parse_from_rfc3339(&watch.started_at)
            .ok()
            .and_then(|start| {
                (chrono::Utc::now() - start.with_timezone(&chrono::Utc))
                    .to_std()
                    .ok()
            })
            .unwrap_or_default();
        let final_age = (watch.status != WatchStatus::Running).then_some(initial_age);
        Self {
            watch,
            lines: VecDeque::new(),
            next_seq: 0,
            trimmed: false,
            partial: Default::default(),
            display: std::cell::RefCell::default(),
            line_bytes: 0,
            observed_next: 0,
            observed_at: now,
            initial_age,
            final_age,
        }
    }

    /// Elapsed duration, frozen at the first observed completion.
    #[must_use]
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.final_age
            .unwrap_or_else(|| self.initial_age + now.saturating_duration_since(self.observed_at))
    }

    pub(super) fn metadata(&mut self, watch: Watch, now: Instant) {
        if self.watch.status != WatchStatus::Running && watch.status == WatchStatus::Running {
            return;
        }
        if watch.status != WatchStatus::Running && self.final_age.is_none() {
            self.final_age = Some(self.elapsed(now));
        }
        self.watch = watch;
    }

    fn append(&mut self, chunk: WatchChunk) {
        let index = usize::from(chunk.stream == WatchStream::Stderr);
        for part in chunk.text.split_inclusive('\n') {
            self.partial[index].push_str(part.strip_suffix('\n').unwrap_or(part));
            if part.ends_with('\n') {
                let mut text = self.partial[index].take_text();
                if text.ends_with('\r') {
                    text.pop();
                }
                self.line_bytes += text.len();
                self.lines.push_back(WatchLine {
                    stream: chunk.stream,
                    text: text.into(),
                });
            }
            self.bound();
        }
        self.next_seq += 1;
    }

    fn bound(&mut self) {
        while self.lines.len() + self.partial.iter().filter(|s| !s.is_empty()).count() > LINE_CAP
            || self.line_bytes + self.partial.iter().map(PartialLine::len).sum::<usize>() > BYTE_CAP
        {
            self.trimmed = true;
            if let Some(line) = self.lines.pop_front() {
                self.line_bytes -= line.text.len();
            } else {
                let total: usize = self.partial.iter().map(PartialLine::len).sum();
                let i = usize::from(self.partial[1].len() > self.partial[0].len());
                self.partial[i].trim_front(total - BYTE_CAP);
            }
        }
    }

    pub(super) fn chunks(&mut self, chunks: Vec<WatchChunk>) {
        let previous_seq = self.next_seq;
        for chunk in chunks {
            self.observed_next = self.observed_next.max(chunk.seq.saturating_add(1));
            if chunk.seq == self.next_seq {
                self.append(chunk);
            }
        }
        if self.next_seq != previous_seq {
            self.invalidate_display();
        }
    }

    /// Prepared immutable log storage, shared across renders until output changes.
    ///
    /// Built here rather than on append: a chatty child appends far more often than any pane
    /// reads it, and a watch no pane shows never builds these arrays at all.
    pub fn shared_display(&self) -> WatchDisplay {
        let mut cached = self.display.borrow_mut();
        cached.get_or_insert_with(|| self.prepare_display()).clone()
    }

    pub(super) fn clear_partial(&mut self) {
        self.partial = Default::default();
        self.invalidate_display();
    }

    /// Drops the prepared arrays; the next reader rebuilds them.
    fn invalidate_display(&mut self) {
        *self.display.borrow_mut() = None;
    }

    /// Completed lines followed by the live partial line of each stream.
    fn prepare_display(&self) -> WatchDisplay {
        let mut lines = Vec::with_capacity(self.lines.len() + 2);
        let mut tones = Vec::with_capacity(lines.capacity());
        for line in &self.lines {
            lines.push(line.text.clone());
            tones.push(tone_for(line.stream));
        }
        for (stream, partial) in [WatchStream::Stdout, WatchStream::Stderr]
            .into_iter()
            .zip(&self.partial)
        {
            if !partial.is_empty() {
                lines.push(partial.text().into());
                tones.push(tone_for(stream));
            }
        }
        (lines.into(), tones.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watches::tests::{chunk, watch};
    #[test]
    fn completed_and_partial_lines_are_bounded_including_multibyte_text() {
        let now = Instant::now();
        let mut mirror = WatchMirror::new(watch(1), now);
        mirror.append(chunk(
            0,
            WatchStream::Stdout,
            &"line\n".repeat(LINE_CAP + 1),
        ));
        assert!(mirror.trimmed);
        assert_eq!(mirror.lines.len(), LINE_CAP);
        mirror.append(chunk(1, WatchStream::Stderr, &"é".repeat(BYTE_CAP)));
        assert!(
            mirror.line_bytes + mirror.partial.iter().map(PartialLine::len).sum::<usize>()
                <= BYTE_CAP
        );
        assert!(mirror.lines.is_empty());
        assert!(
            mirror.partial[1]
                .text()
                .is_char_boundary(mirror.partial[1].len())
        );
    }
}
