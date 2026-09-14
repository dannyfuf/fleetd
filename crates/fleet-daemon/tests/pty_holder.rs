//! The detached PTY holder, end to end: a real `fleetd pty-hold` process, a daemon that
//! disconnects the way a restart does, and a second daemon that reattaches and gets the replay.

use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use fleet_core::{
    ids::TerminalId,
    paths::{FleetHome, pty_socket_path},
};

mod infra;

/// Ceiling on every wait here; a holder that misses it has a bug, not a slow machine.
const DEADLINE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);

const TAG_INPUT: u8 = 0x01;
const TAG_RESIZE: u8 = 0x02;
const TAG_KILL: u8 = 0x03;
const TAG_DETACH: u8 = 0x04;
const TAG_HELLO: u8 = 0x81;
const TAG_OUTPUT: u8 = 0x82;
const TAG_EXITED: u8 = 0x83;

/// Printed by the shell but never by the terminal's echo of the command that prints it.
const MARKER: &[u8] = b"FLEETOK";
/// Socket nonce for this suite; production picks a fresh one per spawn.
const NONCE: &str = "00112233445566aa";
/// The wire version this test hand-decodes; a bump here means the golden moved too.
const HOLDER_PROTOCOL_VERSION: u8 = fleet_term::HOLDER_PROTOCOL_VERSION;

/// A `fleetd pty-hold` process, killed if the test fails before it stops itself.
struct Holder {
    child: Child,
}

impl Drop for Holder {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            // Fixture teardown, mirroring `tests/infra/mod.rs`: a failing test must not leave a
            // shell behind, and there is nothing useful to do with either error.
            let _ignored = self.child.kill();
            let _ignored = self.child.wait();
        }
    }
}

#[test]
fn a_holder_outlives_its_daemon_and_replays_what_the_shell_already_printed() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = FleetHome::new(temp.path().join("fleet"));
    let terminal = TerminalId(1);
    let socket = pty_socket_path(&home, terminal, NONCE);
    let sidecar = home.pty_sidecar_path(terminal);
    std::fs::create_dir_all(home.pty_dir()).unwrap_or_else(|error| panic!("{error}"));
    // Stands in for the record the daemon writes; the holder must remove it when it exits.
    std::fs::write(&sidecar, b"{}").unwrap_or_else(|error| panic!("{error}"));

    let log = temp.path().join("holder.err");
    let Some(holder) = start_holder(&home, terminal, temp.path(), &log) else {
        eprintln!("skipping: this environment cannot allocate a pseudo-terminal");
        return;
    };
    wait_for(&socket, true);

    // The first daemon: connect, drive the shell, then vanish the way a restart does.
    let mut first = connect(&socket);
    let held = greeting(&mut first);
    assert!(
        held.child_pid.is_some(),
        "a holder greets every connection with the child it holds"
    );
    assert!(
        held.cols > 0 && held.rows > 0,
        "a greeting carries the grid its replay was produced for"
    );
    // Split so the terminal's echo of this line cannot itself contain the marker.
    write_frame(&mut first, TAG_INPUT, b"printf '%s%s\\n' FLEET OK\n");
    assert!(
        read_until(&mut first, MARKER).is_some(),
        "the holder did not relay the shell's output"
    );

    // Resize before disappearing, the way a client with its own window does. The holder applies a
    // connection's frames in order, so waiting for a later frame's echo is what makes the resize
    // observably applied before the disconnect rather than racing it.
    let mut resize = Vec::new();
    resize.extend_from_slice(&101_u16.to_be_bytes());
    resize.extend_from_slice(&31_u16.to_be_bytes());
    write_frame(&mut first, TAG_RESIZE, &resize);
    write_frame(&mut first, TAG_INPUT, b"printf '%s%s\\n' SIZE OK\n");
    assert!(
        read_until(&mut first, b"SIZEOK").is_some(),
        "the holder did not apply the frames sent before the disconnect"
    );
    first
        .shutdown(Shutdown::Both)
        .unwrap_or_else(|error| panic!("{error}"));
    drop(first);

    // The second daemon: the shell is still there, and so is everything it printed. The greeting
    // reports the grid the child is living at *now* — a daemon that built its emulator at the
    // creation size would parse this replay wrongly and never repaint.
    let mut second = connect(&socket);
    let reattached = greeting(&mut second);
    assert_eq!(
        reattached.child_pid, held.child_pid,
        "a reattach must find the same child, not a new one"
    );
    assert_eq!(
        (reattached.cols, reattached.rows),
        (101, 31),
        "a greeting must report the current window size, not the one at creation"
    );
    let replayed = read_until(&mut second, MARKER)
        .unwrap_or_else(|| panic!("a reattaching daemon did not receive the replay buffer"));
    assert!(
        replayed
            .windows(MARKER.len())
            .any(|window| window == MARKER),
        "the replay did not contain output printed before the disconnect"
    );

    // A daemon that only detaches leaves the shell running: the proof is that a third daemon is
    // still greeted by the same child, not that a file happens to exist.
    write_frame(&mut second, TAG_DETACH, &[]);
    drop(second);

    // Killing does end it, and the holder removes everything it owns.
    let mut third = connect(&socket);
    assert_eq!(
        greeting(&mut third),
        reattached,
        "detaching stopped the holder or its child"
    );
    write_frame(&mut third, TAG_KILL, &[]);
    assert!(
        read_until_tag(&mut third, TAG_EXITED),
        "the holder did not report its child's exit"
    );
    drop(third);

    // The holder unlinks its socket first and its sidecar second, so both are waited on.
    wait_for(&socket, false);
    wait_for(&sidecar, false);
    assert!(!socket.exists(), "the holder left its socket behind");
    assert!(!sidecar.exists(), "the holder left its sidecar behind");
    drop(holder);
}

fn start_holder(home: &FleetHome, terminal: TerminalId, cwd: &Path, log: &Path) -> Option<Holder> {
    let errors = std::fs::File::create(log).unwrap_or_else(|error| panic!("{error}"));
    let child = Command::new(env!("CARGO_BIN_EXE_fleetd"))
        .arg("--home")
        .arg(home.root())
        .arg("pty-hold")
        .arg("--terminal")
        .arg(terminal.0.to_string())
        .arg("--session")
        .arg("holder-test")
        .arg("--name")
        .arg("sh")
        .arg("--cwd")
        .arg(cwd)
        .arg("--socket")
        .arg(pty_socket_path(home, terminal, NONCE))
        .arg("--sidecar")
        .arg(home.pty_sidecar_path(terminal))
        .env("SHELL", test_shell())
        // An isolated home keeps this test out of the developer's own shell profile.
        .env("HOME", cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(errors))
        .spawn()
        .unwrap_or_else(|error| panic!("start fleetd pty-hold: {error}"));
    let mut holder = Holder { child };
    // Wait for the outcome rather than for a fixed moment: a machine slower than the sleep would
    // otherwise be reported as a skip, which reads as a pass.
    let socket = pty_socket_path(home, terminal, NONCE);
    let deadline = Instant::now() + DEADLINE;
    loop {
        match holder.child.try_wait() {
            Ok(Some(status)) => {
                let diagnostics = std::fs::read_to_string(log).unwrap_or_default();
                // A sandbox with no free pseudo-terminal is the one environment this test skips
                // in; every other early exit is a real failure and must say so.
                assert!(
                    diagnostics.contains("PTY setup failed"),
                    "pty holder exited with {status}: {diagnostics}"
                );
                return None;
            }
            Ok(None) if socket.exists() => return Some(holder),
            Ok(None) => {}
            Err(error) => panic!("failed to observe the pty holder: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "pty holder never bound {}",
            socket.display()
        );
        std::thread::sleep(POLL);
    }
}

/// A holder's greeting, as the daemon reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Greeting {
    child_pid: Option<u32>,
    cols: u16,
    rows: u16,
}

fn greeting(stream: &mut UnixStream) -> Greeting {
    let (tag, payload) = read_frame(stream).unwrap_or_else(|| panic!("holder sent no greeting"));
    assert_eq!(
        tag, TAG_HELLO,
        "the first frame of a connection is the greeting"
    );
    let [
        version,
        a,
        b,
        c,
        d,
        cols_high,
        cols_low,
        rows_high,
        rows_low,
    ] = payload[..]
    else {
        panic!("a greeting is nine bytes, got {}", payload.len());
    };
    assert_eq!(
        version, HOLDER_PROTOCOL_VERSION,
        "this test and the holder must agree on the wire"
    );
    Greeting {
        child_pid: Some(u32::from_be_bytes([a, b, c, d])).filter(|pid| *pid != 0),
        cols: u16::from_be_bytes([cols_high, cols_low]),
        rows: u16::from_be_bytes([rows_high, rows_low]),
    }
}

/// Returns a shell that accepts `-l`, which is how Fleet starts every terminal.
fn test_shell() -> PathBuf {
    let bash = PathBuf::from("/bin/bash");
    if bash.is_file() {
        bash
    } else {
        PathBuf::from("/bin/sh")
    }
}

fn connect(socket: &Path) -> UnixStream {
    let deadline = Instant::now() + DEADLINE;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(DEADLINE))
                    .unwrap_or_else(|error| panic!("{error}"));
                return stream;
            }
            Err(error) if Instant::now() >= deadline => {
                panic!(
                    "could not reach the holder at {}: {error}",
                    socket.display()
                )
            }
            Err(_) => std::thread::sleep(POLL),
        }
    }
}

fn write_frame(stream: &mut UnixStream, tag: u8, payload: &[u8]) {
    let mut frame = vec![tag];
    frame.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    frame.extend_from_slice(payload);
    stream
        .write_all(&frame)
        .unwrap_or_else(|error| panic!("{error}"));
    stream.flush().unwrap_or_else(|error| panic!("{error}"));
}

fn read_frame(stream: &mut UnixStream) -> Option<(u8, Vec<u8>)> {
    let mut header = [0_u8; 5];
    stream.read_exact(&mut header).ok()?;
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).ok()?;
    Some((header[0], payload))
}

/// Reads output frames until `needle` has been seen, returning everything read.
fn read_until(stream: &mut UnixStream, needle: &[u8]) -> Option<Vec<u8>> {
    let deadline = Instant::now() + DEADLINE;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        let (tag, payload) = read_frame(stream)?;
        if tag == TAG_OUTPUT {
            seen.extend_from_slice(&payload);
            if seen.windows(needle.len()).any(|window| window == needle) {
                return Some(seen);
            }
        }
    }
    None
}

fn read_until_tag(stream: &mut UnixStream, wanted: u8) -> bool {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        match read_frame(stream) {
            Some((tag, _)) if tag == wanted => return true,
            Some(_) => {}
            None => return false,
        }
    }
    false
}

fn wait_for(path: &Path, present: bool) {
    let deadline = Instant::now() + DEADLINE;
    while path.exists() != present && Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
}

/// The user-visible promise: `fleet daemon restart` leaves the agent in every tab running.
#[tokio::test]
async fn a_terminal_and_its_shell_survive_a_daemon_restart_under_the_same_id() {
    use fleet_core::config::Agent;
    use fleet_daemon::{
        adapters::files::{Files, RealFiles},
        stores::config::ConfigStore,
    };
    use fleet_proto::{
        PROTOCOL_VERSION,
        codec::FleetCodec,
        request::{Request, RequestBody},
        response::{Response, ResponseBody},
    };
    use futures_util::{SinkExt, StreamExt};
    use std::sync::Arc;
    use tokio_util::codec::Framed;

    type Client = Framed<tokio::net::UnixStream, FleetCodec<Request, serde_json::Value>>;

    // The shared fixture carries helpers this suite does not need; naming them keeps the
    // module's unused halves out of this binary's dead-code warnings, as its other users do.
    let _ = infra::RemoteDaemon::start;
    let _ = infra::assert_remote_contract;

    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet-home");
    let files: Arc<dyn Files> = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let config = ConfigStore::new(&home, files);
    let mut effective = config
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    // A command that outlives the whole test, so "the shell is still there" is unambiguous.
    effective.agent_commands.claude = "/bin/sleep 300".to_owned();
    config
        .save(effective)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    async fn hello(socket: &Path) -> Client {
        let deadline = Instant::now() + DEADLINE;
        let stream = loop {
            match tokio::net::UnixStream::connect(socket).await {
                Ok(stream) => break stream,
                Err(error) if Instant::now() >= deadline => {
                    panic!("daemon never listened: {error}")
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        };
        let mut client = Framed::new(stream, FleetCodec::<Request, serde_json::Value>::new());
        client
            .send(Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    client: "pty-holder-test".into(),
                },
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let _greeting = answer(&mut client).await;
        client
    }

    async fn ask(client: &mut Client, id: u64, body: RequestBody) -> Response {
        client
            .send(Request { id, body })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        answer(client).await
    }

    async fn answer(client: &mut Client) -> Response {
        let value = client
            .next()
            .await
            .unwrap_or_else(|| panic!("connection closed"))
            .unwrap_or_else(|error| panic!("{error}"));
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    let socket = home.join("fleetd.sock");
    let mut first = infra::DaemonProcess::start(&home);
    let mut client = hello(&socket).await;
    let created = ask(
        &mut client,
        2,
        RequestBody::EnsureSession {
            worktree: None,
            agent: Some(Agent::Claude),
            sleep_previous: false,
        },
    )
    .await;
    let Ok(ResponseBody::Session(created)) = created.result else {
        panic!("expected an agent session");
    };
    let terminal = created.terminals[0].id;
    let shell = created.terminals[0].shell_pid;
    assert!(shell.is_some(), "a PTY terminal reports its login shell");
    let sidecar = FleetHome::new(&home).pty_sidecar_path(terminal);
    assert!(
        sidecar.exists(),
        "a terminal records the holder that owns it"
    );

    // Exactly what `fleet daemon restart` asks for before it falls back to SIGTERM.
    let stopping = ask(
        &mut client,
        3,
        RequestBody::DaemonShutdown {
            stop_sessions: false,
        },
    )
    .await;
    assert_eq!(stopping.result, Ok(ResponseBody::ShuttingDown));
    first.wait().await;
    assert!(
        !socket.exists(),
        "the retiring daemon left its socket behind"
    );
    assert!(
        sidecar.exists(),
        "a restart must not discard the record of a live holder"
    );

    let _second = infra::DaemonProcess::start(&home);
    let mut client = hello(&socket).await;
    let snapshot = ask(&mut client, 4, RequestBody::GetSnapshot).await;
    let Ok(ResponseBody::Snapshot(snapshot)) = snapshot.result else {
        panic!("expected a snapshot");
    };
    let adopted = snapshot
        .sessions
        .iter()
        .find(|session| session.id == created.id)
        .unwrap_or_else(|| panic!("the session did not survive the restart"));
    assert_eq!(
        adopted
            .terminals
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![terminal],
        "the terminal must come back under the id its client tabs already hold"
    );
    assert_eq!(
        adopted.terminals[0].shell_pid, shell,
        "the surviving terminal must be the same shell, not a new one"
    );

    // Killing the session is what actually ends the shell, restart or no restart.
    let killed = ask(
        &mut client,
        5,
        RequestBody::KillSession {
            session: created.id,
        },
    )
    .await;
    assert!(killed.result.is_ok(), "kill the adopted session");
    wait_for(&sidecar, false);
    assert!(
        !sidecar.exists(),
        "a killed session leaves no holder record"
    );
}
