use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, atomic::AtomicUsize, mpsc},
    task::{Context, Poll, Waker},
    thread,
    time::{Duration, Instant},
};

use fleet_proto::terminal::{CursorState, FrameUpdate, TerminalModes, ViewportInfo};

use super::*;

fn detached_host(commands: mpsc::Sender<OwnerEvent>, join: thread::JoinHandle<()>) -> TerminalHost {
    let (_event_sender, events) = async_channel::unbounded();
    let now = Instant::now();
    TerminalHost {
        child_pid: None,
        commands,
        command_bytes: Arc::new(AtomicUsize::new(0)),
        events,
        activity: Arc::new(Mutex::new(TerminalActivity {
            last_output_at: now,
            last_input_at: now,
            output_bytes_total: 0,
        })),
        join: Some(join),
    }
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.poll(&mut context)
}

fn poll_until_ready<F: Future>(future: Pin<&mut F>) -> F::Output {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut future = future;
    loop {
        if let Poll::Ready(output) = poll_once(future.as_mut()) {
            return output;
        }
        assert!(Instant::now() < deadline, "attachment future did not wake");
        thread::yield_now();
    }
}

fn frame(terminal: TerminalId, cols: u16, rows: u16) -> FrameUpdate {
    FrameUpdate {
        terminal,
        seq: 1,
        cols,
        rows,
        full: true,
        shift: None,
        rows_changed: Vec::new(),
        cursor: CursorState {
            row: 0,
            col: 0,
            visible: true,
            shape: Default::default(),
        },
        viewport: ViewportInfo {
            scrollback_len: 0,
            offset: 0,
            history_epoch: 0,
        },
        modes: TerminalModes::default(),
        title: None,
    }
}

#[test]
fn attach_rejects_elapsed_deadline_before_sending() {
    let (commands, inbox) = mpsc::channel();
    let join = thread::spawn(move || drop(inbox));
    let host = detached_host(commands, join);
    let elapsed = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    {
        let mut attach = std::pin::pin!(host.attach(80, 24, elapsed));
        let Poll::Ready(result) = poll_once(attach.as_mut()) else {
            panic!("an elapsed deadline waited for the owner")
        };
        assert!(
            matches!(result, Err(HostError::AttachFailed(message)) if message == "attachment deadline elapsed")
        );
    };

    host.join().unwrap();
}

#[test]
fn attach_reports_closed_response_channel() {
    let (commands, inbox) = mpsc::channel();
    let join = thread::spawn(move || {
        let OwnerEvent::BudgetedCommand {
            command: HostCommand::Attach { reply, .. },
            ..
        } = inbox.recv().unwrap()
        else {
            panic!("owner received an unexpected command")
        };
        drop(reply);
    });
    let host = detached_host(commands, join);
    {
        let mut attach =
            std::pin::pin!(host.attach(80, 24, Instant::now() + Duration::from_secs(1)));
        let result = poll_until_ready(attach.as_mut());
        assert!(
            matches!(result, Err(HostError::AttachFailed(message)) if message == "response channel closed")
        );
    }

    host.join().unwrap();
}

#[test]
fn attach_rejects_frame_returned_after_deadline() {
    let (commands, inbox) = mpsc::channel();
    let join = thread::spawn(move || {
        let OwnerEvent::BudgetedCommand {
            command:
                HostCommand::Attach {
                    cols,
                    rows,
                    deadline,
                    reply,
                },
            ..
        } = inbox.recv().unwrap()
        else {
            panic!("owner received an unexpected command")
        };
        thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
        while Instant::now() <= deadline {
            thread::yield_now();
        }
        reply
            .send_blocking(Ok(frame(TerminalId(11), cols, rows)))
            .unwrap();
    });
    let host = detached_host(commands, join);
    let deadline = Instant::now() + Duration::from_millis(20);
    {
        let mut attach = std::pin::pin!(host.attach(80, 24, deadline));
        let result = poll_until_ready(attach.as_mut());
        assert!(
            matches!(result, Err(HostError::AttachFailed(message)) if message == "attachment deadline elapsed")
        );
    }

    host.join().unwrap();
}

#[test]
fn kill_is_delivered_even_when_the_command_queue_is_full() {
    let (sender, receiver) = mpsc::channel();
    let host = detached_host(sender, thread::spawn(|| {}));
    host.command_bytes
        .store(COMMAND_QUEUE_BYTES, std::sync::atomic::Ordering::Release);

    assert!(matches!(
        host.paste("blocked"),
        Err(HostError::QueueFull { .. })
    ));
    host.kill()
        .unwrap_or_else(|error| panic!("kill must not be refused by backpressure: {error}"));
    assert!(matches!(
        receiver.recv().expect("kill command"),
        OwnerEvent::BudgetedCommand {
            command: HostCommand::Kill,
            ..
        }
    ));
}
