//! The pagination cursor for a windowed transcript read.
//!
//! One integer, `(thread_id, before_seq)`, and that is the whole design. It is available as one
//! integer because Fleet's [`Seq`] is per thread, dense from 1, assigned by the same reducer that
//! writes the row, and the log is append-only — so the sequence the reducer stamped on the oldest
//! event already delivered *is* a total order over everything older. t3code's
//! `(anchor_timestamp, turn_id)` keyset exists only because its sequence is global and its
//! projection rows get rewritten under it; porting that pair here would buy nothing and cost two
//! comparisons, two columns and a tie-break rule.
//!
//! Two properties carry the weight:
//!
//! - **Decoding is total.** A malformed token, an unknown version, or a token minted against a
//!   different thread decodes to `None`, and a `None` cursor means "serve the first page". A
//!   stale cursor after a reconnect must degrade to "reload recent history", never to an error.
//! - **The thread is embedded.** A cursor can therefore never be replayed against another
//!   thread, which is the failure a bare integer would leave open.
//!
//! The encoding is deliberately plain ASCII rather than `base64url(serde_json)`: the token is
//! opaque to every client either way, it already travels as a wire `String`, and a second
//! encoding layer would earn a workspace dependency for nothing.

use std::fmt;

use fleet_core::agents::{Seq, ThreadId};

/// Encoding version. An unknown version decodes to `None`, so this can change without a
/// capability bump — a client holding an older token simply gets the first page back.
const CURSOR_VERSION: u8 = 1;

/// The token prefix, so a cursor is recognisable in a log line or a bug report.
const CURSOR_TAG: &str = "fat";

/// An opaque, exclusive keyset cursor into one thread's transcript.
///
/// "Exclusive" is the load-bearing word: [`TranscriptCursor::before_seq`] is the sequence of the
/// oldest event the client already has, and a page reads strictly *older* than it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptCursor {
    thread: ThreadId,
    before_seq: Seq,
}

impl TranscriptCursor {
    /// A cursor that reads strictly older than `before_seq` in `thread`.
    pub(crate) const fn new(thread: ThreadId, before_seq: Seq) -> Self {
        Self { thread, before_seq }
    }

    /// The exclusive upper bound of the page this cursor opens.
    pub(crate) const fn before_seq(self) -> Seq {
        self.before_seq
    }

    /// Decodes a client-supplied token, or `None` when it cannot serve `thread`.
    ///
    /// Every rejection — a wrong tag, an unknown version, an unparsable sequence, a token from
    /// another thread — is the same answer, because the caller's response to all of them is
    /// identical: read the newest page. Returning an error instead would turn a reconnect after
    /// a restart into a visible failure on a thread that is perfectly readable.
    pub(crate) fn decode(token: &str, thread: ThreadId) -> Option<Self> {
        let mut parts = token.split('.');
        let tag = parts.next()?;
        let version = parts.next()?.parse::<u8>().ok()?;
        let encoded_thread = parts.next()?.parse::<ThreadId>().ok()?;
        let before_seq = parts.next()?.parse::<u64>().ok()?;
        if parts.next().is_some() || tag != CURSOR_TAG || version != CURSOR_VERSION {
            return None;
        }
        if encoded_thread != thread {
            tracing::debug!(
                thread = %thread,
                cursor_thread = %encoded_thread,
                "discarding a transcript cursor minted against another thread"
            );
            return None;
        }
        Some(Self {
            thread,
            before_seq: Seq(before_seq),
        })
    }
}

impl fmt::Display for TranscriptCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{CURSOR_TAG}.{CURSOR_VERSION}.{}.{}",
            self.thread, self.before_seq
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{CURSOR_TAG, CURSOR_VERSION, TranscriptCursor};
    use fleet_core::agents::{Seq, ThreadId};

    #[test]
    fn a_cursor_round_trips_through_its_token() {
        let thread = ThreadId::new();
        let cursor = TranscriptCursor::new(thread, Seq(4_711));

        let decoded = TranscriptCursor::decode(&cursor.to_string(), thread);

        assert_eq!(decoded, Some(cursor));
        assert_eq!(decoded.map(TranscriptCursor::before_seq), Some(Seq(4_711)));
    }

    #[test]
    fn a_cursor_from_another_thread_decodes_to_the_first_page() {
        // The thread is embedded precisely so this cannot be answered with another thread's
        // window, and the answer has to be "page one" rather than an error: the client that
        // sends it is usually one that just reconnected.
        let minted = TranscriptCursor::new(ThreadId::new(), Seq(12));

        assert_eq!(
            TranscriptCursor::decode(&minted.to_string(), ThreadId::new()),
            None
        );
    }

    #[test]
    fn every_malformed_token_decodes_to_the_first_page() {
        let thread = ThreadId::new();
        let next_version = CURSOR_VERSION.saturating_add(1);
        for token in [
            String::new(),
            "garbage".to_owned(),
            format!("{CURSOR_TAG}.{CURSOR_VERSION}.{thread}"),
            format!("{CURSOR_TAG}.{CURSOR_VERSION}.{thread}.not-a-number"),
            format!("{CURSOR_TAG}.{CURSOR_VERSION}.not-a-uuid.7"),
            format!("nope.{CURSOR_VERSION}.{thread}.7"),
            format!("{CURSOR_TAG}.{next_version}.{thread}.7"),
            format!("{CURSOR_TAG}.{CURSOR_VERSION}.{thread}.7.8"),
        ] {
            assert_eq!(
                TranscriptCursor::decode(&token, thread),
                None,
                "`{token}` must decode to the first page"
            );
        }
    }
}
