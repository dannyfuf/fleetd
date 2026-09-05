//! A developer-only scripted-input driver, so an automated reviewer can drive the GUI without
//! macOS Accessibility permission. Ported from `fleet-app`'s `drive.rs`.
//!
//! It is enabled only when `FLEET_LAZYGIT_DRIVE` names a file path. The window then polls that
//! file every 100 ms, reads the lines appended since the previous poll, and executes each one:
//!
//! ```text
//! key ctrl-d ?          dispatch one or more gpui keystrokes to the window
//! type hello world      dispatch every character as a keystroke, so text inputs receive it
//! wheel 5               scroll the pointer wheel five rows down (negative scrolls up)
//! wheel 5 0.75 0.5      the same, aimed at 75 % across and 50 % down the window
//! hwheel 4              scroll four cells to the right (a sideways trackpad swipe)
//! wait 500              pause the script for 500 ms
//! shot /tmp/help.png    raise the window, then screencapture the screen into it
//! quit                  cx.quit()
//! ```
//!
//! Blank lines and lines starting with `#` are ignored. Every executed line, every parse error
//! and every finished screenshot is appended to `$FLEET_LAZYGIT_DRIVE.log`, so a script runner
//! can wait on `done <line>` instead of sleeping. Keystrokes go through
//! `Window::dispatch_keystroke`, so they resolve against the real focus chain.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use gpui::{
    App, AppContext, Keystroke, Modifiers, PlatformInput, ScrollDelta, ScrollWheelEvent, Task,
    TouchPhase, Window, point, px,
};

/// How often the driver looks for newly appended lines.
const POLL: Duration = Duration::from_millis(100);
/// How many pixels one `wheel` unit scrolls. The diff row height, so `wheel 5` is five rows.
const WHEEL_UNIT: f32 = 18.0;
/// How long the window is given to come to the front before a screenshot.
const SETTLE: Duration = Duration::from_millis(250);
/// The macOS screenshot tool, invoked without its capture sound.
const SCREENCAPTURE: &str = "/usr/sbin/screencapture";

/// The script path from `FLEET_LAZYGIT_DRIVE`, when the driver is enabled.
#[must_use]
pub fn script_path() -> Option<PathBuf> {
    std::env::var_os("FLEET_LAZYGIT_DRIVE")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// One executable line of a drive script.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// Dispatch these keystrokes, in order.
    Keys(Vec<Keystroke>),
    /// Dispatch every character of this text as a keystroke.
    Type(String),
    /// Scroll the wheel at a fractional window position.
    Wheel {
        /// How much to scroll; positive moves the content up (or left), as a real wheel does.
        rows: f32,
        /// Whether the delta is horizontal — the axis `uniform_list` ignores.
        horizontal: bool,
        /// Where the pointer sits, as a fraction of the window width.
        x: f32,
        /// Where the pointer sits, as a fraction of the window height.
        y: f32,
    },
    /// Pause the script.
    Wait(Duration),
    /// Capture the screen into this file.
    Shot(PathBuf),
    /// Quit the application.
    Quit,
}

/// Parses one script line. `Ok(None)` is a blank line or a comment.
fn parse(line: &str) -> Result<Option<Step>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let (command, rest) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    let rest = rest.trim();
    match command {
        "key" => {
            if rest.is_empty() {
                return Err("key needs at least one keystroke".to_owned());
            }
            let mut keys = Vec::new();
            for token in rest.split_whitespace() {
                let keystroke = Keystroke::parse(token)
                    .map_err(|error| format!("bad keystroke {token:?}: {error}"))?;
                keys.push(keystroke);
            }
            Ok(Some(Step::Keys(keys)))
        }
        // `type` keeps its argument verbatim, spaces included.
        "type" => {
            let text = line.trim_start().strip_prefix("type").unwrap_or("");
            let text = text.strip_prefix(' ').unwrap_or(text);
            if text.is_empty() {
                return Err("type needs some text".to_owned());
            }
            Ok(Some(Step::Type(text.to_owned())))
        }
        // `wheel <rows> [x-fraction] [y-fraction]`, or `hwheel <cells> …` for the sideways
        // axis. The default position is the main panel: three quarters across, half way down.
        "wheel" | "hwheel" => {
            let mut parts = rest.split_whitespace();
            let rows = parts
                .next()
                .and_then(|token| token.parse::<f32>().ok())
                .ok_or_else(|| format!("wheel needs a row count, got {rest:?}"))?;
            let x = parts
                .next()
                .and_then(|token| token.parse::<f32>().ok())
                .unwrap_or(0.75);
            let y = parts
                .next()
                .and_then(|token| token.parse::<f32>().ok())
                .unwrap_or(0.5);
            Ok(Some(Step::Wheel {
                rows,
                horizontal: command == "hwheel",
                x,
                y,
            }))
        }
        "wait" => rest
            .parse::<u64>()
            .map(|millis| Some(Step::Wait(Duration::from_millis(millis))))
            .map_err(|_| format!("wait needs a millisecond count, got {rest:?}")),
        "shot" => {
            if rest.is_empty() {
                return Err("shot needs a path".to_owned());
            }
            Ok(Some(Step::Shot(PathBuf::from(rest))))
        }
        "quit" => Ok(Some(Step::Quit)),
        other => Err(format!("unknown command {other:?}")),
    }
}

/// The keystroke that types `character`, as the platform would report it.
fn keystroke_for(character: char) -> Keystroke {
    let (key, shift) = match character {
        ' ' => ("space".to_owned(), false),
        '\t' => ("tab".to_owned(), false),
        upper if upper.is_ascii_uppercase() => (upper.to_ascii_lowercase().to_string(), true),
        shifted if unshifted_ascii(shifted).is_some() => (
            unshifted_ascii(shifted).unwrap_or(shifted).to_string(),
            true,
        ),
        other => (other.to_string(), false),
    };
    Keystroke {
        modifiers: Modifiers {
            shift,
            ..Modifiers::none()
        },
        key,
        key_char: (character != '\t').then(|| character.to_string()),
    }
}

/// Adds the composed text macOS supplies on a real printable `KeyDownEvent`.
///
/// `Window::dispatch_keystroke` does not run a synthetic keystroke through the platform's
/// keyboard-layout translation, so the driver has to fill this field itself.
fn with_simulated_key_char(mut keystroke: Keystroke) -> Keystroke {
    if keystroke.key_char.is_some()
        || keystroke.modifiers.control
        || keystroke.modifiers.alt
        || keystroke.modifiers.platform
    {
        return keystroke;
    }
    keystroke.key_char = match keystroke.key.as_str() {
        "space" => Some(" ".to_owned()),
        key if key.chars().count() == 1 => {
            let character = key.chars().next().unwrap_or_default();
            Some(if keystroke.modifiers.shift {
                shifted_ascii(character).unwrap_or(character).to_string()
            } else {
                character.to_string()
            })
        }
        _ => None,
    };
    keystroke
}

fn unshifted_ascii(character: char) -> Option<char> {
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const UNSHIFTED: &str = "`1234567890-=[]\\;',./";
    SHIFTED
        .chars()
        .position(|candidate| candidate == character)
        .and_then(|index| UNSHIFTED.chars().nth(index))
}

fn shifted_ascii(character: char) -> Option<char> {
    const UNSHIFTED: &str = "`1234567890-=[]\\;',./";
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    if character.is_ascii_lowercase() {
        return Some(character.to_ascii_uppercase());
    }
    UNSHIFTED
        .chars()
        .position(|candidate| candidate == character)
        .and_then(|index| SHIFTED.chars().nth(index))
}

/// Appends one timestamped line to `$FLEET_LAZYGIT_DRIVE.log`.
fn log_line(log: &Path, message: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log) {
        let _ignored = writeln!(file, "{stamp} {message}");
    }
}

/// The log file for a script path: `<script>.log`.
fn log_path(script: &Path) -> PathBuf {
    let mut name = script.as_os_str().to_owned();
    name.push(".log");
    PathBuf::from(name)
}

/// Reads the lines appended to a file since the last read, by byte offset.
struct Tail {
    path: PathBuf,
    offset: u64,
    partial: String,
}

impl Tail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: String::new(),
        }
    }

    fn poll(&mut self) -> Vec<String> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(length) = file.metadata().map(|meta| meta.len()) else {
            return Vec::new();
        };
        if length < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if length == self.offset {
            return Vec::new();
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buffer = Vec::new();
        let Ok(read) = file.read_to_end(&mut buffer) else {
            return Vec::new();
        };
        self.offset += u64::try_from(read).unwrap_or(0);
        self.partial.push_str(&String::from_utf8_lossy(&buffer));

        let mut lines = Vec::new();
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            lines.push(line.trim_end_matches(['\r', '\n']).to_owned());
        }
        lines
    }
}

/// Spawns the driver on `window`. Call only when [`script_path`] is `Some`.
pub fn spawn(script: PathBuf, window: &Window, cx: &App) -> Task<()> {
    let log = log_path(&script);
    tracing::info!(script = %script.display(), "drive: script driver armed");
    log_line(&log, &format!("start {}", script.display()));

    window.spawn(cx, async move |cx| {
        let mut tail = Tail::new(script);
        loop {
            cx.background_executor().timer(POLL).await;
            for line in tail.poll() {
                let step = match parse(&line) {
                    Ok(None) => continue,
                    Ok(Some(step)) => step,
                    Err(error) => {
                        log_line(&log, &format!("error {line:?}: {error}"));
                        continue;
                    }
                };
                log_line(&log, &format!("run {line}"));
                match step {
                    Step::Keys(keys) => {
                        for key in keys {
                            let key = with_simulated_key_char(key);
                            let handled =
                                cx.update(|window, cx| window.dispatch_keystroke(key.clone(), cx));
                            match handled {
                                Ok(true) => {}
                                Ok(false) => {
                                    log_line(&log, &format!("unhandled key {}", key.unparse()));
                                }
                                Err(error) => {
                                    log_line(&log, &format!("window gone: {error}"));
                                    return;
                                }
                            }
                        }
                    }
                    Step::Type(text) => {
                        for character in text.chars() {
                            let keystroke = keystroke_for(character);
                            if cx
                                .update(|window, cx| window.dispatch_keystroke(keystroke, cx))
                                .is_err()
                            {
                                log_line(&log, "window gone");
                                return;
                            }
                        }
                    }
                    Step::Wheel {
                        rows,
                        horizontal,
                        x,
                        y,
                    } => {
                        // A synthetic `ScrollWheelEvent` goes through the same hit test a real
                        // one does, so it exercises the element's own listener rather than a
                        // test-only shortcut.
                        let sent = cx.update(|window, cx| {
                            let size = window.viewport_size();
                            let position = point(
                                px(f32::from(size.width) * x),
                                px(f32::from(size.height) * y),
                            );
                            window.dispatch_event(
                                PlatformInput::ScrollWheel(ScrollWheelEvent {
                                    position,
                                    delta: ScrollDelta::Pixels(if horizontal {
                                        point(px(-rows * WHEEL_UNIT), px(0.0))
                                    } else {
                                        point(px(0.0), px(-rows * WHEEL_UNIT))
                                    }),
                                    modifiers: Modifiers::none(),
                                    touch_phase: TouchPhase::Moved,
                                }),
                                cx,
                            );
                        });
                        if sent.is_err() {
                            log_line(&log, "window gone");
                            return;
                        }
                    }
                    Step::Wait(duration) => cx.background_executor().timer(duration).await,
                    Step::Shot(path) => {
                        // A driven app is usually launched into the background, so raise the
                        // window and let the compositor settle before photographing it.
                        if cx
                            .update(|window, cx| {
                                window.activate_window();
                                cx.activate(true);
                            })
                            .is_err()
                        {
                            log_line(&log, "window gone");
                            return;
                        }
                        cx.background_executor().timer(SETTLE).await;
                        let target = path.clone();
                        let status = cx
                            .background_spawn(async move {
                                Command::new(SCREENCAPTURE).arg("-x").arg(&target).status()
                            })
                            .await;
                        match status {
                            Ok(status) if status.success() => {
                                log_line(&log, &format!("done shot {}", path.display()));
                            }
                            other => {
                                log_line(&log, &format!("shot failed: {other:?}"));
                            }
                        }
                    }
                    Step::Quit => {
                        log_line(&log, "done quit");
                        let _ignored = cx.update(|_window, cx| cx.quit());
                        return;
                    }
                }
                log_line(&log, &format!("done {line}"));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        assert_eq!(parse("").expect("blank"), None);
        assert_eq!(parse("# note").expect("comment"), None);
        assert_eq!(
            parse("wait 250").expect("wait"),
            Some(Step::Wait(Duration::from_millis(250)))
        );
        assert_eq!(
            parse("type a b").expect("type"),
            Some(Step::Type("a b".to_owned()))
        );
        assert_eq!(parse("quit").expect("quit"), Some(Step::Quit));
        assert_eq!(
            parse("wheel 5").expect("wheel"),
            Some(Step::Wheel {
                rows: 5.0,
                horizontal: false,
                x: 0.75,
                y: 0.5
            })
        );
        assert_eq!(
            parse("wheel -2 0.5 0.25").expect("wheel"),
            Some(Step::Wheel {
                rows: -2.0,
                horizontal: false,
                x: 0.5,
                y: 0.25
            })
        );
        assert_eq!(
            parse("hwheel 3").expect("hwheel"),
            Some(Step::Wheel {
                rows: 3.0,
                horizontal: true,
                x: 0.75,
                y: 0.5
            })
        );
        assert!(parse("wheel").is_err());
        assert!(parse("hwheel").is_err());
        assert!(parse("nope").is_err());
        let Some(Step::Keys(keys)) = parse("key ctrl-d 2").expect("keys") else {
            panic!("expected keystrokes");
        };
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn a_printable_keystroke_carries_its_character() {
        let keystroke = with_simulated_key_char(Keystroke::parse("a").expect("parses"));
        assert_eq!(keystroke.key_char.as_deref(), Some("a"));
        let shifted = with_simulated_key_char(Keystroke::parse("A").expect("parses"));
        assert_eq!(shifted.key_char.as_deref(), Some("A"));
        let chord = with_simulated_key_char(Keystroke::parse("ctrl-d").expect("parses"));
        assert_eq!(chord.key_char, None);
    }
}
