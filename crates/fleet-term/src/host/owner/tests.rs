use std::{path::PathBuf, time::SystemTime};

use super::commands::{PendingViewport, flush_viewport};
use super::*;
use crate::host::{
    COMMAND_OVERHEAD, HostError, TerminalHost, TerminalHostOptions, command_bytes, send_command,
};
use crate::pty::{PTY_WRITE_QUEUE_BYTES, PtyOptions};
use fleet_proto::terminal::{KeyEvent, WheelEvent};

fn test_activity() -> Arc<Mutex<TerminalActivity>> {
    let now = Instant::now();
    Arc::new(Mutex::new(TerminalActivity {
        last_output_at: now,
        last_input_at: now,
        output_bytes_total: 0,
    }))
}

use async_channel::TryRecvError;

#[test]
fn queue_budget_preserves_input_order() {
    let (sender, receiver) = mpsc::channel();
    let queued_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let command_size = COMMAND_OVERHEAD + 3;
    let limit = command_size * 2;

    send_command(
        &sender,
        &queued_bytes,
        HostCommand::Write(b"one".to_vec()),
        limit,
    )
    .unwrap();
    send_command(
        &sender,
        &queued_bytes,
        HostCommand::Write(b"two".to_vec()),
        limit,
    )
    .unwrap();
    assert!(matches!(
        send_command(
            &sender,
            &queued_bytes,
            HostCommand::Write(b"three".to_vec()),
            limit,
        ),
        Err(HostError::QueueFull { .. })
    ));

    let queued = [receiver.recv().unwrap(), receiver.recv().unwrap()].map(|event| match event {
        OwnerEvent::BudgetedCommand {
            command: HostCommand::Write(bytes),
            ..
        } => bytes,
        _ => panic!("unexpected queued owner event"),
    });
    assert_eq!(queued, [b"one".to_vec(), b"two".to_vec()]);
}

#[test]
fn scroll_or_key_reserves_encoded_input_bytes() {
    use fleet_proto::terminal::{Key, KeyAction, Modifiers};

    let command = HostCommand::ScrollOrKey {
        scroll: ScrollCommand::Pages(-1),
        key: KeyEvent {
            key: Key::Char('x'),
            mods: Modifiers::empty(),
            text: Some("input".to_owned()),
            action: KeyAction::Press,
        },
    };

    assert_eq!(command_bytes(&command), COMMAND_OVERHEAD + 5);
}

#[test]
fn writer_saturation_is_rejected_at_host_boundary() {
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal: TerminalId(30),
        pty: PtyOptions::command(
            "/bin/sh",
            ["-c", "stty raw -echo; printf READY; trap '' HUP; sleep 60"],
            PathBuf::from("/tmp"),
            80,
            24,
        ),
        scrollback_bytes: 1024,
        initial_command: None,
        starting_sequence: 1,
    })
    .unwrap();
    let events = host.event_receiver();
    await_event(&events, |event| {
        matches!(event, HostEvent::Frame(frame) if frame.rows_changed.iter().any(|row| {
            row.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains("READY")
        }))
    });
    host.write(vec![b'x'; PTY_WRITE_QUEUE_BYTES * 3 / 4])
        .unwrap();

    let (reply, attached) = async_channel::bounded(1);
    host.commands
        .send(OwnerEvent::Command(HostCommand::Attach {
            cols: 81,
            rows: 24,
            deadline: Instant::now() + Duration::from_secs(5),
            reply,
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match attached.try_recv() {
            Ok(frame) => {
                assert_eq!(frame.unwrap().cols, 81);
                break;
            }
            Err(TryRecvError::Empty) if Instant::now() < deadline => thread::yield_now(),
            other => {
                let _ = std::process::Command::new("/bin/kill")
                    .args(["-KILL", &host.child_pid().unwrap().to_string()])
                    .status();
                panic!("owner did not process the first write: {other:?}");
            }
        }
    }

    assert!(matches!(
        host.write(vec![b'y'; PTY_WRITE_QUEUE_BYTES / 2]),
        Err(HostError::QueueFull { .. })
    ));
    host.kill().unwrap();
    host.join().unwrap();
}

#[test]
fn bounded_event_queue_recovers_with_contiguous_full_frame() {
    let (mut owner, _sender, events) = owner_fixture_with_event_capacity("exec /bin/cat", 2);
    owner.sequence = 40;
    owner.engine.feed(b"first");
    owner.dirty = true;
    owner.last_frame_at = Instant::now()
        .checked_sub(FRAME_INTERVAL)
        .unwrap_or_else(Instant::now);
    assert!(owner.deliver_frame_if_ready());
    owner
        .try_send_event(HostEvent::Title("occupied".to_owned()))
        .unwrap();

    owner.engine.feed(b" second");
    owner.dirty = true;
    owner.last_frame_at = Instant::now()
        .checked_sub(FRAME_INTERVAL)
        .unwrap_or_else(Instant::now);
    assert!(owner.deliver_frame_if_ready());
    assert!(owner.dirty && owner.force_full);

    let HostEvent::Frame(first) = events.try_recv().unwrap() else {
        panic!("first queued event was not a frame")
    };
    assert_eq!(
        events.try_recv().unwrap(),
        HostEvent::Title("occupied".to_owned())
    );
    assert!(owner.deliver_frame_if_ready());
    let HostEvent::Frame(recovery) = events.try_recv().unwrap() else {
        panic!("recovery event was not a frame")
    };
    assert!(recovery.full);
    assert_eq!(recovery.seq, first.seq + 1);
    owner.pty.kill().unwrap();
}

#[test]
fn pending_side_effects_coalesce_by_kind() {
    let (mut owner, _sender, _events) = owner_fixture_with_event_capacity("exec /bin/cat", 2);
    owner.queue_pending_event(HostEvent::Title("old title".to_owned()));
    owner.queue_pending_event(HostEvent::Cwd("/old".to_owned()));
    owner.queue_pending_event(HostEvent::Bell);
    owner.queue_pending_event(HostEvent::Title("new title".to_owned()));
    owner.queue_pending_event(HostEvent::Cwd("/new".to_owned()));
    owner.queue_pending_event(HostEvent::Bell);

    assert_eq!(
        owner.pending_events,
        VecDeque::from([
            HostEvent::Title("new title".to_owned()),
            HostEvent::Cwd("/new".to_owned()),
            HostEvent::Bell,
        ])
    );
    owner.pty.kill().unwrap();
}

#[test]
fn blocked_write_does_not_block_owner() {
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal: TerminalId(29),
        pty: PtyOptions::command(
            "/bin/sh",
            ["-c", "trap '' HUP; sleep 60"],
            PathBuf::from("/tmp"),
            80,
            24,
        ),
        scrollback_bytes: 1024,
        initial_command: None,
        starting_sequence: 1,
    })
    .unwrap();
    host.write(vec![b'x'; 512 * 1024]).unwrap();
    let (reply, attached) = async_channel::bounded(1);
    host.commands
        .send(OwnerEvent::Command(HostCommand::Attach {
            cols: 81,
            rows: 24,
            deadline: Instant::now() + Duration::from_secs(5),
            reply,
        }))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let frame = loop {
        match attached.try_recv() {
            Ok(frame) => break frame.unwrap(),
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                thread::yield_now();
            }
            other => {
                let _ = std::process::Command::new("/bin/kill")
                    .args(["-KILL", &host.child_pid().unwrap().to_string()])
                    .status();
                panic!("owner did not process attach while PTY write was blocked: {other:?}");
            }
        }
    };
    assert_eq!(frame.cols, 81);
    host.kill().unwrap();
    host.join().unwrap();
}

#[test]
fn snapshot_failure_forces_recovery() {
    let (mut owner, _sender, events) = owner_fixture("exec /bin/cat");
    owner.sequence = 7;
    owner.engine.feed(b"recover me");
    owner.engine.fail_next_snapshot();
    owner.dirty = true;
    owner.last_frame_at = Instant::now()
        .checked_sub(FRAME_INTERVAL)
        .unwrap_or_else(Instant::now);

    assert!(owner.deliver_frame_if_ready());
    assert!(owner.dirty && owner.force_full);
    assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));

    owner.last_frame_at = Instant::now()
        .checked_sub(FRAME_INTERVAL)
        .unwrap_or_else(Instant::now);
    assert!(owner.deliver_frame_if_ready());
    let HostEvent::Frame(frame) = events.try_recv().unwrap() else {
        panic!("recovery did not emit a frame")
    };
    assert!(frame.full);
    assert_eq!(frame.seq, 7);
    assert!(
        frame.rows_changed[0]
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("recover")
    );
    assert!(!owner.dirty && !owner.force_full);
    owner.pty.kill().unwrap();
}

#[test]
fn large_paste_into_echoing_cat_does_not_block_frames_or_commands() {
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal: TerminalId(21),
        pty: PtyOptions::command(
            "/bin/cat",
            std::iter::empty::<&str>(),
            PathBuf::from("/tmp"),
            80,
            24,
        ),
        scrollback_bytes: 1024 * 1024,
        initial_command: None,
        starting_sequence: 1,
    })
    .unwrap();
    let events = host.event_receiver();
    // 2 MiB with short lines stays below canonical line limits and exercises echo backpressure.
    host.paste(format!("{}\n", "x".repeat(63)).repeat(32 * 1024))
        .unwrap();
    let (reply, attached) = async_channel::bounded(1);
    // Queue behind the paste without blocking, unlike the synchronous `attach` helper.
    host.commands
        .send(OwnerEvent::Command(HostCommand::Attach {
            cols: 81,
            rows: 24,
            deadline: Instant::now() + Duration::from_secs(15),
            reply,
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut processed_command = false;
    let mut produced_output = false;
    while Instant::now() < deadline && !(processed_command && produced_output) {
        if let Ok(frame) = attached.try_recv() {
            let frame = frame.unwrap();
            assert!(frame.full);
            assert_eq!(frame.cols, 81);
            processed_command = true;
        }
        if let Ok(HostEvent::Frame(frame)) = events.try_recv() {
            produced_output |= frame.viewport.scrollback_len > 0;
        }
        thread::sleep(Duration::from_millis(1));
    }
    // A regression blocks the owner in write_all, so kill the child independently on failure.
    if !(processed_command && produced_output) {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-KILL", &host.child_pid().unwrap().to_string()])
            .status();
    }
    host.kill().unwrap();
    host.join().unwrap();
    assert!(
        processed_command,
        "large paste blocked the subsequent attach command"
    );
    assert!(produced_output, "large paste blocked output frames");
}

#[test]
fn scroll_or_key_uses_live_screen_modes() {
    use fleet_proto::terminal::{Key, KeyAction, Modifiers};
    let pty = Pty::spawn(PtyOptions::command(
        "/bin/sh",
        ["-c", "stty raw -echo; printf READY; exec /bin/cat"],
        PathBuf::from("/tmp"),
        40,
        10,
    ))
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while Instant::now() < deadline && output != b"READY" {
        if let Some(bytes) = pty.try_read().unwrap() {
            output.extend(bytes);
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(output, b"READY");
    let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
    engine.feed("line\r\n".repeat(100).as_bytes());
    engine.take_frame(true);
    let (sender, receiver) = mpsc::channel();
    let (events, _) = async_channel::unbounded();
    let wakeup = PtyWakeup::new(sender.clone());
    let mut owner = TerminalOwner::new(
        TerminalId(22),
        pty,
        engine,
        receiver,
        events,
        test_activity(),
        wakeup,
    );
    for alternate in [false, true, false] {
        owner.engine.feed(if alternate {
            b"\x1b[?1049h"
        } else {
            b"\x1b[?1049l"
        });
        let before = owner.engine.viewport_offset();
        sender
            .send(OwnerEvent::Command(HostCommand::ScrollOrKey {
                scroll: ScrollCommand::Pages(-1),
                key: KeyEvent {
                    key: Key::PageUp,
                    mods: Modifiers::SHIFT,
                    text: None,
                    action: KeyAction::Press,
                },
            }))
            .unwrap();
        owner.drain_commands(None);
        if alternate {
            assert_eq!(owner.engine.viewport_offset(), 0);
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut output = Vec::new();
            while Instant::now() < deadline && output.len() < 6 {
                if let Some(bytes) = owner.pty.try_read().unwrap() {
                    output.extend(bytes);
                }
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(output, b"\x1b[5;2~");
        } else {
            assert_eq!(owner.engine.viewport_offset(), before + 10);
            assert!(owner.viewport_moved);
            assert!(owner.pty.try_read().unwrap().is_none());
        }
    }
    owner.pty.kill().unwrap();
}

#[test]
fn coalesced_scrolls_match_sequential_clamping() {
    let mut batched = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
    let mut sequential = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
    for engine in [&mut batched, &mut sequential] {
        engine.feed("line\r\n".repeat(100).as_bytes());
    }
    let commands = [
        ScrollCommand::Lines(10),
        ScrollCommand::Lines(-3),
        ScrollCommand::Pages(-2),
        ScrollCommand::Top,
        ScrollCommand::Lines(-99),
        ScrollCommand::Lines(5),
        ScrollCommand::Bottom,
        ScrollCommand::ToOffset(500),
        ScrollCommand::Pages(1),
    ];
    let mut pending = PendingViewport::new(&batched);
    for command in commands {
        pending.push(command);
        sequential.scroll(command);
    }
    let mut moved = false;
    flush_viewport(&mut Some(pending), &mut batched, &mut moved);
    assert!(moved);
    assert_eq!(batched.viewport_offset(), sequential.viewport_offset());
    assert_eq!(
        batched.take_frame(true).rows_changed,
        sequential.take_frame(true).rows_changed
    );
}

#[test]
fn queued_wheels_coalesce_and_input_returns_to_bottom() {
    use fleet_proto::terminal::{Key, KeyAction, Modifiers};
    let pty = Pty::spawn(PtyOptions::command(
        "/bin/cat",
        std::iter::empty::<&str>(),
        PathBuf::from("/tmp"),
        40,
        10,
    ))
    .unwrap();
    let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
    engine.feed("line\r\n".repeat(100).as_bytes());
    engine.take_frame(true);
    let (sender, receiver) = mpsc::channel();
    let (events, _) = async_channel::unbounded();
    let wakeup = PtyWakeup::new(sender.clone());
    let mut owner = TerminalOwner::new(
        TerminalId(1),
        pty,
        engine,
        receiver,
        events,
        test_activity(),
        wakeup,
    );
    for _ in 0..3 {
        sender
            .send(OwnerEvent::Command(HostCommand::Wheel(WheelEvent {
                steps: -1,
                col: 0,
                row: 0,
                mods: Modifiers::empty(),
            })))
            .unwrap();
    }
    owner.drain_commands(None);
    assert_eq!(owner.engine.viewport_offset(), 3);
    assert!(owner.viewport_moved && !owner.force_full);
    let frame = owner.engine.take_frame(false);
    assert_eq!(frame.shift, Some(-3));
    assert_eq!(frame.rows_changed.len(), 3);
    for input in [
        HostCommand::Key(KeyEvent {
            key: Key::Char('x'),
            mods: Modifiers::empty(),
            text: Some("x".into()),
            action: KeyAction::Press,
        }),
        HostCommand::Paste("paste".into()),
        HostCommand::Write(b"raw".to_vec()),
    ] {
        owner.engine.scroll(ScrollCommand::Top);
        sender.send(OwnerEvent::Command(input)).unwrap();
        owner.drain_commands(None);
        assert_eq!(owner.engine.viewport_offset(), 0);
    }
    owner.pty.kill().unwrap();
}

#[test]
fn continuous_output_does_not_starve_viewport_commands() {
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal: TerminalId(20),
        pty: PtyOptions::command("/usr/bin/yes", ["output"], PathBuf::from("/tmp"), 80, 24),
        scrollback_bytes: 1024 * 1024,
        initial_command: None,
        starting_sequence: 1,
    })
    .unwrap();
    let events = host.event_receiver();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while Instant::now() < deadline {
        if let Ok(HostEvent::Frame(frame)) = events.try_recv()
            && frame.viewport.scrollback_len > 100
        {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(ready, "continuous-output child produced no history");
    host.scroll(ScrollCommand::Lines(-20)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut moved = false;
    while Instant::now() < deadline {
        if let Ok(HostEvent::Frame(frame)) = events.try_recv()
            && frame.viewport.offset > 0
        {
            moved = true;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    host.kill().unwrap();
    host.join().unwrap();
    assert!(moved, "continuous PTY output starved the scroll frame");
}

#[test]
fn host_emits_command_output_before_exit() {
    let options = TerminalHostOptions {
        terminal: TerminalId(17),
        pty: PtyOptions::command(
            "/bin/sh",
            ["-c", "printf OK"],
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            20,
            4,
        ),
        scrollback_bytes: 100,
        initial_command: None,
        starting_sequence: 1,
    };
    let host = TerminalHost::spawn(options)
        .unwrap_or_else(|error| panic!("failed to spawn terminal host: {error}"));
    let receiver = host.event_receiver();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_ok = false;
    let mut saw_exit = false;
    while Instant::now() < deadline && !saw_exit {
        match receiver.try_recv() {
            Ok(HostEvent::Frame(frame)) => {
                saw_ok |= frame.rows_changed.iter().any(|row| {
                    row.cells
                        .iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .contains("OK")
                });
            }
            Ok(HostEvent::Exited(code)) => {
                assert_eq!(code, Some(0));
                saw_exit = true;
            }
            Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(TryRecvError::Closed) => break,
        }
    }
    assert!(saw_ok, "no frame contained command output");
    assert!(saw_exit, "host did not report child exit");
    host.join()
        .unwrap_or_else(|_| panic!("terminal host thread panicked"));
}

#[test]
fn terminal_query_reply_reaches_the_pty_child() {
    let options = TerminalHostOptions {
        terminal: TerminalId(18),
        pty: PtyOptions::command(
            "/bin/sh",
            [
                "-c",
                r#"stty raw -echo; printf '\033[6n'; dd bs=1 count=6 >/dev/null 2>&1; stty sane; printf 'GOT\n'"#,
            ],
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            20,
            4,
        ),
        scrollback_bytes: 100,
        initial_command: None,
        starting_sequence: 1,
    };
    let host = TerminalHost::spawn(options)
        .unwrap_or_else(|error| panic!("failed to spawn terminal host: {error}"));
    let receiver = host.event_receiver();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    let mut saw_exit = false;
    while Instant::now() < deadline && !saw_exit {
        match receiver.try_recv() {
            Ok(HostEvent::Frame(frame)) => {
                for row in frame.rows_changed {
                    output.extend(row.cells.iter().map(|cell| cell.text.as_str()));
                    output.push('\n');
                }
            }
            Ok(HostEvent::Exited(code)) => {
                assert_eq!(code, Some(0));
                saw_exit = true;
            }
            Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(TryRecvError::Closed) => break,
        }
    }
    assert!(
        output.contains("GOT"),
        "the child did not receive the cursor-position response: {output:?}"
    );
    assert!(saw_exit, "querying child did not exit");
    host.join()
        .unwrap_or_else(|_| panic!("terminal host thread panicked"));
}

#[test]
#[ignore = "requires nvim on PATH; run explicitly with --ignored"]
fn nvim_can_enter_insert_escape_and_quit() {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("fleet-nvim-{nonce}"));
    std::fs::create_dir(&directory).unwrap_or_else(|error| {
        panic!(
            "failed to create nvim test directory {}: {error}",
            directory.display()
        )
    });
    let path = directory.join("buffer.txt");
    let options = TerminalHostOptions {
        terminal: TerminalId(19),
        pty: PtyOptions::command(
            "nvim",
            [
                "--clean".into(),
                "-n".into(),
                "-u".into(),
                "NONE".into(),
                "--cmd".into(),
                "set noswapfile".into(),
                path.as_os_str().to_owned(),
            ],
            directory.clone(),
            80,
            24,
        ),
        scrollback_bytes: 100,
        initial_command: None,
        starting_sequence: 1,
    };
    let host = TerminalHost::spawn(options)
        .unwrap_or_else(|error| panic!("failed to spawn nvim terminal: {error}"));
    let receiver = host.event_receiver();
    let ready_by = Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while Instant::now() < ready_by && !ready {
        match receiver.try_recv() {
            Ok(HostEvent::Frame(frame)) => ready = frame.modes.alt_screen,
            Ok(HostEvent::Exited(code)) => panic!("nvim exited before input: {code:?}"),
            Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(TryRecvError::Closed) => break,
        }
    }
    assert!(ready, "nvim never entered its alternate screen");

    let send = |key, mods, text: Option<&str>| {
        host.key(KeyEvent {
            key,
            mods,
            text: text.map(str::to_owned),
            action: fleet_proto::terminal::KeyAction::Press,
        })
        .unwrap_or_else(|error| panic!("failed to send nvim key: {error}"));
    };
    let plain = fleet_proto::terminal::Modifiers::empty();
    for character in ['i', 'f', 'l', 'e', 'e', 't'] {
        send(
            fleet_proto::terminal::Key::Char(character),
            plain,
            Some(&character.to_string()),
        );
    }
    send(fleet_proto::terminal::Key::Escape, plain, None);
    send(
        fleet_proto::terminal::Key::Char('a'),
        fleet_proto::terminal::Modifiers::SHIFT,
        Some("A"),
    );
    send(
        fleet_proto::terminal::Key::Char('['),
        fleet_proto::terminal::Modifiers::CTRL,
        None,
    );
    send(
        fleet_proto::terminal::Key::Char(';'),
        fleet_proto::terminal::Modifiers::SHIFT,
        Some(":"),
    );
    for character in ['w', 'q'] {
        send(
            fleet_proto::terminal::Key::Char(character),
            plain,
            Some(&character.to_string()),
        );
    }
    send(fleet_proto::terminal::Key::Enter, plain, None);

    let exit_by = Instant::now() + Duration::from_secs(5);
    let mut exit = None;
    while Instant::now() < exit_by && exit.is_none() {
        match receiver.try_recv() {
            Ok(HostEvent::Exited(code)) => exit = Some(code),
            Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(TryRecvError::Closed) => break,
        }
    }
    if exit.is_none() {
        let _ignored = host.kill();
    }
    assert_eq!(
        exit,
        Some(Some(0)),
        "nvim did not accept Esc, Ctrl-[, then :wq"
    );
    assert_eq!(
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("nvim did not write {}: {error}", path.display())),
        "fleet\n"
    );
    std::fs::remove_file(&path)
        .unwrap_or_else(|error| panic!("failed to remove {}: {error}", path.display()));
    let _ignored = std::fs::remove_file(directory.join(".nvimlog"));
    std::fs::remove_dir(&directory).unwrap_or_else(|error| {
        panic!(
            "failed to remove nvim test directory {}: {error}",
            directory.display()
        )
    });
    host.join()
        .unwrap_or_else(|_| panic!("terminal host thread panicked"));
}

fn owner_fixture(
    script: &str,
) -> (
    TerminalOwner,
    mpsc::Sender<OwnerEvent>,
    async_channel::Receiver<HostEvent>,
) {
    let (sender, inbox) = mpsc::channel();
    let wakeup = PtyWakeup::new(sender.clone());
    let notify = Arc::clone(&wakeup);
    let pty = Pty::spawn_notifying(
        PtyOptions::command("/bin/sh", ["-c", script], "/tmp", 40, 10),
        Arc::new(move || notify.notify()),
    )
    .unwrap();
    let (events, receiver) = async_channel::unbounded();
    let owner = TerminalOwner::new(
        TerminalId(25),
        pty,
        GhosttyEngine::new(40, 10, 1024 * 1024).unwrap(),
        inbox,
        events,
        test_activity(),
        wakeup,
    );
    (owner, sender, receiver)
}

fn owner_fixture_with_event_capacity(
    script: &str,
    event_capacity: usize,
) -> (
    TerminalOwner,
    mpsc::Sender<OwnerEvent>,
    async_channel::Receiver<HostEvent>,
) {
    let (sender, inbox) = mpsc::channel();
    let wakeup = PtyWakeup::new(sender.clone());
    let notify = Arc::clone(&wakeup);
    let pty = Pty::spawn_notifying(
        PtyOptions::command("/bin/sh", ["-c", script], "/tmp", 40, 10),
        Arc::new(move || notify.notify()),
    )
    .unwrap();
    let (events, receiver) = async_channel::bounded(event_capacity);
    let owner = TerminalOwner::new(
        TerminalId(26),
        pty,
        GhosttyEngine::new(40, 10, 1024 * 1024).unwrap(),
        inbox,
        events,
        test_activity(),
        wakeup,
    );
    (owner, sender, receiver)
}

fn await_event(
    events: &async_channel::Receiver<HostEvent>,
    predicate: impl Fn(&HostEvent) -> bool,
) -> HostEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match events.try_recv() {
            Ok(event) if predicate(&event) => return event,
            Ok(_) | Err(TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1));
            }
            other => panic!("terminal event did not arrive: {other:?}"),
        }
    }
}

#[test]
fn idle_owner_sleeps_until_commands_or_output_arrive() {
    let (owner, sender, events) = owner_fixture("printf READY; exec /bin/cat");
    let iterations = Arc::clone(&owner.iterations);
    let join = thread::spawn(move || owner.run(None, 1));
    await_event(&events, |event| matches!(event, HostEvent::Frame(_)));
    // Allow the single idle-compression deadline to complete before measuring.
    thread::sleep(Duration::from_millis(400));
    let before = iterations.load(Ordering::Relaxed);
    thread::sleep(Duration::from_millis(200));
    let idle_iterations = iterations.load(Ordering::Relaxed) - before;
    sender
        .send(OwnerEvent::Command(HostCommand::Write(b"WAKE\n".to_vec())))
        .unwrap();
    await_event(&events, |event| {
        matches!(event, HostEvent::Frame(frame) if frame.rows_changed.iter().any(|row| {
            row.cells.iter().map(|cell| cell.text.as_str()).collect::<String>().contains("WAKE")
        }))
    });
    sender.send(OwnerEvent::Command(HostCommand::Kill)).unwrap();
    await_event(&events, |event| matches!(event, HostEvent::Exited(_)));
    join.join().unwrap();
    assert_eq!(
        idle_iterations, 0,
        "idle owner woke without an event or deadline"
    );
    println!("idle owner: {idle_iterations} loop wakeups over 200 ms after compression");
}

#[test]
fn child_exit_wakes_owner_even_when_descendant_keeps_output_open() {
    let (owner, _sender, events) = owner_fixture("sleep 1 & exit 7");
    let started = Instant::now();
    let join = thread::spawn(move || owner.run(None, 1));
    let event = await_event(&events, |event| matches!(event, HostEvent::Exited(_)));
    join.join().unwrap();
    assert_eq!(event, HostEvent::Exited(Some(7)));
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "owner slept until inherited PTY closed"
    );
}

#[test]
fn attach_snapshots_once_and_delivers_both_sequences() {
    let (mut owner, sender, events) = owner_fixture("exec /bin/cat");
    owner.sequence = 42;
    owner.engine.feed(b"\x1b]2;title\x07before attach");
    let (reply, response) = async_channel::bounded(1);
    sender
        .send(OwnerEvent::Command(HostCommand::Attach {
            cols: 41,
            rows: 10,
            deadline: Instant::now() + Duration::from_secs(5),
            reply,
        }))
        .unwrap();
    owner.drain_commands(None);
    assert!(owner.deliver_frame_if_ready());
    let mut reply = response.try_recv().unwrap().unwrap();
    let HostEvent::Frame(broadcast) = events.try_recv().unwrap() else {
        panic!("missing broadcast")
    };
    owner.pty.kill().unwrap();
    assert_eq!(owner.frames_taken, 1);
    assert_eq!(reply.seq, 42);
    assert_eq!(broadcast.seq, 43);
    reply.seq = broadcast.seq;
    assert_eq!(reply, broadcast);
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, HostEvent::Frame(_)))
    );
}

#[test]
fn expired_attach_is_discarded_before_resize_or_snapshot() {
    let (mut owner, sender, events) = owner_fixture("exec /bin/cat");
    let (reply, response) = async_channel::bounded(1);
    sender
        .send(OwnerEvent::Command(HostCommand::Attach {
            cols: 80,
            rows: 24,
            deadline: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
            reply,
        }))
        .unwrap();

    owner.drain_commands(None);

    assert_eq!(owner.engine.rows(), 10);
    assert_eq!(owner.frames_taken, 0);
    assert_eq!(
        response.try_recv().unwrap(),
        Err("attachment deadline elapsed".to_owned())
    );
    assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
    owner.pty.kill().unwrap();
}

#[test]
fn prompt_deadline_wakes_without_pty_output() {
    let (owner, _sender, events) =
        owner_fixture("stty raw -echo; IFS= read -r line; printf '%s' \"$line\"");
    let join = thread::spawn(move || owner.run(Some("READY\n".into()), 1));
    let frame = await_event(&events, |event| matches!(event, HostEvent::Frame(_)));
    await_event(&events, |event| matches!(event, HostEvent::Exited(_)));
    join.join().unwrap();
    let HostEvent::Frame(frame) = frame else {
        unreachable!()
    };
    let text: String = frame
        .rows_changed
        .iter()
        .flat_map(|row| row.cells.iter().map(|cell| cell.text.as_str()))
        .collect();
    assert!(text.contains("READY"));
}

#[test]
fn kill_still_terminates_a_child_that_ignores_sighup() {
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal: TerminalId(27),
        pty: PtyOptions::command(
            "/bin/sh",
            ["-c", "trap '' HUP; printf READY; exec /bin/cat"],
            "/tmp",
            40,
            10,
        ),
        scrollback_bytes: 1024,
        initial_command: None,
        starting_sequence: 1,
    })
    .unwrap();
    let events = host.event_receiver();
    await_event(&events, |event| matches!(event, HostEvent::Frame(_)));
    host.kill().unwrap();
    await_event(&events, |event| matches!(event, HostEvent::Exited(_)));
    host.join().unwrap();
}

#[test]
fn flood_frames_reconstruct_the_complete_terminal() {
    let script = "stty -onlcr; i=0; while [ $i -lt 5000 ]; do printf '%04d abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\\r\\n' \"$i\"; i=$((i+1)); done; exec /bin/cat";
    let output = (0..5000)
        .map(|i| {
            format!("{i:04} abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\r\n")
        })
        .collect::<String>();
    assert!(output.len() > OUTPUT_BATCH_BYTES);
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal: TerminalId(28),
        pty: PtyOptions::command("/bin/sh", ["-c", script], "/tmp", 80, 24),
        scrollback_bytes: 16 * 1024 * 1024,
        initial_command: None,
        starting_sequence: 1,
    })
    .unwrap();
    let events = host.event_receiver();
    let mut mirror = vec![Vec::new(); 24];
    let mut wrapped = [false; 24];
    let mut sequence = 0;
    let mut frames = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    let last_cursor = loop {
        match events.try_recv() {
            Ok(HostEvent::Frame(frame)) => {
                assert_eq!(frame.seq, sequence + 1);
                sequence = frame.seq;
                frames += 1;
                assert_eq!(frame.shift, None);
                for row in frame.rows_changed {
                    mirror[usize::from(row.index)] = row.cells;
                    wrapped[usize::from(row.index)] = row.wrapped;
                }
                let last_line: String = mirror[22].iter().map(|cell| cell.text.as_str()).collect();
                if last_line.starts_with("4999 ") {
                    break frame.cursor;
                }
            }
            Ok(_) | Err(TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1));
            }
            other => {
                host.kill().unwrap();
                let last_line: String = mirror[22].iter().map(|cell| cell.text.as_str()).collect();
                panic!(
                    "flood did not complete: {other:?}; bytes={}, frames={frames}, last={last_line:?}",
                    host.activity().output_bytes_total
                );
            }
        }
    };
    let output_bytes = host.activity().output_bytes_total;
    host.kill().unwrap();
    await_event(&events, |event| matches!(event, HostEvent::Exited(_)));
    host.join().unwrap();
    let mut reference = GhosttyEngine::new(80, 24, 16 * 1024 * 1024).unwrap();
    reference.feed(output.as_bytes());
    let reference = reference.take_frame(true);
    for row in reference.rows_changed {
        assert_eq!(mirror[usize::from(row.index)], row.cells);
        assert_eq!(wrapped[usize::from(row.index)], row.wrapped);
    }
    assert_eq!(last_cursor, reference.cursor);
    assert_eq!(output_bytes, output.len() as u64);
    println!("flood: {output_bytes} bytes, {frames} sequenced frames; mirror equals full snapshot");
}

#[test]
fn pty_output_wakes_an_owner_with_no_remaining_deadline() {
    let (owner, sender, events) = owner_fixture("sleep 0.4; printf READY; exec /bin/cat");
    let join = thread::spawn(move || owner.run(None, 1));
    let event = await_event(&events, |event| matches!(event, HostEvent::Frame(_)));
    sender.send(OwnerEvent::Command(HostCommand::Kill)).unwrap();
    await_event(&events, |event| matches!(event, HostEvent::Exited(_)));
    join.join().unwrap();
    let HostEvent::Frame(frame) = event else {
        unreachable!()
    };
    let text: String = frame.rows_changed[0]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert_eq!(text, "READY");
}

#[test]
fn dropping_unstarted_owner_reaps_its_child() {
    let (owner, _sender, events) = owner_fixture("exec /bin/cat");
    let pid = owner.pty.child_pid().unwrap();
    drop(owner);
    assert!(events.is_closed());
    let status = std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success(), "startup rollback left its child alive");
}
