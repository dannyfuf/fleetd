//! Display-lane lifecycle: where the harness window lives, and who cleans up after it.
//!
//! Three lanes answer one question — what does the window draw on? `headless` draws nothing and
//! runs every layout and state path; `virtual` draws on a Hyprland output the developer never
//! sees; `attach` draws on the developer's own screen so a run can be watched. Every compositor
//! call the harness makes lives in this file, so a different compositor is a new backend here and
//! nothing else.
//!
//! The isolated output is the one thing a run can leak into a real session, so its removal is
//! guarded three times: [`LaneBackend::teardown`], a `Drop` that calls it, and a detached watchdog
//! process that removes the output once the runner's pid is gone — which is what survives a
//! `kill -INT` or a crash.

use crate::{
    baseline,
    capture::{self, Capture as _},
};
use serde::Deserialize;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread::sleep,
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

/// Mode pinned on the isolated output. Two runs must lay out identically, and a headless output
/// is created at the compositor's preferred mode and scale, not at ours.
const VIRTUAL_MODE: &str = "1920x1080@60";
/// Logical width the pinned mode must report back.
const VIRTUAL_WIDTH: u32 = 1920;
/// Logical height the pinned mode must report back.
const VIRTUAL_HEIGHT: u32 = 1080;
/// Scale 1 keeps logical and physical pixels equal, so the default 1440x900 window fits whole.
const VIRTUAL_SCALE: f32 = 1.0;
/// Reported scale is a float; compare it with a tolerance rather than for equality.
const SCALE_EPSILON: f32 = 0.01;
/// How long the compositor has to apply a created output and its pinned mode.
const GEOMETRY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the harness window has to appear before the lane gives up waiting for it.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a moved window has to report the output it was moved to.
const PLACEMENT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a window may still be changing size before it is captured anyway.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the isolated output has to stop changing before its reference capture is taken.
const EMPTY_REFERENCE_TIMEOUT: Duration = Duration::from_secs(3);
/// Gap between compositor polls. Nothing here sleeps as synchronisation; every wait has a reason.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long `hyprctl` may take before the run is told the compositor is wedged.
const HYPRCTL_TIMEOUT: Duration = Duration::from_secs(10);
/// Logical window size the isolated lane pins when `FLEET_HARNESS_SIZE` names none.
///
/// It repeats `fleet-app`'s own harness default because both sides quote
/// `docs/TESTING-HARNESS.md` §1: the app asks the compositor for this size, and a tiling session
/// answers with whatever its layout rules say instead, so the lane has to pin what the app asked
/// for. A run whose window is a different size every time cannot hold a baseline (§6).
const DEFAULT_WINDOW_SIZE: (u32, u32) = (1440, 900);
/// Smallest logical edge accepted from `FLEET_HARNESS_SIZE`, mirroring the app's own floor.
const MIN_WINDOW_EDGE: u32 = 1;
/// Per-channel move that counts a pixel as different when asking whether Fleet is in a capture.
///
/// The same threshold `baseline` compares with: two photographs of the same unchanged output
/// differ by rasterisation noise and nothing more.
const EMPTY_CHANNEL_THRESHOLD: u8 = 8;
/// Share of the harness window's own area that must have changed for a capture to count.
///
/// A third is far below what Fleet actually paints — the window is opaque chrome over whatever
/// was there — and far above the nothing a capture of a covered output shows.
const MIN_CHANGED_WINDOW_SHARE: f64 = 1.0 / 3.0;

/// Where the harness window draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// No compositor at all: full logic, no pixels, `shot` fails.
    Headless,
    /// A dedicated Hyprland output the developer's session never shows.
    Virtual,
    /// The developer's current session and monitor, for watching a run live.
    Attach,
}

impl Lane {
    /// Picks the lane to use when nothing was asked for: `virtual` when a compositor answers.
    pub fn detect() -> Self {
        if monitors().is_ok() {
            Self::Virtual
        } else {
            Self::Headless
        }
    }

    /// The lane's name in the CLI, the scenario report and `run.jsonl`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Virtual => "virtual",
            Self::Attach => "attach",
        }
    }
}

impl std::fmt::Display for Lane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A run that is not on the lane it asked for.
///
/// The runner records this in `run.jsonl` so a screenshot taken from the developer's real screen
/// is never later mistaken for an isolated one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LaneFallback {
    /// The lane that was requested.
    pub requested: Lane,
    /// The lane the run actually used.
    pub effective: Lane,
    /// Why the requested lane was unusable.
    pub reason: String,
}

impl std::fmt::Display for LaneFallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the {} lane is unavailable, falling back to {}: {}",
            self.requested, self.effective, self.reason
        )
    }
}

/// One lane's whole lifecycle, from the child environment to the last compositor call.
pub trait LaneBackend: Send + Sync {
    /// The lane actually in use, which differs from the requested one after a fallback.
    fn lane(&self) -> Lane;
    /// Prepares the environment the Fleet child process is launched with.
    fn prepare(&mut self, command: &mut Command) -> anyhow::Result<()>;
    /// Waits for the harness window and puts it where this lane wants it. Idempotent.
    fn bind_window(&self) -> anyhow::Result<()>;
    /// Writes one PNG of what this lane shows.
    fn capture(&self, output: &Path) -> anyhow::Result<()>;
    /// Releases every compositor resource this lane created. Idempotent.
    fn teardown(&mut self) -> anyhow::Result<()>;
    /// The fallback this run took, for `run.jsonl`.
    fn fallback(&self) -> Option<LaneFallback>;
}

/// The window title harness-mode Fleet sets, and the only handle the compositor has on it.
pub fn window_title(run_id: &str) -> String {
    format!("Fleet [harness:{run_id}]")
}

/// The isolated output's name. `grim -o` takes it verbatim.
pub fn output_name(run_id: &str) -> String {
    format!("fleet-harness-{run_id}")
}

/// Builds the backend for `lane`, announcing the lane that will actually be used.
///
/// An unreachable compositor turns a requested `virtual` run into a `headless` one with a warning
/// and a recorded [`LaneFallback`], because the alternative — failing the run — leaves an agent
/// with no result at all. A requested `attach` run never changes lane silently: it fails.
pub fn backend(lane: Lane, run_id: &str) -> anyhow::Result<Box<dyn LaneBackend>> {
    validate_run_id(run_id)?;
    let backend: Box<dyn LaneBackend> = match lane {
        Lane::Headless => Box::new(HeadlessBackend { fallback: None }),
        Lane::Attach => Box::new(HyprlandBackend::attach(run_id)?),
        Lane::Virtual => match HyprlandBackend::isolated(run_id) {
            Ok(backend) => Box::new(backend),
            Err(error) => {
                let fallback = LaneFallback {
                    requested: Lane::Virtual,
                    effective: Lane::Headless,
                    reason: format!("{error:#}"),
                };
                eprintln!("warning: {fallback}");
                Box::new(HeadlessBackend {
                    fallback: Some(fallback),
                })
            }
        },
    };
    match backend.lane() {
        Lane::Headless => println!("lane: headless (no pixels)"),
        Lane::Virtual => println!("lane: virtual (isolated output {})", output_name(run_id)),
        Lane::Attach => println!("lane: attach (this session)"),
    }
    Ok(backend)
}

/// The logical window size the isolated lane pins, from `FLEET_HARNESS_SIZE` or the default.
///
/// The app parses the same variable for the size it asks the compositor for; the lane parses it
/// again for the size it holds the compositor to. A malformed value falls back to the default
/// rather than varying, which is what `docs/TESTING-HARNESS.md` §1 promises on both sides.
fn pinned_window_size() -> (u32, u32) {
    std::env::var("FLEET_HARNESS_SIZE")
        .ok()
        .and_then(|value| parse_window_size(&value))
        .unwrap_or(DEFAULT_WINDOW_SIZE)
}

/// Parses `WIDTHxHEIGHT`, rejecting anything that is not two usable logical edges.
fn parse_window_size(raw: &str) -> Option<(u32, u32)> {
    let (width, height) = raw.split_once(['x', 'X'])?;
    let width: u32 = width.trim().parse().ok()?;
    let height: u32 = height.trim().parse().ok()?;
    (width >= MIN_WINDOW_EDGE && height >= MIN_WINDOW_EDGE).then_some((width, height))
}

/// Rejects a run id that could not be an output name, a window title or a Lua string literal.
#[cfg(test)]
pub(crate) fn validate_run_id_for_test(run_id: &str) -> anyhow::Result<()> {
    validate_run_id(run_id)
}

fn validate_run_id(run_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!run_id.is_empty(), "run id is empty");
    anyhow::ensure!(
        run_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')),
        "run id {run_id:?} must be ASCII letters, digits, '-' or '_'"
    );
    Ok(())
}

/// The lane with no compositor: every layout and state path runs, nothing is drawn.
struct HeadlessBackend {
    fallback: Option<LaneFallback>,
}

impl LaneBackend for HeadlessBackend {
    fn lane(&self) -> Lane {
        Lane::Headless
    }

    fn prepare(&mut self, command: &mut Command) -> anyhow::Result<()> {
        // GPUI picks its headless backend when no compositor variable is set; ZED_HEADLESS makes
        // that explicit so an inherited DISPLAY cannot quietly put the window on a real screen.
        command.env("ZED_HEADLESS", "1");
        command.env_remove("WAYLAND_DISPLAY");
        command.env_remove("DISPLAY");
        Ok(())
    }

    fn bind_window(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn capture(&self, output: &Path) -> anyhow::Result<()> {
        // Deliberately before any file is created: a zero-byte PNG would look like evidence.
        anyhow::bail!(
            "no pixels in the headless lane; re-run with --lane virtual to capture {}",
            output.display()
        )
    }

    fn teardown(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn fallback(&self) -> Option<LaneFallback> {
        self.fallback.clone()
    }
}

/// The two compositor lanes. `virtual` owns an output it created; `attach` owns nothing.
struct HyprlandBackend {
    /// The unique title the window is found by.
    title: String,
    /// Everything a `&self` method may still change after a fallback.
    inner: Mutex<Inner>,
}

struct Inner {
    lane: Lane,
    /// Present only while this run's isolated output exists.
    output: Option<VirtualOutput>,
    bound: bool,
    fallback: Option<LaneFallback>,
}

/// The created output, and the process that removes it if this one never gets the chance.
struct VirtualOutput {
    name: String,
    watchdog: Option<Child>,
    /// A photograph of this output before the harness window reached it, kept for the run so
    /// every later capture can be asked whether anything of Fleet is in it. `None` when the
    /// reference could not be taken, which skips the check rather than failing the run.
    empty: Option<NamedTempFile>,
}

impl HyprlandBackend {
    /// Attaches to the developer's session without creating or changing anything.
    fn attach(run_id: &str) -> anyhow::Result<Self> {
        monitors().map_err(|error| error.context("the attach lane needs a running compositor"))?;
        Ok(Self {
            title: window_title(run_id),
            inner: Mutex::new(Inner {
                lane: Lane::Attach,
                output: None,
                bound: false,
                fallback: None,
            }),
        })
    }

    /// Creates this run's isolated output and pins its geometry.
    ///
    /// The watchdog is spawned before the first fallible step and the backend is built before the
    /// geometry is pinned, so every failure path from here on drops a value whose `Drop` removes
    /// the output.
    fn isolated(run_id: &str) -> anyhow::Result<Self> {
        monitors().map_err(|error| error.context("the virtual lane needs a running compositor"))?;
        let name = output_name(run_id);
        hyprctl(&["output", "create", "headless", &name])?;
        let watchdog = spawn_watchdog(&name);
        let backend = Self {
            title: window_title(run_id),
            inner: Mutex::new(Inner {
                lane: Lane::Virtual,
                output: Some(VirtualOutput {
                    name,
                    watchdog,
                    empty: None,
                }),
                bound: false,
                fallback: None,
            }),
        };
        backend.pin_geometry()?;
        Ok(backend)
    }

    /// Photographs the isolated output in the last moment before the harness window reaches it.
    ///
    /// This is the reference [`Self::verify_window_was_captured`] needs, and the moment matters
    /// twice. Earlier — at creation — the compositor has not yet drawn the session's own surfaces
    /// there, so the reference would be a blank buffer that every later capture differs from,
    /// including the ones that caught nothing but those surfaces. Later, the window is on the
    /// output and an empty capture would have to move it away again. A failure here is a warning,
    /// never a failed run: the check it feeds is a guard, and a run without it is exactly as good
    /// as every run before the guard existed.
    ///
    /// It takes no `&self` and touches no lock. `bind_window` holds the backend's mutex across
    /// the whole placement, so anything reached from there that locks again deadlocks the runner.
    fn record_empty_output(name: &str, window: &Client, monitor_id: i64) -> Option<NamedTempFile> {
        if window.monitor == monitor_id {
            eprintln!(
                "warning: no empty-output reference for {name}: the window mapped there already, \
                 so a capture of it would not be empty"
            );
            return None;
        }
        let file = match NamedTempFile::with_suffix(".png") {
            Ok(file) => file,
            Err(error) => {
                eprintln!("warning: no empty-output reference for {name}: {error}");
                return None;
            }
        };
        // Taken twice and required to agree. A created output is not painted the instant it
        // exists: capture it too early and the reference is a blank buffer that *every* later
        // capture differs from, which turns the guard off exactly where it matters — a run on a
        // locked session then files a photograph of the lock screen as evidence and passes. Two
        // agreeing captures are the cheap proof that whatever the session paints there has
        // settled. Failing to settle is a skipped check with a warning, never a failed run.
        let probe = match NamedTempFile::with_suffix(".png") {
            Ok(probe) => probe,
            Err(error) => {
                eprintln!("warning: no empty-output reference for {name}: {error}");
                return None;
            }
        };
        let output = capture::OutputCapture::new(name.to_owned());
        let deadline = Instant::now() + EMPTY_REFERENCE_TIMEOUT;
        loop {
            if let Err(error) = output.capture(file.path()) {
                eprintln!("warning: no empty-output reference for {name}: {error:#}");
                return None;
            }
            sleep(POLL_INTERVAL);
            if let Err(error) = output.capture(probe.path()) {
                eprintln!("warning: no empty-output reference for {name}: {error:#}");
                return None;
            }
            match baseline::differing_share(file.path(), probe.path(), EMPTY_CHANNEL_THRESHOLD) {
                Ok(0.0) => return Some(probe),
                Ok(share) if Instant::now() >= deadline => {
                    eprintln!(
                        "warning: no empty-output reference for {name}: the output is still \
                         changing ({:.1}% between two captures {POLL_INTERVAL:?} apart), so a \
                         capture cannot be checked against it",
                        share * 100.0
                    );
                    return None;
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("warning: no empty-output reference for {name}: {error:#}");
                    return None;
                }
            }
        }
    }

    /// The window's rectangle, once the compositor has stopped moving it.
    ///
    /// The attach lane photographs a *region*, so the rectangle it is given is the picture. A
    /// window the compositor is still resizing reports the size it is heading for while it has
    /// painted only part of it, and the capture comes back part window, part blank — which is
    /// how a shot of half a white rectangle passes as evidence. Two consecutive agreeing reads
    /// is the cheapest available proof that the resize has landed; the deadline is short, and
    /// on expiry the last reading is used rather than failing a run over a moving window.
    fn settled_window_rect(&self) -> anyhow::Result<capture::Rect> {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let mut previous = capture::window_rect(&self.title)?;
        loop {
            sleep(POLL_INTERVAL);
            let current = capture::window_rect(&self.title)?;
            if current == previous {
                return Ok(current);
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "warning: the window titled {:?} was still being resized when it was \
                     captured; the picture may be part blank",
                    self.title
                );
                return Ok(current);
            }
            previous = current;
        }
    }

    /// Where the harness window sits inside the captured output, in output-local pixels.
    ///
    /// Read from the compositor rather than from `FLEET_HARNESS_SIZE`: a scenario may `resize`
    /// the window, and a guard measuring against the pinned size would reject a perfectly good
    /// capture of a deliberately smaller one. `None` when the window or its monitor cannot be
    /// found, which leaves the guard comparing the whole output as it used to.
    fn captured_window_region(&self) -> Option<baseline::Region> {
        let rect = capture::window_rect(&self.title).ok()?;
        if rect.width == 0 || rect.height == 0 {
            return None;
        }
        // `hyprctl` reports both in global compositor coordinates; `grim -o` writes an image
        // whose origin is the monitor's, so the difference is the window's offset in the file.
        let origin = monitors()
            .ok()?
            .into_iter()
            .find(|monitor| Some(&monitor.name) == self.output_name().ok().flatten().as_ref())
            .map(|monitor| (monitor.x, monitor.y))?;
        Some(baseline::Region {
            x: rect.x.saturating_sub(origin.0).max(0).unsigned_abs(),
            y: rect.y.saturating_sub(origin.1).max(0).unsigned_abs(),
            width: rect.width,
            height: rect.height,
        })
    }

    /// Fails a capture that photographed the output but not the harness window.
    ///
    /// `grim -o` photographs the *output*, so anything the session paints over it — a lock
    /// surface, a shell's background and bar layers — is what lands in the file while the run's
    /// structured assertions carry on passing (`docs/TESTING-HARNESS.md` §4). A green run of
    /// screenshots with no Fleet in them is worse than a failed one, so the lane compares each
    /// capture against the output as it was before the window arrived: Fleet covers a large,
    /// opaque share of the output, and a capture that changed almost nothing did not catch it.
    /// The rejected capture is deleted, because a picture the lane has just called unusable must
    /// not stay on disk for a report to inline as if it were evidence — the same rule the
    /// headless lane keeps by refusing to write a file at all.
    fn verify_window_was_captured(&self, shot: &Path, empty: &Path) -> anyhow::Result<()> {
        // Inside the window's own rectangle, not across the whole output. A locked session
        // animates: two captures of one lock screen taken seconds apart differ by more than a
        // 1440×900 window's share of a 1920×1080 output, so an output-wide comparison let a
        // picture with no Fleet in it through — observed on 7 of 41 scenarios. Within the
        // window's rectangle the same two captures differ by nothing, and a window that really
        // is there differs from the background under it almost everywhere.
        let region = self.captured_window_region();
        let share = baseline::differing_share_within(shot, empty, EMPTY_CHANNEL_THRESHOLD, region)?;
        let (width, height, required, scope) = match region {
            Some(region) => (
                region.width,
                region.height,
                MIN_CHANGED_WINDOW_SHARE,
                "the rectangle the window occupies",
            ),
            // The window could not be located, so the old output-wide comparison is all there is.
            None => {
                let (width, height) = pinned_window_size();
                let window = f64::from(width) * f64::from(height);
                let output = f64::from(VIRTUAL_WIDTH) * f64::from(VIRTUAL_HEIGHT);
                (
                    width,
                    height,
                    (window / output) * MIN_CHANGED_WINDOW_SHARE,
                    "the whole output",
                )
            }
        };
        if share >= required {
            return Ok(());
        }
        if let Err(error) = std::fs::remove_file(shot) {
            eprintln!(
                "warning: could not remove the rejected capture {}: {error}",
                shot.display()
            );
        }
        anyhow::bail!(
            "the isolated output was photographed but the harness window is not in it: across \
             {scope}, only {:.1}% differs from the same output before the window arrived, and a \
             {width}×{height} window has to change at least {:.1}%. Something is painting over \
             the output — a session lock, or a shell that attaches its layers to every new one. \
             The capture was discarded rather than left at {}.",
            share * 100.0,
            required * 100.0,
            shot.display(),
        )
    }

    /// Locks the mutable half of the backend.
    fn inner(&self) -> anyhow::Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("lane state was poisoned by an earlier panic"))
    }

    /// The output's name while this run still owns one.
    fn output_name(&self) -> anyhow::Result<Option<String>> {
        Ok(self
            .inner()?
            .output
            .as_ref()
            .map(|output| output.name.clone()))
    }

    /// Pins the created output's mode and scale, then asserts the compositor applied them.
    ///
    /// `hyprctl keyword monitor …` is refused by Hyprland's non-legacy config parser, so the rule
    /// goes through the Lua config API; and because a rule that is accepted is not a rule that
    /// took effect, the result is read back from `hyprctl monitors -j` rather than assumed.
    fn pin_geometry(&self) -> anyhow::Result<()> {
        let Some(name) = self.output_name()? else {
            return Ok(());
        };
        hyprctl(&[
            "eval",
            &format!(
                "hl.monitor({{ output = \"{name}\", mode = \"{VIRTUAL_MODE}\", \
                 position = \"auto\", scale = {VIRTUAL_SCALE} }})"
            ),
        ])?;
        let deadline = Instant::now() + GEOMETRY_TIMEOUT;
        loop {
            let monitor = monitors()?.into_iter().find(|monitor| monitor.name == name);
            if let Some(monitor) = &monitor
                && monitor.width == VIRTUAL_WIDTH
                && monitor.height == VIRTUAL_HEIGHT
                && (monitor.scale - VIRTUAL_SCALE).abs() < SCALE_EPSILON
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "output {name} did not take {VIRTUAL_MODE} at scale {VIRTUAL_SCALE}; it reports {}",
                    match monitor {
                        Some(monitor) => format!(
                            "{}x{} at scale {}",
                            monitor.width, monitor.height, monitor.scale
                        ),
                        None => "no such output".to_owned(),
                    }
                );
            }
            sleep(POLL_INTERVAL);
        }
    }

    /// Moves the window onto the isolated output, confirms it landed there at the pinned size,
    /// and hands back the photograph of the output taken just before it arrived.
    ///
    /// The caller turns a failure here into the `attach` fallback: a window that will not leave
    /// the developer's screen is a reason to warn and carry on, not to lose the run.
    fn place_on_isolated_output(
        &self,
        name: &str,
        window: &Client,
    ) -> anyhow::Result<Option<NamedTempFile>> {
        let monitor = monitors()?
            .into_iter()
            .find(|monitor| monitor.name == name)
            .ok_or_else(|| anyhow::anyhow!("output {name} disappeared before the window moved"))?;
        let empty = Self::record_empty_output(name, window, monitor.id);
        let address = &window.address;
        // These are the classic `hyprctl dispatch` verbs, not the `hl.dispatch(hl.dsp.…)` Lua
        // form. `hyprctl eval` answers "eval is only supported with the lua config manager" on a
        // Hyprland driven by a classic `.conf` — and it answers it on *stdout with exit status
        // zero*, so every placement silently did nothing and the window stayed where it was. See
        // `hyprctl_dispatch`, which is why that can no longer pass unnoticed.
        //
        // Tiling would size the window from the developer's gaps and reserved areas, so the
        // window is floated at the size the app asked for and pinned to the output's origin.
        hyprctl_dispatch(&[
            "movetoworkspacesilent",
            &format!("{},address:{address}", monitor.active_workspace.id),
        ])?;
        hyprctl_dispatch(&["setfloating", &format!("address:{address}")])?;
        // Floating restores the size the window had before the session's layout rules resized it,
        // which is the size that window last floated at rather than the one the harness asked
        // for. Pin it explicitly, or the same scenario is captured at a different size on every
        // box and no baseline survives (`docs/TESTING-HARNESS.md` §1 and §6).
        let (width, height) = pinned_window_size();
        hyprctl_dispatch(&[
            "resizewindowpixel",
            &format!("exact {width} {height},address:{address}"),
        ])?;
        hyprctl_dispatch(&[
            "movewindowpixel",
            &format!("exact {} {},address:{address}", monitor.x, monitor.y),
        ])?;
        let deadline = Instant::now() + PLACEMENT_TIMEOUT;
        loop {
            let placed = clients()?
                .into_iter()
                .find(|client| client.address == window.address);
            if let Some(placed) = &placed
                && placed.monitor == monitor.id
                && placed.size == [width.cast_signed(), height.cast_signed()]
            {
                return Ok(empty);
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "the window would not settle at {width}×{height} on output {name}; the \
                     compositor reports {}",
                    match &placed {
                        Some(placed) => format!(
                            "{}×{} on monitor {}",
                            placed.size[0], placed.size[1], placed.monitor
                        ),
                        None => "no such window".to_owned(),
                    }
                );
            }
            sleep(POLL_INTERVAL);
        }
    }

    /// Gives up the isolated output and finishes the run on the developer's screen.
    fn fall_back_to_attach(&self, inner: &mut Inner, reason: String) {
        if let Err(error) = inner.remove_output() {
            eprintln!("warning: could not remove the isolated output: {error:#}");
        }
        let fallback = LaneFallback {
            requested: Lane::Virtual,
            effective: Lane::Attach,
            reason,
        };
        eprintln!("warning: {fallback}");
        eprintln!("warning: shots from here on show the real screen, not an isolated output");
        inner.lane = Lane::Attach;
        inner.fallback = Some(fallback);
    }

    /// Waits for the harness window to exist, by its unique title.
    fn wait_for_window(&self) -> anyhow::Result<Client> {
        let deadline = Instant::now() + WINDOW_TIMEOUT;
        loop {
            if let Some(client) = clients()?
                .into_iter()
                .find(|client| client.title == self.title)
            {
                return Ok(client);
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "no window titled {:?} appeared within {}s; harness-mode Fleet sets that title",
                    self.title,
                    WINDOW_TIMEOUT.as_secs()
                );
            }
            sleep(POLL_INTERVAL);
        }
    }
}

impl LaneBackend for HyprlandBackend {
    fn lane(&self) -> Lane {
        // A poisoned lock means a panic happened mid-run; claim the lane that promises least
        // rather than the isolation this run can no longer prove.
        self.inner().map_or(Lane::Attach, |inner| inner.lane)
    }

    fn prepare(&mut self, command: &mut Command) -> anyhow::Result<()> {
        // The child renders through the developer's compositor in both lanes; it is the window
        // that moves afterwards, not the session. Pass the session through explicitly so an
        // inherited ZED_HEADLESS cannot silently produce a pixel-free run on a pixel lane.
        command.env_remove("ZED_HEADLESS");
        for variable in [
            "WAYLAND_DISPLAY",
            "HYPRLAND_INSTANCE_SIGNATURE",
            "XDG_RUNTIME_DIR",
        ] {
            match std::env::var_os(variable) {
                Some(value) => {
                    command.env(variable, value);
                }
                None => anyhow::bail!(
                    "{variable} is not set, so the Fleet window cannot reach the compositor; \
                     use --lane headless outside a graphical session"
                ),
            }
        }
        Ok(())
    }

    fn bind_window(&self) -> anyhow::Result<()> {
        if self.inner()?.bound {
            return Ok(());
        }
        let window = self.wait_for_window()?;
        let name = self.output_name()?;
        let mut inner = self.inner()?;
        if let Some(name) = name {
            match self.place_on_isolated_output(&name, &window) {
                Ok(empty) => {
                    if let Some(output) = inner.output.as_mut() {
                        output.empty = empty;
                    }
                }
                Err(error) => self.fall_back_to_attach(&mut inner, format!("{error:#}")),
            }
        }
        inner.bound = true;
        Ok(())
    }

    fn capture(&self, output: &Path) -> anyhow::Result<()> {
        self.bind_window()?;
        let (lane, name, empty) = {
            let inner = self.inner()?;
            let isolated = inner.output.as_ref();
            (
                inner.lane,
                isolated.map(|output| output.name.clone()),
                isolated.and_then(|output| {
                    output
                        .empty
                        .as_ref()
                        .map(|empty| empty.path().to_path_buf())
                }),
            )
        };
        match (lane, name) {
            (Lane::Virtual, Some(name)) => {
                capture::OutputCapture::new(name).capture(output)?;
                match empty.as_deref() {
                    Some(empty) => self.verify_window_was_captured(output, empty),
                    None => Ok(()),
                }
            }
            (Lane::Attach, _) => {
                capture::RegionCapture::new(self.settled_window_rect()?).capture(output)
            }
            (lane, _) => anyhow::bail!("the {lane} lane has no compositor capture backend"),
        }
    }

    fn teardown(&mut self) -> anyhow::Result<()> {
        self.inner()?.remove_output()
    }

    fn fallback(&self) -> Option<LaneFallback> {
        self.inner().ok().and_then(|inner| inner.fallback.clone())
    }
}

impl Drop for HyprlandBackend {
    fn drop(&mut self) {
        if let Err(error) = self.teardown() {
            eprintln!("warning: leaked harness output: {error:#}");
        }
    }
}

impl Inner {
    /// Removes this run's output and stops its watchdog. Safe to call repeatedly.
    fn remove_output(&mut self) -> anyhow::Result<()> {
        let Some(mut output) = self.output.take() else {
            return Ok(());
        };
        let removal = hyprctl(&["output", "remove", &output.name]).and_then(|_| {
            let deadline = Instant::now() + GEOMETRY_TIMEOUT;
            loop {
                if !monitors()?
                    .iter()
                    .any(|monitor| monitor.name == output.name)
                {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    anyhow::bail!("output {} is still listed after removal", output.name);
                }
                sleep(POLL_INTERVAL);
            }
        });
        if let Some(watchdog) = &mut output.watchdog {
            if let Err(error) = watchdog.kill() {
                eprintln!("warning: could not stop the output watchdog: {error}");
            }
            if let Err(error) = watchdog.wait() {
                eprintln!("warning: could not reap the output watchdog: {error}");
            }
        }
        removal
    }
}

/// Starts the process that removes `name` once this runner's pid is gone.
///
/// `Drop` covers a panic and an orderly exit, but not `kill -INT` or a crash, and a leaked
/// monitor is a visible bug in the developer's session — so the guarantee is moved outside the
/// process. The watchdog ignores the signals that a Ctrl-C sends to the whole process group,
/// polls for the runner's pid, and is killed by [`Inner::remove_output`] on the happy path.
fn spawn_watchdog(name: &str) -> Option<Child> {
    let script = format!(
        "trap '' INT TERM HUP; while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; \
         hyprctl output remove {name} >/dev/null 2>&1",
        pid = std::process::id()
    );
    match Command::new("sh")
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => Some(child),
        Err(error) => {
            eprintln!(
                "warning: no output watchdog ({error}); an interrupted run may leak output {name}"
            );
            None
        }
    }
}

/// One entry of `hyprctl monitors -j`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Monitor {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale: f32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    #[serde(rename = "activeWorkspace")]
    pub(crate) active_workspace: WorkspaceRef,
}

/// The workspace reference a monitor or client carries.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WorkspaceRef {
    pub(crate) id: i64,
}

/// One entry of `hyprctl clients -j`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Client {
    pub(crate) address: String,
    pub(crate) title: String,
    /// The id of the monitor the window is on, matching [`Monitor::id`].
    pub(crate) monitor: i64,
    /// Top-left corner in logical compositor coordinates.
    pub(crate) at: [i32; 2],
    /// Logical size.
    pub(crate) size: [i32; 2],
}

/// Reads the compositor's monitor list.
pub(crate) fn monitors() -> anyhow::Result<Vec<Monitor>> {
    parse_monitors(&hyprctl(&["monitors", "-j"])?)
}

/// Reads the compositor's window list.
pub(crate) fn clients() -> anyhow::Result<Vec<Client>> {
    parse_clients(&hyprctl(&["clients", "-j"])?)
}

fn parse_monitors(json: &str) -> anyhow::Result<Vec<Monitor>> {
    let monitors: Vec<Monitor> =
        serde_json::from_str(json).map_err(|error| anyhow::anyhow!("hyprctl monitors: {error}"))?;
    anyhow::ensure!(
        !monitors.is_empty(),
        "the compositor reports no monitors at all"
    );
    Ok(monitors)
}

fn parse_clients(json: &str) -> anyhow::Result<Vec<Client>> {
    serde_json::from_str(json).map_err(|error| anyhow::anyhow!("hyprctl clients: {error}"))
}

/// Runs one `hyprctl` command under a deadline, turning a missing binary into an instruction.
pub(crate) fn hyprctl(arguments: &[&str]) -> anyhow::Result<String> {
    let output = capture::run(
        "hyprctl",
        arguments,
        "install hyprland or use --lane headless",
        HYPRCTL_TIMEOUT,
    )?;
    if !output.status.success() {
        anyhow::bail!(
            "hyprctl {} failed ({}): {}{}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs one `hyprctl dispatch` verb and insists the compositor answered `ok`.
///
/// A dispatcher that Hyprland refuses still exits zero: an unknown verb, a window that no longer
/// exists, and the `hl.dispatch(…)` Lua form on a classic-config session all come back as exit
/// status zero with the complaint on *stdout*. Checking only the exit status therefore reports a
/// successful placement for a window that never moved, and the run goes on to photograph whatever
/// was already on the screen. Reading the answer is the whole point of this wrapper.
pub(crate) fn hyprctl_dispatch(arguments: &[&str]) -> anyhow::Result<()> {
    let mut call = vec!["dispatch"];
    call.extend(arguments.iter().copied());
    let answer = hyprctl(&call)?;
    if answer.trim() != "ok" {
        anyhow::bail!(
            "hyprctl dispatch {} was refused: {}",
            arguments.join(" "),
            answer.trim(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITORS: &str = r#"[{"id":0,"name":"eDP-1","width":1920,"height":1080,"x":0,"y":0,
        "scale":1.5,"activeWorkspace":{"id":1,"name":"1"}},
        {"id":1,"name":"fleet-harness-abc","width":1920,"height":1080,"x":1280,"y":0,
        "scale":1.0,"activeWorkspace":{"id":2,"name":"2"}}]"#;
    const CLIENTS: &str = r#"[{"address":"0x55","title":"Fleet [harness:abc]","monitor":1,
        "at":[1280,0],"size":[1440,900],"workspace":{"id":2,"name":"2"}}]"#;

    #[test]
    fn compositor_json_keeps_only_the_fields_the_lanes_act_on() {
        let monitors = parse_monitors(MONITORS).expect("parse monitors");
        let isolated = &monitors[1];
        assert_eq!(isolated.name, "fleet-harness-abc");
        assert_eq!((isolated.width, isolated.height), (1920, 1080));
        assert_eq!(isolated.active_workspace.id, 2);
        assert_eq!((isolated.x, isolated.y), (1280, 0));

        let clients = parse_clients(CLIENTS).expect("parse clients");
        assert_eq!(clients[0].title, window_title("abc"));
        assert_eq!(clients[0].monitor, isolated.id);
        assert_eq!(clients[0].size, [1440, 900]);

        assert!(
            parse_monitors("[]").is_err(),
            "an empty monitor list is not a usable compositor"
        );
    }

    #[test]
    fn names_and_run_ids_stay_shell_and_lua_safe() {
        assert_eq!(output_name("20260911-1"), "fleet-harness-20260911-1");
        assert_eq!(window_title("20260911-1"), "Fleet [harness:20260911-1]");
        assert!(validate_run_id("2026-09-11_abc").is_ok());
        for rejected in ["", "a b", "a\"b", "a;b", "a/b", "héllo"] {
            assert!(
                validate_run_id(rejected).is_err(),
                "{rejected:?} must not reach an output name or a Lua string"
            );
        }
    }

    #[test]
    fn the_pinned_window_size_never_varies_by_accident() {
        assert_eq!(parse_window_size("1280x720"), Some((1280, 720)));
        assert_eq!(parse_window_size(" 800 X 600 "), Some((800, 600)));
        for rejected in [
            "",
            "1440",
            "1440x",
            "x900",
            "1440x900x1",
            "0x900",
            "-1x9",
            "wide",
        ] {
            assert_eq!(
                parse_window_size(rejected),
                None,
                "{rejected:?} must fall back to the default rather than vary the geometry"
            );
        }
    }

    #[test]
    fn the_headless_lane_refuses_to_fake_a_screenshot() {
        let mut backend = HeadlessBackend { fallback: None };
        assert_eq!(backend.lane(), Lane::Headless);
        let directory = std::env::temp_dir().join("fleet-harness-headless-shot");
        let error = backend
            .capture(&directory)
            .expect_err("the headless lane has no pixels");
        assert!(
            format!("{error}").contains("no pixels in the headless lane"),
            "unexpected message: {error}"
        );
        assert!(
            !directory.exists(),
            "a failed headless shot must not leave a file behind"
        );
        backend.teardown().expect("headless teardown is a no-op");
    }

    #[test]
    fn a_fallback_reads_as_a_warning_a_person_can_act_on() {
        let fallback = LaneFallback {
            requested: Lane::Virtual,
            effective: Lane::Headless,
            reason: "hyprctl not found".to_owned(),
        };
        assert_eq!(
            fallback.to_string(),
            "the virtual lane is unavailable, falling back to headless: hyprctl not found"
        );
        assert_eq!(
            serde_json::to_value(&fallback).expect("serialize"),
            serde_json::json!({
                "requested": "virtual",
                "effective": "headless",
                "reason": "hyprctl not found",
            })
        );
    }
}
