//! Incremental parser for OpenCode's SSE event stream.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct WireEvent {
    #[allow(dead_code)]
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) properties: Value,
}

/// How much unterminated data one event may accumulate before it is abandoned.
///
/// The budget covers both halves of the parser: the bytes of a line that never ends, and the
/// `data:` lines of an event that never reaches its blank separator. A stream that never sends
/// another newline — or never sends another *blank* one — must not grow the parser without
/// bound; 8 MiB is far past any real OpenCode frame, so hitting it means framing itself is lost.
const MAX_PENDING: usize = 8 * 1024 * 1024;

/// What one chunk of the stream decoded to.
///
/// §4.3: unknown message types and fields are kept as diagnostics and never break the stream.
/// A single malformed payload therefore costs its own event and nothing else — the events that
/// arrived in the same TCP chunk, one of which may be the `session.status idle` that settles the
/// turn, are still delivered.
#[derive(Debug, Default, PartialEq)]
pub(super) struct SseBatch {
    pub(super) events: Vec<WireEvent>,
    pub(super) diagnostics: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct SseParser {
    bytes: Vec<u8>,
    data: Vec<String>,
    data_len: usize,
}

impl SseParser {
    pub(super) fn push(&mut self, chunk: &[u8]) -> SseBatch {
        self.bytes.extend_from_slice(chunk);
        let mut batch = SseBatch::default();
        while let Some(newline) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let mut line = self.bytes.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = self.decode(line, &mut batch);
            self.consume_line(&line, &mut batch);
        }
        if self.bytes.len() > MAX_PENDING {
            self.abandon("an unterminated line", &mut batch);
        }
        batch
    }

    pub(super) fn finish(&mut self) -> SseBatch {
        let mut batch = SseBatch::default();
        if !self.bytes.is_empty() {
            let bytes = std::mem::take(&mut self.bytes);
            let line = self.decode(bytes, &mut batch);
            self.consume_line(line.trim_end_matches('\r'), &mut batch);
        }
        self.flush(&mut batch);
        batch
    }

    /// Lossy on purpose: a byte the stream mangled costs its own character, not the frame.
    fn decode(&self, bytes: Vec<u8>, batch: &mut SseBatch) -> String {
        match String::from_utf8(bytes) {
            Ok(line) => line,
            Err(error) => {
                batch
                    .diagnostics
                    .push(format!("OpenCode SSE contained non-UTF-8 data: {error}"));
                String::from_utf8_lossy(error.as_bytes()).into_owned()
            }
        }
    }

    fn consume_line(&mut self, line: &str, batch: &mut SseBatch) {
        if line.is_empty() {
            self.flush(batch);
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            // The joined payload, plus the newline `flush` puts between two `data:` lines.
            self.data_len += data.len().saturating_add(1);
            self.data.push(data.to_owned());
            if self.data_len > MAX_PENDING {
                self.abandon("an event with no blank separator", batch);
            }
        }
    }

    /// Drops both buffers and reports why; the next blank line starts a clean event.
    fn abandon(&mut self, what: &str, batch: &mut SseBatch) {
        self.bytes.clear();
        self.data.clear();
        self.data_len = 0;
        batch.diagnostics.push(format!(
            "OpenCode SSE dropped {what} larger than {MAX_PENDING} bytes"
        ));
    }

    fn flush(&mut self, batch: &mut SseBatch) {
        if self.data.is_empty() {
            return;
        }
        let data = self.data.join("\n");
        self.data.clear();
        self.data_len = 0;
        match serde_json::from_str(&data) {
            Ok(event) => batch.events.push(event),
            Err(error) => batch.diagnostics.push(format!(
                "decode OpenCode SSE event: {error}; payload `{data}`"
            )),
        }
    }
}
