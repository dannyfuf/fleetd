//! The line wire both scripted providers share: one JSON object per line, in both directions.
//!
//! Blocking `std::io` rather than tokio, and that is deliberate. The scripted agent is a strictly
//! sequential peer — it answers one frame, plays the steps that frame released, and only reads
//! again when it needs the next request or a gate answer — so there is nothing for an executor to
//! interleave. `run_transcript` fences the whole loop behind `spawn_blocking`, which is where
//! blocking IO belongs; making it async would also need tokio's `io-std` feature, which this
//! crate's manifest does not take.

use std::{
    io::{BufRead, Write},
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

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

/// The client input before and after the first bounded read.
enum Source<R> {
    Direct(R),
    Threaded(mpsc::Receiver<std::io::Result<String>>),
}

/// A peer's two halves plus its pacing policy.
pub(crate) struct Peer<R, W> {
    source: Option<Source<R>>,
    writer: W,
    pace: Pace,
    written: usize,
}

impl<R: BufRead + Send + 'static, W: Write> Peer<R, W> {
    /// A peer reading from `reader` and answering on `writer`.
    pub(crate) const fn new(reader: R, writer: W, pace: Pace) -> Self {
        Self {
            source: Some(Source::Direct(reader)),
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
            let Some(line) = self.read_line()? else {
                return Ok(None);
            };
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

    /// `read_frame`, but fails with a named error if no frame arrives within `budget`.
    #[cfg(test)]
    pub(crate) fn read_frame_within(&mut self, budget: Duration) -> anyhow::Result<Option<Value>> {
        let deadline = Instant::now()
            .checked_add(budget)
            .ok_or_else(|| anyhow::anyhow!("the frame deadline overflows Instant"))?;
        self.read_frame_before(deadline, budget)
    }

    /// `read_frame`, bounded by one absolute deadline shared by every line in a gate wait.
    pub(crate) fn read_frame_before(
        &mut self,
        deadline: Instant,
        budget: Duration,
    ) -> anyhow::Result<Option<Value>> {
        self.start_reader_thread()?;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or_else(|| anyhow::anyhow!("no frame from the client within {budget:?}"))?;
            let line = match self.source.as_ref() {
                Some(Source::Threaded(receiver)) => match receiver.recv_timeout(remaining) {
                    Ok(line) => line.map(Some).map_err(|error| {
                        anyhow::anyhow!("read a frame from the client: {error}")
                    })?,
                    Err(RecvTimeoutError::Disconnected) => None,
                    Err(RecvTimeoutError::Timeout) => {
                        anyhow::bail!("no frame from the client within {budget:?}")
                    }
                },
                Some(Source::Direct(_)) => {
                    anyhow::bail!("the client reader was not moved to its reader thread")
                }
                None => anyhow::bail!("the client reader is unavailable"),
            };
            let Some(line) = line else {
                return Ok(None);
            };
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

    fn read_line(&mut self) -> anyhow::Result<Option<String>> {
        match self.source.as_mut() {
            Some(Source::Direct(reader)) => {
                let mut line = String::new();
                let read = reader
                    .read_line(&mut line)
                    .map_err(|error| anyhow::anyhow!("read a frame from the client: {error}"))?;
                Ok((read != 0).then_some(line))
            }
            Some(Source::Threaded(receiver)) => match receiver.recv() {
                Ok(line) => line
                    .map(Some)
                    .map_err(|error| anyhow::anyhow!("read a frame from the client: {error}")),
                Err(_) => Ok(None),
            },
            None => anyhow::bail!("the client reader is unavailable"),
        }
    }

    fn start_reader_thread(&mut self) -> anyhow::Result<()> {
        if matches!(self.source, Some(Source::Threaded(_))) {
            return Ok(());
        }
        let source = self
            .source
            .take()
            .ok_or_else(|| anyhow::anyhow!("the client reader is unavailable"))?;
        let Source::Direct(mut reader) = source else {
            anyhow::bail!("the client reader was not direct before its reader thread started")
        };
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("fleet-harness-agent-reader".to_owned())
            .spawn(move || {
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => return,
                        Ok(_) => {
                            // The receiver is gone only when the peer is shutting down.
                            if sender.send(Ok(line)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            // The receiver is gone only when the peer is shutting down.
                            if sender.send(Err(error)).is_err() {
                                return;
                            }
                            return;
                        }
                    }
                }
            })
            .map_err(|error| anyhow::anyhow!("start the client reader thread: {error}"))?;
        // A detached blocking reader may outlive its peer, but never the harness process.
        drop(handle);
        self.source = Some(Source::Threaded(receiver));
        Ok(())
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

#[cfg(test)]
mod tests {
    use std::{
        io::{self, BufReader, Cursor, Read},
        time::{Duration, Instant},
    };

    use serde_json::json;

    use super::{Pace, Peer};

    #[test]
    fn a_cursor_delivers_frames_before_during_and_after_threading() {
        let input = br#"{"path":"direct"}
{"path":"bounded"}
{"path":"threaded"}
"#;
        let mut peer = Peer::new(Cursor::new(input), Vec::<u8>::new(), Pace::Instant);

        let direct = peer
            .read_frame()
            .unwrap_or_else(|error| panic!("read the direct frame: {error}"));
        assert_eq!(direct, Some(json!({"path": "direct"})));

        let bounded = peer
            .read_frame_within(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("read the bounded frame: {error}"));
        assert_eq!(bounded, Some(json!({"path": "bounded"})));

        let threaded = peer
            .read_frame()
            .unwrap_or_else(|error| panic!("read the threaded frame: {error}"));
        assert_eq!(threaded, Some(json!({"path": "threaded"})));
    }

    #[cfg(unix)]
    #[test]
    fn a_held_open_socket_times_out_with_the_named_error() {
        use std::{os::unix::net::UnixStream, sync::mpsc, thread};

        let (client, server) =
            UnixStream::pair().unwrap_or_else(|error| panic!("create a socket pair: {error}"));
        let budget = Duration::from_millis(20);
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("fleet-harness-peer-timeout-test".to_owned())
            .spawn(move || {
                let mut peer = Peer::new(BufReader::new(server), Vec::<u8>::new(), Pace::Instant);
                let result = peer.read_frame_within(budget);
                sender
                    .send(result)
                    .unwrap_or_else(|error| panic!("return the bounded read result: {error}"));
            })
            .unwrap_or_else(|error| panic!("start the bounded read test thread: {error}"));

        let result = receiver.recv_timeout(Duration::from_secs(1));
        drop(client);
        worker
            .join()
            .unwrap_or_else(|_| panic!("the bounded read test thread panicked"));
        let error = result
            .unwrap_or_else(|error| panic!("the bounded read exceeded its watchdog: {error}"))
            .err()
            .unwrap_or_else(|| panic!("an unanswered socket must time out"));

        assert_eq!(
            error.to_string(),
            format!("no frame from the client within {budget:?}")
        );
    }

    #[test]
    fn eof_after_threading_is_none_for_bounded_and_blocking_reads() {
        let mut peer = Peer::new(
            Cursor::new(Vec::<u8>::new()),
            Vec::<u8>::new(),
            Pace::Instant,
        );

        let bounded = peer
            .read_frame_within(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("read bounded EOF: {error}"));
        assert_eq!(bounded, None);

        let blocking = peer
            .read_frame()
            .unwrap_or_else(|error| panic!("read blocking EOF: {error}"));
        assert_eq!(blocking, None);
    }

    #[test]
    fn non_json_lines_are_skipped_on_direct_and_threaded_sources() {
        let input = b"not json\n{\"path\":\"direct\"}\n";
        let mut direct = Peer::new(Cursor::new(input), Vec::<u8>::new(), Pace::Instant);
        let frame = direct
            .read_frame()
            .unwrap_or_else(|error| panic!("read after direct noise: {error}"));
        assert_eq!(frame, Some(json!({"path": "direct"})));

        let input = b"still not json\n{\"path\":\"threaded\"}\n";
        let mut threaded = Peer::new(Cursor::new(input), Vec::<u8>::new(), Pace::Instant);
        let frame = threaded
            .read_frame_within(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("read after threaded noise: {error}"));
        assert_eq!(frame, Some(json!({"path": "threaded"})));
    }

    #[test]
    fn an_expired_absolute_deadline_does_not_consume_queued_noise() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Ok("not json\n".to_owned()))
            .expect("queue noise");
        sender
            .send(Ok("{\"path\":\"too-late\"}\n".to_owned()))
            .expect("queue a later frame");
        let mut peer: Peer<Cursor<Vec<u8>>, Vec<u8>> = Peer {
            source: Some(super::Source::Threaded(receiver)),
            writer: Vec::<u8>::new(),
            pace: Pace::Instant,
            written: 0,
        };

        let error = peer
            .read_frame_before(Instant::now(), Duration::from_secs(5))
            .expect_err("an expired gate cannot be revived by queued input");

        assert_eq!(error.to_string(), "no frame from the client within 5s");
    }

    #[cfg(unix)]
    #[test]
    fn periodic_malformed_lines_cannot_restart_the_bounded_read() {
        use std::{io::Write as _, os::unix::net::UnixStream, thread};

        let (mut client, server) =
            UnixStream::pair().unwrap_or_else(|error| panic!("create a socket pair: {error}"));
        let writer = thread::spawn(move || {
            for _ in 0..12 {
                if writeln!(client, "not json").is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let mut peer = Peer::new(BufReader::new(server), Vec::<u8>::new(), Pace::Instant);

        let error = peer
            .read_frame_within(Duration::from_millis(20))
            .expect_err("periodic malformed lines must not renew the original deadline");
        writer
            .join()
            .unwrap_or_else(|_| panic!("the malformed-line writer panicked"));

        assert_eq!(error.to_string(), "no frame from the client within 20ms");
    }

    #[test]
    fn a_threaded_read_error_is_forwarded_once_and_then_becomes_eof() {
        let mut peer = Peer::new(
            BufReader::new(ErrorOnce(false)),
            Vec::<u8>::new(),
            Pace::Instant,
        );

        let error = peer
            .read_frame_within(Duration::from_secs(1))
            .err()
            .unwrap_or_else(|| panic!("the reader error must be forwarded"));
        assert_eq!(
            error.to_string(),
            "read a frame from the client: scripted read failure"
        );

        let eof = peer
            .read_frame()
            .unwrap_or_else(|error| panic!("read EOF after the forwarded error: {error}"));
        assert_eq!(eof, None);
    }

    struct ErrorOnce(bool);

    impl Read for ErrorOnce {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                Ok(0)
            } else {
                self.0 = true;
                Err(io::Error::other("scripted read failure"))
            }
        }
    }
}
