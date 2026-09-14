//! Screenshot capture backends.
//!
//! The runner captures, never the app. `shot` only raises the window, settles it and answers with
//! its geometry and frame; the pixels are taken from outside by the tool this file names. That
//! keeps `grim` and `hyprctl` out of the product binary and makes a new capture tool a change to
//! this file alone.

use crate::{lane::LaneBackend, rundir::RunDirectory};
use anyhow::Context as _;
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread::{JoinHandle, sleep, spawn},
    time::{Duration, Instant},
};

/// How long a capture tool may take before the run is told instead of waiting forever.
///
/// `grim` waits for a frame, and a blanked or locked output produces none — measured on this box
/// as a capture of a DPMS-off monitor that stalled for minutes. A harness that hangs is worse
/// than one that fails, because nothing downstream ever reports anything.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// Gap between checks on a running tool.
const WAIT_INTERVAL: Duration = Duration::from_millis(20);

/// A source of pixels for one `shot` line.
pub trait Capture {
    /// Writes one PNG of what this source shows to `output`.
    fn capture(&self, output: &Path) -> anyhow::Result<()>;
}

/// A window rectangle in the compositor's logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Logical width.
    pub width: u32,
    /// Logical height.
    pub height: u32,
}

/// Captures a whole compositor output — the `virtual` lane's isolated monitor.
///
/// The output holds nothing but the harness window, so capturing all of it needs no window
/// geometry and cannot crop the window if the app resizes itself mid-run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputCapture {
    output: String,
}

impl OutputCapture {
    /// Captures the compositor output named `output`.
    pub fn new(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
        }
    }
}

impl Capture for OutputCapture {
    #[cfg(target_os = "macos")]
    fn capture(&self, _output: &Path) -> anyhow::Result<()> {
        anyhow::bail!(
            "whole-output capture of {} needs Hyprland and grim; macOS has no virtual lane",
            self.output
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn capture(&self, output: &Path) -> anyhow::Result<()> {
        run_tool(
            "grim",
            &["-o", &self.output, &output.to_string_lossy()],
            "install grim or use --lane headless",
        )
    }
}

/// Captures one rectangle of the developer's screen — the `attach` lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionCapture {
    rect: Rect,
}

impl RegionCapture {
    /// Captures the logical rectangle `rect`.
    pub fn new(rect: Rect) -> Self {
        Self { rect }
    }
}

impl Capture for RegionCapture {
    #[cfg(target_os = "macos")]
    fn capture(&self, output: &Path) -> anyhow::Result<()> {
        // `-x` drops the shutter sound; `-R` takes the rectangle in screen points.
        run_tool(
            "screencapture",
            &[
                "-x",
                &format!(
                    "-R{},{},{},{}",
                    self.rect.x, self.rect.y, self.rect.width, self.rect.height
                ),
                &output.to_string_lossy(),
            ],
            "screencapture ships with macOS; use --lane headless without a screen",
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn capture(&self, output: &Path) -> anyhow::Result<()> {
        run_tool(
            "grim",
            &[
                "-g",
                &format!(
                    "{},{} {}x{}",
                    self.rect.x, self.rect.y, self.rect.width, self.rect.height
                ),
                &output.to_string_lossy(),
            ],
            "install grim or use --lane headless",
        )
    }
}

/// Resolves the harness window's rectangle from the compositor, by its unique title.
#[cfg(target_os = "macos")]
pub fn window_rect(title: &str) -> anyhow::Result<Rect> {
    anyhow::bail!("locating the window titled {title:?} is unimplemented on macOS")
}

/// Resolves the harness window's rectangle from the compositor, by its unique title.
///
/// The title carries the run id, so a second Fleet on the same screen is never captured by
/// mistake; an ambiguous match is an error rather than a guess.
#[cfg(not(target_os = "macos"))]
pub fn window_rect(title: &str) -> anyhow::Result<Rect> {
    let windows: Vec<_> = crate::lane::clients()?
        .into_iter()
        .filter(|client| client.title == title)
        .collect();
    let [window] = windows.as_slice() else {
        anyhow::bail!(
            "expected exactly one window titled {title:?}, the compositor reports {}",
            windows.len()
        );
    };
    Ok(Rect {
        x: window.at[0],
        y: window.at[1],
        width: window.size[0].max(0).unsigned_abs(),
        height: window.size[1].max(0).unsigned_abs(),
    })
}

/// Captures the pixels for one `shot` line and returns the file it wrote.
///
/// Baseline comparison is deliberately not done here: it needs the scenario's *source* path to
/// key a baseline by, which this signature does not carry. `scenario::dispatch` holds that path
/// and hands the returned file to `baseline::Baselines::check`.
pub fn capture_command(
    backend: &dyn LaneBackend,
    run_dir: &RunDirectory,
    sequence: usize,
    name: &str,
) -> anyhow::Result<PathBuf> {
    let path = shot_path(&run_dir.shots, sequence, name);
    std::fs::create_dir_all(&run_dir.shots)
        .with_context(|| format!("create {}", run_dir.shots.display()))?;
    backend
        .capture(&path)
        .with_context(|| format!("capture shot {name:?}"))?;
    anyhow::ensure!(
        path.is_file(),
        "the capture tool reported success but wrote no {}",
        path.display()
    );
    Ok(path)
}

/// Captures the evidence a failed line leaves behind, as `shots/failure-NNN.png`.
///
/// The name is deliberately not `NNN-failure.png`: `docs/TESTING-HARNESS.md` §6 puts every
/// failure shot under one prefix so a reader scanning the directory sees them together, however
/// far apart the failing lines were.
pub fn capture_failure(
    backend: &dyn LaneBackend,
    run_dir: &RunDirectory,
    sequence: usize,
) -> anyhow::Result<PathBuf> {
    let path = run_dir.shots.join(format!("failure-{sequence:03}.png"));
    std::fs::create_dir_all(&run_dir.shots)
        .with_context(|| format!("create {}", run_dir.shots.display()))?;
    backend
        .capture(&path)
        .with_context(|| format!("capture the failure at line {sequence}"))?;
    anyhow::ensure!(
        path.is_file(),
        "the capture tool reported success but wrote no {}",
        path.display()
    );
    Ok(path)
}

/// Names a shot after its command sequence, so the run order is obvious in the directory listing.
fn shot_path(shots: &Path, sequence: usize, name: &str) -> PathBuf {
    shots.join(format!("{sequence:03}-{}.png", file_safe(name)))
}

/// Keeps a scenario-authored shot name inside one path segment.
fn file_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        "shot".to_owned()
    } else {
        safe
    }
}

/// Runs one capture tool and discards its output, failing if it writes to stderr and exits badly.
fn run_tool(tool: &str, arguments: &[&str], hint: &str) -> anyhow::Result<()> {
    let output = run(tool, arguments, hint, CAPTURE_TIMEOUT)?;
    if !output.status.success() {
        anyhow::bail!(
            "{tool} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Runs an external tool under a deadline, turning a missing binary into an instruction.
///
/// Every subprocess the harness shells out to goes through here: a missing tool must say what to
/// install, and a wedged one must end the command rather than the run.
pub(crate) fn run(
    tool: &str,
    arguments: &[&str],
    hint: &str,
    timeout: Duration,
) -> anyhow::Result<Output> {
    let mut child = Command::new(tool)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("{tool} not found; {hint}")
            } else {
                anyhow::Error::new(error).context(format!("run {tool}"))
            }
        })?;
    // Drain both pipes off-thread. A tool whose output outgrows the pipe buffer would otherwise
    // block on the write while this thread blocks on its exit.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .with_context(|| format!("wait for {tool}"))?
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                child.kill().with_context(|| format!("stop {tool}"))?;
                child.wait().with_context(|| format!("reap {tool}"))?;
                anyhow::bail!(
                    "{tool} did not finish within {}s and was stopped; a blanked or locked screen \
                     produces no frame for a capture to wait on",
                    timeout.as_secs()
                );
            }
            None => sleep(WAIT_INTERVAL),
        }
    };
    Ok(Output {
        status,
        stdout: collect(stdout)?,
        stderr: collect(stderr)?,
    })
}

/// Reads one of the child's pipes to the end on its own thread.
fn drain<R: Read + Send + 'static>(stream: Option<R>) -> JoinHandle<Vec<u8>> {
    spawn(move || {
        let mut buffer = Vec::new();
        if let Some(mut stream) = stream
            && let Err(error) = stream.read_to_end(&mut buffer)
        {
            eprintln!("warning: could not read a tool's output: {error}");
        }
        buffer
    })
}

/// Joins a drain thread, whose only failure is a panic inside it.
fn collect(handle: JoinHandle<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("a tool output reader panicked"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shots_are_named_by_sequence_and_stay_inside_one_directory() {
        let shots = Path::new("/tmp/run/shots");
        assert_eq!(
            shot_path(shots, 7, "hub"),
            Path::new("/tmp/run/shots/007-hub.png")
        );
        assert_eq!(
            shot_path(shots, 142, "help_overlay-2"),
            Path::new("/tmp/run/shots/142-help_overlay-2.png")
        );
        assert_eq!(
            shot_path(shots, 1, "../../etc/passwd"),
            Path::new("/tmp/run/shots/001-..-..-etc-passwd.png"),
            "a scenario-authored name must never escape the shots directory"
        );
        assert_eq!(
            shot_path(shots, 1, ""),
            Path::new("/tmp/run/shots/001-shot.png")
        );
    }

    #[test]
    fn a_stalled_tool_ends_the_command_rather_than_the_run() {
        let error = run(
            "sh",
            &["-c", "sleep 30"],
            "sh ships with every unix",
            Duration::from_millis(300),
        )
        .expect_err("the tool outlives its deadline");
        assert!(
            format!("{error}").starts_with("sh did not finish within 0s and was stopped;"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn a_missing_tool_says_which_one_and_what_to_do() {
        let error = run_tool(
            "fleet-harness-no-such-capture-tool",
            &[],
            "install grim or use --lane headless",
        )
        .expect_err("the tool does not exist");
        assert_eq!(
            format!("{error}"),
            "fleet-harness-no-such-capture-tool not found; install grim or use --lane headless"
        );
    }
}
