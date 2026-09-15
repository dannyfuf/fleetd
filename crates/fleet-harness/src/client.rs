//! Socket client for correlated harness frames.
//!
//! One request is written and its correlated response read before the next command goes out,
//! so a scenario line never has to guess whether the previous one landed.

use anyhow::Context as _;
use fleet_drive::protocol::{Command, Request, Response};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
};

/// How long the runner waits for a response beyond the command's own advertised budget.
///
/// Commands that deliberately take time (`wait`, `await`, `hover`, `advance`) add their own
/// milliseconds on top, so a slow-but-honest command is never mistaken for a hung app.
const RESPONSE_GRACE: Duration = Duration::from_secs(30);

/// A connected harness socket.
#[derive(Debug)]
pub struct Client {
    socket: PathBuf,
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
    /// Bytes of a response frame read so far.
    ///
    /// `AsyncBufReadExt::read_line` is not cancel-safe: the bytes it has already moved out of
    /// the `BufReader` live in the `String` it was given, and a `timeout` that drops the future
    /// drops them with it. Owning that buffer across calls is what lets a timed-out read resume
    /// where it stopped instead of leaving the stream half-framed for every later command.
    pending: String,
    /// Set when framing can no longer be trusted, which is a scenario-level failure.
    poisoned: Option<String>,
}

impl Client {
    /// Connects to a Fleet that has already created its `FLEET_HARNESS_SOCK` socket.
    ///
    /// This is a single attempt: the caller owns the Fleet process and polls for its exit
    /// while it waits, so a crash during startup is reported instead of timed out.
    pub async fn connect(path: &Path) -> anyhow::Result<Self> {
        let (reader, writer) = UnixStream::connect(path)
            .await
            .with_context(|| format!("connect to the Fleet harness socket at {}", path.display()))?
            .into_split();
        Ok(Self {
            socket: path.to_owned(),
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            pending: String::new(),
            poisoned: None,
        })
    }

    /// Sends one command and returns the response the app correlated to it.
    ///
    /// A failing command is a successful send: `ok: false` is a value, not an error. Only a
    /// transport, timeout or correlation problem is an `Err`.
    pub async fn send(&mut self, command: Command) -> anyhow::Result<Response> {
        if let Some(reason) = &self.poisoned {
            anyhow::bail!("the harness connection is no longer usable: {reason}");
        }
        let request = Request::new(self.next_id, &command)?;
        self.next_id += 1;
        let mut frame = serde_json::to_vec(&request).context("encode harness request")?;
        frame.push(b'\n');
        self.writer
            .write_all(&frame)
            .await
            .with_context(|| self.wrote(&request))?;
        self.writer
            .flush()
            .await
            .with_context(|| self.wrote(&request))?;

        let budget = budget(&command);
        let read = match tokio::time::timeout(budget, self.reader.read_line(&mut self.pending))
            .await
        {
            Ok(read) => read.with_context(|| format!("read the response to {:?}", request.cmd))?,
            Err(_elapsed) => {
                // The partial frame is still in `self.pending`, so nothing was lost — but the
                // response this command is owed may still arrive, and every later command would
                // then read an answer correlated to an older id. Saying so once is better than
                // failing every remaining line of the scenario for the wrong reason.
                let reason = format!(
                    "Fleet did not answer {:?} within {budget:?} on {}",
                    request.cmd,
                    self.socket.display()
                );
                self.poisoned = Some(reason.clone());
                anyhow::bail!(reason);
            }
        };
        let line = std::mem::take(&mut self.pending);
        if read == 0 {
            // Quitting closes the socket as the window goes away, which is the orderly end of
            // a scenario rather than a transport failure.
            if matches!(command, Command::Quit(_)) {
                return Ok(Response::ok(
                    request.id,
                    serde_json::json!({ "closed": true }),
                ));
            }
            anyhow::bail!(
                "Fleet closed the harness socket while answering {:?}; see app.log",
                request.cmd
            );
        }
        let response: Response = serde_json::from_str(line.trim_end())
            .with_context(|| format!("decode the response to {:?} from {line:?}", request.cmd))?;
        anyhow::ensure!(
            response.id == request.id,
            "Fleet answered id {} to request id {} ({:?})",
            response.id,
            request.id,
            request.cmd
        );
        Ok(response)
    }

    fn wrote(&self, request: &Request) -> String {
        format!("send {:?} to {}", request.cmd, self.socket.display())
    }
}

/// How long a command is allowed to take, including the delay it asked for itself.
fn budget(command: &Command) -> Duration {
    let requested = match command {
        Command::Wait(args) => args.millis,
        Command::Await(args) => args.timeout_ms,
        Command::Hover(args) => args.dwell_ms,
        Command::Advance(args) => args.millis,
        _ => 0,
    };
    RESPONSE_GRACE + Duration::from_millis(requested)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_drive::protocol::{EmptyArgs, KeyArgs, WaitArgs};
    use tokio::net::UnixListener;

    /// Answers one connection, replying to each request in turn and closing once the canned
    /// replies run out.
    async fn serve(path: PathBuf, replies: Vec<String>) -> Vec<String> {
        let listener = UnixListener::bind(&path).expect("bind harness socket");
        let (stream, _address) = listener.accept().await.expect("accept");
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut seen = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.expect("read request") == 0 {
                break;
            }
            seen.push(line.trim_end().to_owned());
            let Some(reply) = replies.get(seen.len() - 1) else {
                break;
            };
            writer
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .expect("write response");
            if seen.len() == replies.len() {
                // Nothing left to answer; returning closes the socket instead of blocking
                // on a request that is never going to arrive.
                break;
            }
        }
        seen
    }

    /// Removes a leftover socket, treating "it was not there" as success.
    fn remove(path: &Path) {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove {}: {error}", path.display()),
        }
    }

    fn socket_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fleet-harness-client-{}-{name}.sock",
            std::process::id()
        ));
        remove(&path);
        path
    }

    #[tokio::test]
    async fn commands_are_correlated_and_failures_are_values() {
        let path = socket_path("correlated");
        let server = tokio::spawn(serve(
            path.clone(),
            vec![
                r#"{"id":1,"ok":true,"data":{"handled":[true]},"error":null}"#.to_owned(),
                r#"{"id":2,"ok":false,"data":{},"error":"unknown target"}"#.to_owned(),
            ],
        ));
        // The listener is bound inside the task, so connect until it is there.
        let mut client = loop {
            if let Ok(client) = Client::connect(&path).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        let first = client
            .send(Command::Key(KeyArgs {
                keys: vec!["?".to_owned()],
            }))
            .await
            .expect("send key");
        assert!(first.ok && first.id == 1);
        let second = client
            .send(Command::Wait(WaitArgs { millis: 1 }))
            .await
            .expect("send wait");
        assert!(!second.ok);
        assert_eq!(second.error.as_deref(), Some("unknown target"));

        drop(client);
        let seen = server.await.expect("server task");
        assert_eq!(
            seen,
            vec![
                r#"{"id":1,"cmd":"key","args":{"keys":["?"]}}"#,
                r#"{"id":2,"cmd":"wait","args":{"millis":1}}"#,
            ]
        );
        remove(&path);
    }

    /// A response timeout desynchronises the frame stream: `read_line` is not cancel-safe, and
    /// the answer this command is owed can still arrive while the next command is waiting for
    /// its own. Continuing on that stream fails every later line for a reason that has nothing
    /// to do with what it tested — and takes the failure evidence with it — so the connection
    /// is declared unusable instead.
    #[tokio::test(start_paused = true)]
    async fn a_response_timeout_stops_the_connection_rather_than_desynchronising_it() {
        let path = socket_path("timeout");
        // A server that accepts and then never answers anything.
        let listener = tokio::net::UnixListener::bind(&path).expect("bind harness socket");
        let server = tokio::spawn(async move {
            let (stream, _address) = listener.accept().await.expect("accept");
            // Held so the connection stays open while the client's budget expires.
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
            drop(stream);
        });
        let mut client = loop {
            if let Ok(client) = Client::connect(&path).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        let timed_out = client
            .send(Command::Meta(EmptyArgs {}))
            .await
            .expect_err("a silent server must not answer");
        assert!(
            format!("{timed_out:#}").contains("did not answer"),
            "the first failure names the command that timed out: {timed_out:#}"
        );

        let after = client
            .send(Command::Meta(EmptyArgs {}))
            .await
            .expect_err("the connection is no longer framed");
        assert!(
            format!("{after:#}").contains("no longer usable"),
            "later commands say why rather than reporting a decode failure: {after:#}"
        );

        server.abort();
        remove(&path);
    }

    #[tokio::test]
    async fn a_closed_socket_ends_quit_but_fails_every_other_command() {
        for (command, quits) in [
            (Command::Quit(EmptyArgs {}), true),
            (Command::Meta(EmptyArgs {}), false),
        ] {
            let path = socket_path(if quits { "quit" } else { "meta" });
            let server = tokio::spawn(serve(path.clone(), Vec::new()));
            let mut client = loop {
                if let Ok(client) = Client::connect(&path).await {
                    break client;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            let result = client.send(command).await;
            assert_eq!(result.is_ok(), quits, "closed socket handling");
            let _seen = server.await.expect("server task");
            remove(&path);
        }
    }

    #[test]
    fn a_command_budget_covers_the_delay_it_asked_for() {
        assert_eq!(budget(&Command::Meta(EmptyArgs {})), RESPONSE_GRACE);
        assert_eq!(
            budget(&Command::Wait(WaitArgs { millis: 5_000 })),
            RESPONSE_GRACE + Duration::from_secs(5)
        );
    }
}
