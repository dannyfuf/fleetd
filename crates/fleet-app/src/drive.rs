//! A developer-only scripted-input driver, so an automated reviewer can drive the GUI without
//! macOS Accessibility permission.
//!
//! The driver is enabled only when `FLEET_DRIVE` names a file path. When it is set, the shell
//! spawns one foreground task on the window that polls that file every 100 ms, reads the lines
//! appended since the previous poll, and executes each one:
//!
//! ```text
//! key ctrl-s ?          dispatch one or more gpui keystrokes to the window
//! type hello world      dispatch every character as a keystroke, so text inputs receive it
//! wait 500              pause the script for 500 ms
//! shot /tmp/help.png    raise the window, then screencapture every display into it
//! quit                  cx.quit()
//! ```
//!
//! Blank lines and lines starting with `#` are ignored. Every executed line, every parse error
//! and every finished screenshot is appended to `$FLEET_DRIVE.log` with a timestamp, so the
//! driver is observable from the outside.
//!
//! When `FLEET_DRIVE` is unset nothing is spawned and the app is byte-for-byte unaffected.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use gpui::{App, AppContext, Keystroke, Modifiers, Task, Window};

/// How often the driver looks for newly appended lines.
const POLL: Duration = Duration::from_millis(100);
/// How long the window is given to come to the front before a screenshot.
const SETTLE: Duration = Duration::from_millis(250);
/// The macOS screenshot tool, invoked without its capture sound.
const SCREENCAPTURE: &str = "/usr/sbin/screencapture";

/// The script path from `FLEET_DRIVE`, when the driver is enabled.
#[must_use]
pub fn script_path() -> Option<PathBuf> {
    std::env::var_os("FLEET_DRIVE")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// One executable line of a drive script.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// Dispatch these keystrokes, in order.
    Keys(Vec<Keystroke>),
    /// Dispatch every character of this text as a keystroke.
    Type(String),
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
        "wait" => rest
            .parse::<u64>()
            .map(|ms| Some(Step::Wait(Duration::from_millis(ms))))
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

/// The files one `shot` writes: `screencapture` takes one path per display, in display order.
///
/// A single display is the plain case and gets exactly the requested path. With more, the window
/// may well be on a display the first file does not show — `screencapture` photographs one
/// display per file and gpui reports window bounds relative to the window's own screen, so there
/// is nothing to aim with — and the extra displays land beside it as `<name>-2.png`, `<name>-3.png`.
fn shot_paths(path: &Path, displays: usize) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let stem = path
        .file_stem()
        .map_or_else(|| "shot".into(), |stem| stem.to_string_lossy().into_owned());
    let extension = path
        .extension()
        .map_or_else(|| "png".into(), |ext| ext.to_string_lossy().into_owned());
    for index in 2..=displays.max(1) {
        paths.push(path.with_file_name(format!("{stem}-{index}.{extension}")));
    }
    paths
}

/// Appends one timestamped line to `$FLEET_DRIVE.log`.
fn log_line(log: &Path, message: &str) {
    let stamp = chrono::Local::now().format("%H:%M:%S%.3f");
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
    /// A trailing line that has not been terminated yet.
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

    /// Every complete line appended since the previous call.
    fn poll(&mut self) -> Vec<String> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(length) = file.metadata().map(|meta| meta.len()) else {
            return Vec::new();
        };
        // The script was replaced or truncated: start over rather than read garbage.
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
///
/// The returned task polls until the script says `quit`, the window goes away, or the app shuts
/// down; the caller detaches it.
pub fn spawn(script: PathBuf, window: &Window, cx: &App) -> Task<()> {
    let log = log_path(&script);
    tracing::info!(script = %script.display(), log = %log.display(), "drive: script driver armed");
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
                        tracing::warn!(line = %line, %error, "drive: bad script line");
                        log_line(&log, &format!("error {line:?}: {error}"));
                        continue;
                    }
                };
                tracing::info!(line = %line, "drive: run");
                log_line(&log, &format!("run {line}"));

                match step {
                    Step::Keys(keys) => {
                        for key in keys {
                            let key = with_simulated_key_char(key);
                            let handled = cx.update(|window, cx| {
                                window.dispatch_keystroke(key.clone(), cx)
                            });
                            match handled {
                                Ok(handled) => {
                                    if !handled {
                                        tracing::warn!(key = %key.unparse(), "drive: keystroke unhandled");
                                        log_line(&log, &format!("unhandled key {}", key.unparse()));
                                    }
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
                    Step::Wait(duration) => cx.background_executor().timer(duration).await,
                    Step::Shot(path) => {
                        // A driven app is usually launched into the background, so whatever was
                        // frontmost would otherwise end up in the picture: raise the window and
                        // let the compositor settle first.
                        let displays = cx.update(|window, cx| {
                            window.activate_window();
                            cx.activate(true);
                            cx.displays().len()
                        });
                        let Ok(displays) = displays else {
                            log_line(&log, "window gone");
                            return;
                        };
                        cx.background_executor().timer(SETTLE).await;
                        let targets = shot_paths(&path, displays);
                        let arguments = targets.clone();
                        let status = cx
                            .background_spawn(async move {
                                let mut command = Command::new(SCREENCAPTURE);
                                command.arg("-x");
                                command.args(&arguments);
                                command.status()
                            })
                            .await;
                        match status {
                            Ok(status) if status.success() => {
                                tracing::info!(path = %path.display(), displays, "drive: screenshot");
                                for target in &targets {
                                    log_line(&log, &format!("done shot {}", target.display()));
                                }
                            }
                            Ok(status) => {
                                tracing::warn!(path = %path.display(), ?status, "drive: screencapture failed");
                                log_line(
                                    &log,
                                    &format!("error shot {}: {status}", path.display()),
                                );
                            }
                            Err(error) => {
                                tracing::warn!(path = %path.display(), %error, "drive: screencapture failed");
                                log_line(&log, &format!("error shot {}: {error}", path.display()));
                            }
                        }
                    }
                    Step::Quit => {
                        tracing::info!("drive: quit");
                        log_line(&log, "done quit");
                        let _ignored = cx.update(|_window, cx| cx.quit());
                        return;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_comment_lines_are_skipped() {
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("   "), Ok(None));
        assert_eq!(parse("# a note"), Ok(None));
    }

    #[test]
    fn key_accepts_several_keystrokes() {
        let Ok(Some(Step::Keys(keys))) = parse("key ctrl-s ?") else {
            panic!("expected keystrokes");
        };
        assert_eq!(keys.len(), 2);
        assert!(keys[0].modifiers.control);
        assert_eq!(keys[0].key, "s");
        assert_eq!(keys[1].key, "?");
    }

    #[test]
    fn type_keeps_inner_spaces() {
        assert_eq!(
            parse("type hello  world"),
            Ok(Some(Step::Type("hello  world".to_owned())))
        );
    }

    #[test]
    fn uppercase_types_as_shifted_lowercase() {
        let keystroke = keystroke_for('A');
        assert_eq!(keystroke.key, "a");
        assert!(keystroke.modifiers.shift);
        assert_eq!(keystroke.key_char.as_deref(), Some("A"));
        assert_eq!(keystroke_for(' ').key, "space");
    }

    #[test]
    fn shifted_punctuation_has_the_platform_composed_text() {
        let typed = keystroke_for(':');
        assert_eq!(typed.key, ";");
        assert!(typed.modifiers.shift);
        assert_eq!(typed.key_char.as_deref(), Some(":"));

        let parsed = Keystroke::parse("shift-;").expect("valid keystroke");
        let simulated = with_simulated_key_char(parsed);
        assert_eq!(simulated.key, ";");
        assert_eq!(simulated.key_char.as_deref(), Some(":"));
    }

    #[test]
    fn wait_shot_and_quit_parse() {
        assert_eq!(
            parse("wait 250"),
            Ok(Some(Step::Wait(Duration::from_millis(250))))
        );
        assert_eq!(
            parse("shot /tmp/a.png"),
            Ok(Some(Step::Shot(PathBuf::from("/tmp/a.png"))))
        );
        assert_eq!(parse("quit"), Ok(Some(Step::Quit)));
        assert!(parse("nope").is_err());
        assert!(parse("wait soon").is_err());
    }

    #[test]
    fn tail_reads_only_new_complete_lines() {
        let dir = std::env::temp_dir().join(format!("fleet-drive-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("script.txt");
        std::fs::write(&path, "one\ntw").expect("write");
        let mut tail = Tail::new(path.clone());
        assert_eq!(tail.poll(), vec!["one".to_owned()]);
        let mut file = OpenOptions::new().append(true).open(&path).expect("append");
        writeln!(file, "o").expect("append");
        assert_eq!(tail.poll(), vec!["two".to_owned()]);
        assert!(tail.poll().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shot_paths_add_one_file_per_extra_display() {
        let path = Path::new("/tmp/help.png");
        assert_eq!(shot_paths(path, 1), vec![PathBuf::from("/tmp/help.png")]);
        assert_eq!(
            shot_paths(path, 3),
            vec![
                PathBuf::from("/tmp/help.png"),
                PathBuf::from("/tmp/help-2.png"),
                PathBuf::from("/tmp/help-3.png"),
            ]
        );
    }

    #[test]
    fn log_path_appends_the_suffix() {
        assert_eq!(
            log_path(Path::new("/tmp/script.txt")),
            PathBuf::from("/tmp/script.txt.log")
        );
    }
}
