//! Shared scripted-input mechanics. Each caller selects its existing wire dialect.

mod files;
mod protocol;

use files::{Tail, log_path, shot_paths};
use gpui::{
    App, AppContext, AsyncWindowContext, Modifiers, PlatformInput, ScrollDelta, ScrollWheelEvent,
    Task, TouchPhase, Window, point, px,
};
use protocol::{Step, keystroke_for, parse, with_simulated_key_char};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const POLL: Duration = Duration::from_millis(100);
const SETTLE: Duration = Duration::from_millis(250);
// Script wheel units are part of the driver protocol, independent of theme density.
const WHEEL_UNIT: f32 = 18.0;
const SCREENCAPTURE: &str = "/usr/sbin/screencapture";

/// Command and acknowledgement policy retained by each application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Keys, text, waits, all-display screenshots, and quit; no per-line completion.
    Fleet,
    /// Also accepts wheel input and acknowledges every executable line.
    Lazygit,
}

/// Starts a driver cancelled on window close, even during an idle poll or script wait.
/// `timestamp` runs on the background executor and preserves the caller's log prefix.
pub fn spawn(
    script: PathBuf,
    window: &Window,
    cx: &App,
    dialect: Dialect,
    timestamp: fn() -> String,
) -> Task<()> {
    let window_id = window.window_handle().window_id();
    let (closed_tx, closed_rx) = async_channel::bounded(1);
    let subscription = cx.on_window_closed(move |_, closed| {
        if closed == window_id {
            let _ = closed_tx.try_send(());
        }
    });
    window.spawn(cx, async move |cx| {
        let _subscription = subscription;
        tokio::select! {
            _ = closed_rx.recv() => {},
            _ = run(script, dialect, timestamp, cx) => {},
        }
    })
}

struct Log {
    path: PathBuf,
    timestamp: fn() -> String,
}

impl Log {
    async fn write(&self, message: impl Into<String>, cx: &AsyncWindowContext) {
        let path = self.path.clone();
        let timestamp = self.timestamp;
        let message = message.into();
        cx.background_spawn(async move {
            files::log_line(&path, &timestamp(), &message);
        })
        .await;
    }
}

async fn run(
    script: PathBuf,
    dialect: Dialect,
    timestamp: fn() -> String,
    cx: &mut AsyncWindowContext,
) {
    let log = Log {
        path: log_path(&script),
        timestamp,
    };
    tracing::info!(script = %script.display(), "drive: script driver armed");
    log.write(format!("start {}", script.display()), cx).await;
    let mut tail = Tail::new(script);
    loop {
        cx.background_executor().timer(POLL).await;
        let (returned_tail, lines) = cx
            .background_spawn(async move {
                let lines = tail.poll();
                (tail, lines)
            })
            .await;
        tail = returned_tail;
        for line in lines {
            let step = match parse(&line, dialect) {
                Ok(None) => continue,
                Ok(Some(step)) => step,
                Err(error) => {
                    tracing::warn!(%line, %error, "drive: bad script line");
                    log.write(format!("error {line:?}: {error}"), cx).await;
                    continue;
                }
            };
            log.write(format!("run {line}"), cx).await;
            match perform(step, dialect, &log, cx).await {
                Flow::Stop => return,
                Flow::Continue if dialect == Dialect::Lazygit => {
                    log.write(format!("done {line}"), cx).await;
                }
                Flow::Continue | Flow::Failed => {}
            }
        }
    }
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Failed,
    Stop,
}

/// Applies one parsed step. `Stop` means the window is gone or the script asked to quit;
/// the caller must not acknowledge the line afterwards.
async fn perform(step: Step, dialect: Dialect, log: &Log, cx: &mut AsyncWindowContext) -> Flow {
    match step {
        Step::Keys(keys) => {
            for key in keys {
                let key = with_simulated_key_char(key);
                match cx.update(|window, cx| window.dispatch_keystroke(key.clone(), cx)) {
                    Ok(true) => {}
                    Ok(false) => {
                        log.write(format!("unhandled key {}", key.unparse()), cx)
                            .await;
                    }
                    Err(error) => {
                        log.write(format!("window gone: {error}"), cx).await;
                        return Flow::Stop;
                    }
                }
            }
            Flow::Continue
        }
        Step::Type(text) => {
            for character in text.chars() {
                let dispatched =
                    cx.update(|window, cx| window.dispatch_keystroke(keystroke_for(character), cx));
                if dispatched.is_err() {
                    log.write("window gone", cx).await;
                    return Flow::Stop;
                }
            }
            Flow::Continue
        }
        Step::Wheel {
            rows,
            horizontal,
            x,
            y,
        } => {
            let scrolled = cx.update(|window, cx| {
                let size = window.viewport_size();
                let position = point(
                    px(f32::from(size.width) * x),
                    px(f32::from(size.height) * y),
                );
                let delta = if horizontal {
                    point(px(-rows * WHEEL_UNIT), px(0.0))
                } else {
                    point(px(0.0), px(-rows * WHEEL_UNIT))
                };
                window.dispatch_event(
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(delta),
                        modifiers: Modifiers::none(),
                        touch_phase: TouchPhase::Moved,
                    }),
                    cx,
                );
            });
            if scrolled.is_err() {
                log.write("window gone", cx).await;
                return Flow::Stop;
            }
            Flow::Continue
        }
        Step::Wait(duration) => {
            cx.background_executor().timer(duration).await;
            Flow::Continue
        }
        Step::Shot(path) => capture(&path, dialect, log, cx).await,
        Step::Quit => {
            log.write("done quit", cx).await;
            let _ = cx.update(|_, cx| cx.quit());
            Flow::Stop
        }
    }
}

/// Raises the window, lets it settle, then captures every display the dialect asks for.
async fn capture(path: &Path, dialect: Dialect, log: &Log, cx: &mut AsyncWindowContext) -> Flow {
    let displays = cx.update(|window, cx| {
        window.activate_window();
        cx.activate(true);
        if dialect == Dialect::Fleet {
            cx.displays().len()
        } else {
            1
        }
    });
    let Ok(displays) = displays else {
        log.write("window gone", cx).await;
        return Flow::Stop;
    };
    cx.background_executor().timer(SETTLE).await;
    let targets = shot_paths(path, displays);
    let path = path.to_path_buf();
    let result = cx
        .background_spawn(async move {
            let status = Command::new(SCREENCAPTURE)
                .arg("-x")
                .args(&targets)
                .status();
            screenshot_messages(dialect, &path, &targets, status)
        })
        .await;
    for message in result.messages {
        log.write(message, cx).await;
    }
    if result.succeeded {
        Flow::Continue
    } else {
        Flow::Failed
    }
}

struct ScreenshotResult {
    messages: Vec<String>,
    succeeded: bool,
}

fn screenshot_messages(
    _dialect: Dialect,
    path: &Path,
    targets: &[PathBuf],
    status: std::io::Result<std::process::ExitStatus>,
) -> ScreenshotResult {
    match status {
        Ok(status) if status.success() => ScreenshotResult {
            messages: targets
                .iter()
                .map(|target| format!("done shot {}", target.display()))
                .collect(),
            succeeded: true,
        },
        Ok(status) => ScreenshotResult {
            messages: vec![format!("error shot {}: {status}", path.display())],
            succeeded: false,
        },
        Err(error) => ScreenshotResult {
            messages: vec![format!("error shot {}: {error}", path.display())],
            succeeded: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, IntoElement, Render, TestAppContext, div};

    struct DriverWindow;
    impl Render for DriverWindow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct Script(PathBuf);
    impl Drop for Script {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(log_path(&self.0));
        }
    }

    #[gpui::test]
    async fn closing_a_window_cancels_its_waiting_driver(cx: &mut TestAppContext) {
        let script = Script(std::env::temp_dir().join(format!(
            "fleet-driver-window-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )));
        std::fs::write(&script.0, "wait 600000\nkey j\n").expect("write script");
        let window = cx.add_window(|_, _| DriverWindow);
        let task = window
            .update(cx, |_, window, cx| {
                spawn(script.0.clone(), window, cx, Dialect::Lazygit, || {
                    "0".into()
                })
            })
            .expect("start driver");
        cx.run_until_parked();
        cx.executor().advance_clock(POLL);
        cx.run_until_parked();
        let before = std::fs::read_to_string(log_path(&script.0)).expect("driver log");
        assert!(before.contains("run wait 600000\n"), "{before}");
        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("close window");
        task.await;
        let after = std::fs::read_to_string(log_path(&script.0)).expect("driver log");
        assert_eq!(
            before, after,
            "closing must cancel the wait without dispatching the next key"
        );
    }

    #[gpui::test]
    async fn closing_an_idle_window_cancels_its_driver(cx: &mut TestAppContext) {
        let script = Script(std::env::temp_dir().join(format!(
            "fleet-driver-idle-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )));
        std::fs::write(&script.0, "").expect("write script");
        let window = cx.add_window(|_, _| DriverWindow);
        let task = window
            .update(cx, |_, window, cx| {
                spawn(script.0.clone(), window, cx, Dialect::Fleet, || "0".into())
            })
            .expect("start driver");
        cx.run_until_parked();
        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("close window");
        task.await;
    }

    #[cfg(unix)]
    #[test]
    fn failed_screenshot_emits_error() {
        use std::os::unix::process::ExitStatusExt;
        let path = Path::new("/tmp/help.png");
        let targets = shot_paths(path, 2);
        let success = screenshot_messages(
            Dialect::Fleet,
            path,
            &targets,
            Ok(std::process::ExitStatus::from_raw(0)),
        );
        assert!(success.succeeded);
        assert_eq!(
            success.messages,
            ["done shot /tmp/help.png", "done shot /tmp/help-2.png"]
        );

        for dialect in [Dialect::Fleet, Dialect::Lazygit] {
            let failure = screenshot_messages(
                dialect,
                path,
                &targets,
                Err(std::io::Error::other("failed")),
            );
            assert!(!failure.succeeded);
            assert_eq!(failure.messages, ["error shot /tmp/help.png: failed"]);
        }

        let failure = screenshot_messages(
            Dialect::Lazygit,
            path,
            &targets[..1],
            Ok(std::process::ExitStatus::from_raw(256)),
        );
        assert!(!failure.succeeded);
        assert!(failure.messages[0].starts_with("error shot /tmp/help.png:"));
    }
}
