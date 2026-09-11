//! Line framing shared by both harnesses.
//!
//! Claude Code and `codex app-server` both speak newline-delimited JSON over stdio, and every
//! rule below is one framing bug someone has already shipped (spec A.2.1):
//!
//! - split on `\n` **only**; a bare `\r` in the middle of a line is data, not a terminator;
//! - strip exactly **one** trailing `\r`, so a `\r\r\n` line keeps the first one;
//! - blank and whitespace-only lines are **skipped**, never decode errors;
//! - a final unterminated line at EOF is **processed before** termination;
//! - there is no small line-length cap — one frame is legitimately megabytes — but there is a
//!   hard budget, because an unterminated line from a wedged child would otherwise grow the
//!   daemon's heap until the machine gives out.

/// One complete frame, or the reason there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A complete frame with its terminator and one trailing `\r` removed.
    Frame(String),
    /// A line past the byte budget. Its bytes are dropped; the count is a diagnostic.
    Oversized(usize),
}

/// Incremental `\n` splitter over a byte stream.
///
/// It owns the partial tail between reads, which is what makes the parameterised chunk-size test
/// meaningful: the same bytes fed one at a time and 1024 at a time must produce the same frames.
#[derive(Debug)]
pub struct LineSplitter {
    buffer: Vec<u8>,
    budget: usize,
    /// Set once the pending line passed the budget: its bytes are dropped through the next `\n`.
    oversized: bool,
    /// Bytes seen for the pending oversized line, reported once when it terminates.
    oversized_bytes: usize,
    finished: bool,
}

impl LineSplitter {
    /// A splitter that abandons any single line longer than `budget` bytes.
    #[must_use]
    pub fn new(budget: usize) -> Self {
        Self {
            buffer: Vec::new(),
            budget,
            oversized: false,
            oversized_bytes: 0,
            finished: false,
        }
    }

    /// Appends `bytes` and returns every frame they completed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut rest = bytes;
        while let Some(newline) = rest.iter().position(|byte| *byte == b'\n') {
            let (head, tail) = rest.split_at(newline);
            self.extend(head);
            if let Some(line) = self.take_line() {
                lines.push(line);
            }
            rest = &tail[1..];
        }
        self.extend(rest);
        lines
    }

    /// Returns the unterminated final line, if the stream ended holding one.
    ///
    /// Idempotent: a transport that calls it on both the EOF branch and the teardown branch must
    /// not emit the same frame twice.
    pub fn finish(&mut self) -> Option<Line> {
        if self.finished {
            return None;
        }
        self.finished = true;
        self.take_line()
    }

    fn extend(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.oversized {
            self.oversized_bytes = self.oversized_bytes.saturating_add(bytes.len());
            return;
        }
        if self.buffer.len().saturating_add(bytes.len()) > self.budget {
            self.oversized = true;
            self.oversized_bytes = self.buffer.len().saturating_add(bytes.len());
            self.buffer = Vec::new();
            return;
        }
        self.buffer.extend_from_slice(bytes);
    }

    fn take_line(&mut self) -> Option<Line> {
        if self.oversized {
            let bytes = self.oversized_bytes;
            self.oversized = false;
            self.oversized_bytes = 0;
            return Some(Line::Oversized(bytes));
        }
        if self.buffer.is_empty() {
            return None;
        }
        let mut line = std::mem::take(&mut self.buffer);
        // Exactly one `\r`: `{"a":1}\r\r` is a frame whose last byte is data.
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        // A mangled byte costs its character, never the frame.
        let text = String::from_utf8_lossy(&line).into_owned();
        if text.trim().is_empty() {
            return None;
        }
        Some(Line::Frame(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(input: &[u8], chunk: usize, budget: usize) -> Vec<Line> {
        let mut splitter = LineSplitter::new(budget);
        let mut lines = Vec::new();
        for slice in input.chunks(chunk.max(1)) {
            lines.extend(splitter.push(slice));
        }
        lines.extend(splitter.finish());
        lines
    }

    /// spec A.2.1: the parameterised framing test at 1-, 7- and 1024-byte chunks with multi-byte
    /// UTF-8 and an embedded `\r`. It is the test that catches essentially every framing bug.
    #[test]
    fn framing_is_identical_at_every_chunk_size() {
        // A bare `\r` mid-line, a `\r\n` terminator, a blank line, a whitespace-only line, a
        // multi-byte grapheme, and a final unterminated line.
        let input = "{\"a\":\"caf\u{e9}\r ok\"}\r\n\n   \n{\"b\":\"\u{1f680}\u{2014}\"}\n{\"c\":1}"
            .as_bytes();
        let expected = vec![
            Line::Frame("{\"a\":\"caf\u{e9}\r ok\"}".to_owned()),
            Line::Frame("{\"b\":\"\u{1f680}\u{2014}\"}".to_owned()),
            Line::Frame("{\"c\":1}".to_owned()),
        ];
        for chunk in [1_usize, 7, 1024] {
            assert_eq!(
                frames(input, chunk, 8 * 1024 * 1024),
                expected,
                "chunk size {chunk} framed differently"
            );
        }
    }

    #[test]
    fn one_trailing_carriage_return_is_stripped_and_the_second_is_data() {
        assert_eq!(
            frames(b"{\"a\":1}\r\r\n", 1, 1024),
            vec![Line::Frame("{\"a\":1}\r".to_owned())]
        );
    }

    #[test]
    fn a_final_unterminated_line_is_processed_before_termination() {
        let mut splitter = LineSplitter::new(1024);
        assert!(splitter.push(b"{\"tail\":true}").is_empty());
        assert_eq!(
            splitter.finish(),
            Some(Line::Frame("{\"tail\":true}".to_owned()))
        );
        // Idempotent: a transport that finishes twice must not replay the frame.
        assert_eq!(splitter.finish(), None);
    }

    #[test]
    fn blank_and_whitespace_only_lines_are_skipped_rather_than_failing() {
        assert!(frames(b"\n\n \t \n\r\n", 1, 1024).is_empty());
    }

    #[test]
    fn an_oversized_line_is_dropped_and_the_next_line_still_frames() {
        let mut input = vec![b'x'; 2048];
        input.push(b'\n');
        input.extend_from_slice(b"{\"after\":1}\n");
        let lines = frames(&input, 7, 1024);
        assert!(matches!(lines.first(), Some(Line::Oversized(bytes)) if *bytes > 1024));
        assert_eq!(lines.get(1), Some(&Line::Frame("{\"after\":1}".to_owned())));
    }

    #[test]
    fn a_megabyte_frame_is_not_capped_below_the_budget() {
        let payload = "y".repeat(2 * 1024 * 1024);
        let mut input = format!("{{\"big\":\"{payload}\"}}").into_bytes();
        input.push(b'\n');
        let lines = frames(&input, 64 * 1024, 8 * 1024 * 1024);
        assert!(matches!(lines.as_slice(), [Line::Frame(frame)] if frame.len() > 2 * 1024 * 1024));
    }
}
