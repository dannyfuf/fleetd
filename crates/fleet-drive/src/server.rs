//! Unix-socket request transport for the application's harness driver.
//!
//! One newline-delimited JSON frame carries a request, one carries its correlated response, and
//! the runner reads each response before it sends the next command (`docs/TESTING-HARNESS.md`
//! §1). The listener owns a dedicated OS thread rather than a slot on GPUI's background pool:
//! `accept` and `read` block, and the pool's threads belong to the application's own work.
//!
//! The thread never touches an entity. It hands each decoded request to the window through
//! [`RequestChannel`] and blocks on the one-shot reply channel that travels with it, so a
//! command is answered only after the window has actually applied it.

use crate::protocol::{Request, Response};
use anyhow::Context as _;
use std::{
    io::{BufRead as _, BufReader, Write as _},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

/// How long a blocked read waits before the thread re-checks for shutdown.
///
/// This is not command latency — a frame that arrives wakes the read immediately. It only bounds
/// how long [`Server::drop`] waits for a thread parked inside a connected client's read.
const READ_POLL: Duration = Duration::from_millis(100);

/// How long a blocked write waits before it gives the connection up.
///
/// A client that stops reading mid-`dump` would otherwise park this thread inside `write_all`,
/// and `Server::drop` joins that thread — so the application's own shutdown would hang behind a
/// dead runner. Generous next to any real response, and bounded.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the listener waits before retrying an `accept` that failed transiently.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// The largest request frame the server will assemble, in bytes.
///
/// The frames this protocol defines are small — the longest legitimate one is a `type` line —
/// so a frame that grows past this is a client with no newline in it, and reading it to the end
/// means allocating until the OS kills Fleet. One mebibyte is three orders of magnitude above
/// anything the grammar can produce.
const MAX_FRAME: usize = 1024 * 1024;

/// The correlation id used when a frame could not be parsed and has no id to echo.
const UNCORRELATED: u64 = 0;

/// A request received off-thread and its one-response channel.
///
/// Dropping this without sending is how a closing window cancels an in-flight command: the
/// socket thread sees the reply channel close and answers the client with a failure.
#[derive(Debug)]
pub struct IncomingRequest {
    pub request: Request,
    pub reply: async_channel::Sender<Response>,
}

/// Application-facing receiving half of the harness server.
#[derive(Debug)]
pub struct RequestChannel {
    pub requests: async_channel::Receiver<IncomingRequest>,
}

/// Lifetime handle for the socket listener.
///
/// Dropping it stops the thread and unlinks the socket. Drop it *after* the matching
/// [`RequestChannel`], so a thread parked on a hand-off or a reply is released before the join.
#[derive(Debug)]
pub struct Server {
    pub path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Starts the newline-delimited JSON listener at `path`.
///
/// Binding happens before this returns, so a runner that spawned the app may connect as soon as
/// the socket file appears. One client is served at a time.
pub fn listen(path: &Path) -> anyhow::Result<(Server, RequestChannel)> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    // A socket left behind by an earlier run makes `bind` fail with EADDRINUSE. The harness owns
    // this path exclusively — the runner hands out a fresh one per run — so clearing it is safe.
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("could not clear {}", path.display()));
        }
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("could not listen on {}", path.display()))?;
    // Unbounded by design: the socket thread must never park on a send it cannot cancel, and the
    // runner keeps exactly one request in flight, so the queue never holds more than that plus
    // whatever a misbehaving client pipelines.
    let (requests_tx, requests_rx) = async_channel::unbounded();
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread = std::thread::Builder::new()
        .name("fleet-harness-socket".to_owned())
        .spawn({
            let shutdown = Arc::clone(&shutdown);
            move || accept_loop(&listener, &requests_tx, &shutdown)
        })
        .context("could not start the harness socket thread")?;
    Ok((
        Server {
            path: path.to_path_buf(),
            shutdown,
            thread: Some(thread),
        },
        RequestChannel {
            requests: requests_rx,
        },
    ))
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // `accept` parks until a peer arrives, so the loop needs one last connection to notice
        // the flag. A failure here means nothing is listening any more, which is the same
        // outcome, so the result is deliberately discarded.
        drop(UnixStream::connect(&self.path));
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("harness socket: the listener thread panicked");
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, path = %self.path.display(), "harness socket: could not unlink");
            }
        }
    }
}

/// Serves one client at a time until the server is dropped.
fn accept_loop(
    listener: &UnixListener,
    requests: &async_channel::Sender<IncomingRequest>,
    shutdown: &AtomicBool,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            // The wake-up connection from `Server::drop` lands here; the flag sends it away.
            Ok(_) if shutdown.load(Ordering::Acquire) => return,
            Ok((stream, _address)) => serve(stream, requests, shutdown),
            // A transient `accept` failure — the process out of descriptors, a client that hung
            // up between the connection and the accept — must not retire the listener: the
            // socket file stays on disk either way, so every later command would connect and
            // then hang waiting for a response nothing is going to write.
            Err(error) if is_retryable(&error) || is_transient_accept(&error) => {
                tracing::warn!(%error, "harness socket: accept failed, retrying");
                std::thread::sleep(ACCEPT_BACKOFF);
            }
            Err(error) => {
                tracing::warn!(%error, "harness socket: accept failed");
                return;
            }
        }
    }
}

/// Reads frames from one client, answering each before reading the next.
fn serve(
    stream: UnixStream,
    requests: &async_channel::Sender<IncomingRequest>,
    shutdown: &AtomicBool,
) {
    if let Err(error) = stream.set_read_timeout(Some(READ_POLL)) {
        tracing::warn!(%error, "harness socket: could not arm the read timeout");
        return;
    }
    if let Err(error) = stream.set_write_timeout(Some(WRITE_TIMEOUT)) {
        tracing::warn!(%error, "harness socket: could not arm the write timeout");
        return;
    }
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(error) => {
            tracing::warn!(%error, "harness socket: could not split the connection");
            return;
        }
    };
    let mut reader = BufReader::new(stream);
    loop {
        let (response, keep_going) = match read_frame(&mut reader, shutdown) {
            Frame::End => return,
            Frame::Line(frame) => respond(&frame, requests),
            // The connection is still framed — the overrun was drained to the next newline — so
            // the client is told what happened rather than left to time out on a dead socket.
            Frame::Oversized(bytes) => (
                Response::err(
                    UNCORRELATED,
                    format!("request frame exceeded {MAX_FRAME} bytes after {bytes} bytes"),
                ),
                true,
            ),
        };
        if let Err(error) = write_frame(&mut writer, &response) {
            tracing::warn!(%error, "harness socket: could not write a response");
            return;
        }
        if !keep_going {
            return;
        }
    }
}

/// What one read off the wire produced.
enum Frame {
    /// A complete newline-terminated frame.
    Line(String),
    /// A frame that grew past [`MAX_FRAME`]; the rest of it has been drained.
    Oversized(usize),
    /// The peer or the server is gone.
    End,
}

/// Answers one frame. Returns the response to write and whether the connection may continue.
fn respond(frame: &str, requests: &async_channel::Sender<IncomingRequest>) -> (Response, bool) {
    let request = match serde_json::from_str::<Request>(frame) {
        Ok(request) => request,
        // The id lives inside the frame that failed to parse, so there is nothing to echo.
        Err(error) => {
            return (
                Response::err(UNCORRELATED, format!("malformed request frame: {error}")),
                true,
            );
        }
    };
    let id = request.id;
    let (reply_tx, reply_rx) = async_channel::bounded(1);
    if requests
        .send_blocking(IncomingRequest {
            request,
            reply: reply_tx,
        })
        .is_err()
    {
        return (
            Response::err(
                id,
                "the application is no longer accepting harness commands",
            ),
            false,
        );
    }
    match reply_rx.recv_blocking() {
        Ok(response) => (response, true),
        // The driver dropped the request without answering it: its window closed mid-command.
        Err(_closed) => (
            Response::err(id, "the application closed before the command completed"),
            false,
        ),
    }
}

/// Reads one newline-terminated frame, bounded by [`MAX_FRAME`].
fn read_frame(reader: &mut BufReader<UnixStream>, shutdown: &AtomicBool) -> Frame {
    let mut buffer = Vec::new();
    loop {
        if shutdown.load(Ordering::Acquire) {
            return Frame::End;
        }
        let complete = match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => return Frame::End,
            Ok(_) => buffer.last() == Some(&b'\n'),
            // A timed-out or interrupted read keeps whatever it already appended to `buffer`.
            Err(error) if is_retryable(&error) => false,
            Err(error) => {
                tracing::warn!(%error, "harness socket: read failed");
                return Frame::End;
            }
        };
        // Checked before the frame is accepted, so a single oversized *complete* frame is
        // refused too rather than parsed.
        if buffer.len() > MAX_FRAME {
            let bytes = buffer.len();
            tracing::warn!(bytes, "harness socket: oversized request frame");
            if complete {
                return Frame::Oversized(bytes);
            }
            return match drain_to_newline(reader, shutdown) {
                true => Frame::Oversized(bytes),
                false => Frame::End,
            };
        }
        if complete {
            break;
        }
    }
    match String::from_utf8(buffer) {
        Ok(frame) => Frame::Line(frame),
        Err(error) => {
            tracing::warn!(%error, "harness socket: a frame was not valid utf-8");
            Frame::End
        }
    }
}

/// Discards bytes up to and including the next newline, so the connection stays framed.
///
/// `false` once the peer or the server is gone. The discarded bytes are never buffered, so
/// draining a runaway writer costs no memory however long it goes on.
fn drain_to_newline(reader: &mut BufReader<UnixStream>, shutdown: &AtomicBool) -> bool {
    let mut sink = Vec::new();
    loop {
        if shutdown.load(Ordering::Acquire) {
            return false;
        }
        sink.clear();
        match reader.read_until(b'\n', &mut sink) {
            Ok(0) => return false,
            Ok(_) if sink.last() == Some(&b'\n') => return true,
            Ok(_) => {}
            Err(error) if is_retryable(&error) => {}
            Err(error) => {
                tracing::warn!(%error, "harness socket: read failed while draining");
                return false;
            }
        }
    }
}

/// Whether an `accept` failure is one a later `accept` can still succeed after.
fn is_transient_accept(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::OutOfMemory
    ) || error.raw_os_error() == Some(libc_emfile())
        || error.raw_os_error() == Some(libc_enfile())
}

/// `EMFILE`: this process is out of file descriptors.
const fn libc_emfile() -> i32 {
    24
}

/// `ENFILE`: the system is out of file descriptors.
const fn libc_enfile() -> i32 {
    23
}

/// Whether a read failure is the read timeout expiring rather than a broken connection.
fn is_retryable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

fn write_frame(writer: &mut UnixStream, response: &Response) -> anyhow::Result<()> {
    let mut frame = serde_json::to_vec(response).context("encode a harness response")?;
    frame.push(b'\n');
    writer
        .write_all(&frame)
        .context("write a harness response")?;
    writer.flush().context("flush a harness response")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Command, KeyArgs};

    /// A short, unique path: unix socket paths are capped near 108 bytes.
    fn socket_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-drive-{name}-{}-{:?}.sock",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    fn write_line(stream: &mut UnixStream, line: &str) {
        stream
            .write_all(format!("{line}\n").as_bytes())
            .expect("write a request frame");
    }

    fn read_line(reader: &mut BufReader<UnixStream>) -> String {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read a response frame");
        line.trim_end().to_owned()
    }

    /// A frame with no newline in it must not be read into memory without bound: the socket
    /// thread would allocate until the OS killed Fleet, and the scenario would report "Fleet
    /// exited" rather than the oversized frame that caused it.
    #[test]
    fn an_oversized_frame_is_refused_and_the_connection_stays_framed() {
        let path = socket_path("oversized");
        let (server, channel) = listen(&path).expect("listen");
        let mut client = UnixStream::connect(&path).expect("connect");
        let mut reader = BufReader::new(client.try_clone().expect("split"));

        // Comfortably past MAX_FRAME, with the newline only at the very end.
        let mut runaway = "x".repeat(MAX_FRAME + 4096);
        runaway.push('\n');
        client
            .write_all(runaway.as_bytes())
            .expect("write the runaway frame");
        client.flush().expect("flush");

        let refusal = read_line(&mut reader);
        assert!(
            refusal.contains("request frame exceeded"),
            "the client is told what happened: {refusal}"
        );
        assert!(
            channel.requests.is_empty(),
            "nothing that large reaches the application"
        );

        // The connection is still usable, which is the whole point of draining rather than
        // closing: the next frame is answered normally.
        write_line(&mut client, r#"{"id":9,"cmd":"meta","args":{}}"#);
        let incoming = channel.requests.recv_blocking().expect("receive a request");
        assert_eq!(incoming.request.id, 9);
        incoming
            .reply
            .send_blocking(Response::ok(9, serde_json::json!({})))
            .expect("reply");
        assert_eq!(
            read_line(&mut reader),
            r#"{"id":9,"ok":true,"data":{},"error":null}"#
        );
        drop(channel);
        drop(server);
    }

    #[test]
    fn frames_round_trip_and_a_malformed_one_keeps_the_connection() {
        let path = socket_path("round-trip");
        let (server, channel) = listen(&path).expect("listen");
        let mut client = UnixStream::connect(&path).expect("connect");
        let mut reader = BufReader::new(client.try_clone().expect("split"));

        write_line(&mut client, r#"{"id":7,"cmd":"key","args":{"keys":["?"]}}"#);
        let incoming = channel.requests.recv_blocking().expect("receive a request");
        assert_eq!(
            incoming.request.command().expect("decode the command"),
            Command::Key(KeyArgs {
                keys: vec!["?".to_owned()]
            })
        );
        incoming
            .reply
            .send_blocking(Response::ok(7, serde_json::json!({ "handled": [true] })))
            .expect("reply");
        assert_eq!(
            read_line(&mut reader),
            r#"{"id":7,"ok":true,"data":{"handled":[true]},"error":null}"#
        );

        // A frame that is not a request is refused without closing the connection.
        write_line(&mut client, "not json");
        let refusal = read_line(&mut reader);
        assert!(refusal.starts_with(r#"{"id":0,"ok":false,"data":{},"error":"malformed"#));

        // An unknown command still reaches the application, which is what answers `ok:false`.
        write_line(&mut client, r#"{"id":8,"cmd":"future","args":{}}"#);
        let unknown = channel.requests.recv_blocking().expect("receive a request");
        assert_eq!(unknown.request.cmd, "future");
        assert!(unknown.request.command().is_err());
        unknown
            .reply
            .send_blocking(Response::err(8, "unknown command"))
            .expect("reply");
        assert_eq!(
            read_line(&mut reader),
            r#"{"id":8,"ok":false,"data":{},"error":"unknown command"}"#
        );

        drop(channel);
        drop(server);
        assert!(!path.exists(), "dropping the server unlinks its socket");
    }

    #[test]
    fn dropping_a_request_unanswered_fails_it_for_the_client() {
        let path = socket_path("cancelled");
        let (server, channel) = listen(&path).expect("listen");
        let mut client = UnixStream::connect(&path).expect("connect");
        let mut reader = BufReader::new(client.try_clone().expect("split"));

        write_line(&mut client, r#"{"id":3,"cmd":"meta","args":{}}"#);
        let incoming = channel.requests.recv_blocking().expect("receive a request");
        // Exactly what a closing window does to the command it was part-way through.
        drop(incoming);
        assert_eq!(
            read_line(&mut reader),
            r#"{"id":3,"ok":false,"data":{},"error":"the application closed before the command completed"}"#
        );

        drop(channel);
        drop(server);
    }

    #[test]
    fn pipelined_frames_are_answered_in_order() {
        // A runner reads each response before sending the next, but a hand-driven `socat`
        // session writes the whole script at once; the frames must not be merged or reordered.
        let path = socket_path("pipelined");
        let (server, channel) = listen(&path).expect("listen");
        let mut client = UnixStream::connect(&path).expect("connect");
        let mut reader = BufReader::new(client.try_clone().expect("split"));
        client
            .write_all(
                b"{\"id\":1,\"cmd\":\"meta\",\"args\":{}}\n{\"id\":2,\"cmd\":\"quit\",\"args\":{}}\n",
            )
            .expect("write both frames");

        for id in [1, 2] {
            let incoming = channel.requests.recv_blocking().expect("receive a request");
            assert_eq!(incoming.request.id, id);
            incoming
                .reply
                .send_blocking(Response::ok(id, serde_json::json!({})))
                .expect("reply");
            assert_eq!(
                read_line(&mut reader),
                format!(r#"{{"id":{id},"ok":true,"data":{{}},"error":null}}"#)
            );
        }

        drop(channel);
        drop(server);
    }

    #[test]
    fn a_request_arriving_after_the_driver_is_gone_is_refused() {
        let path = socket_path("no-driver");
        let (server, channel) = listen(&path).expect("listen");
        let mut client = UnixStream::connect(&path).expect("connect");
        let mut reader = BufReader::new(client.try_clone().expect("split"));
        drop(channel);

        write_line(&mut client, r#"{"id":4,"cmd":"meta","args":{}}"#);
        assert_eq!(
            read_line(&mut reader),
            r#"{"id":4,"ok":false,"data":{},"error":"the application is no longer accepting harness commands"}"#
        );

        drop(server);
    }
}
