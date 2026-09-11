//! Test-only helpers that make the privacy rules assertable.
//!
//! Two of the three privacy tests per harness (spec A.7.6) are about *log lines*, not about
//! return values, so they need a subscriber. [`logged`] installs one for the duration of a
//! closure and hands back everything the harness wrote, so a test can assert that a payload —
//! and every substring of one — never reached it.

use std::{
    io,
    sync::{Arc, Mutex, PoisonError},
};

use tracing_subscriber::fmt::MakeWriter;

/// A shared byte buffer a `tracing` subscriber can write into.
#[derive(Clone, Default)]
pub(crate) struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl io::Write for Buffer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut sink = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        sink.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for Buffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

/// Runs `body` with a capturing subscriber installed and returns its result and the log text.
pub(crate) fn logged<T>(body: impl FnOnce() -> T) -> (T, String) {
    let buffer = Buffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let value = tracing::subscriber::with_default(subscriber, body);
    (value, buffer.text())
}

/// Substrings that must never appear in a log line, an error, or an event.
///
/// Every one is a plausible thing an agent transcript carries: a token, a customer name, a file
/// body, a command with an argument that is itself a secret.
pub(crate) const SECRETS: [&str; 4] = [
    "sk-live-DEADBEEF-not-a-real-token",
    "ACME Holdings quarterly revenue",
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "psql://user:hunter2@db.internal/prod",
];

/// Asserts that no secret from [`SECRETS`] appears anywhere in `text`.
pub(crate) fn assert_no_secret(text: &str, what: &str) {
    for secret in SECRETS {
        assert!(
            !text.contains(secret),
            "{what} leaked wire data ({secret:?} appeared in {text:?})"
        );
    }
}
