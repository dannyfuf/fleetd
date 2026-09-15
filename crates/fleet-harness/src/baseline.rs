//! Tolerance-based screenshot baseline comparison.
//!
//! Structured dumps are the oracle; pixels are evidence. GPU rasterisation, font fallback and
//! animation make a byte-exact image policy a flake factory, so a shot is compared against its
//! committed baseline with two budgets at once: a per-channel difference a pixel may carry
//! before it counts as changed, and a share of the image that may count as changed before the
//! shot fails. One repainted caret never fails a run; a recoloured panel always does.
//!
//! Three rules keep the corpus reviewable:
//!
//! - Comparison happens only in the `virtual` lane, which pins the output, scale and window
//!   size. `headless` has no pixels and `attach` captures the developer's own session, so
//!   neither produces an image any other machine could reproduce.
//! - A missing baseline is not a failure. A new scenario runs green and records its first
//!   baseline the next time someone passes `--update-baselines`.
//! - Baselines are committed, so a rewrite shows up as a reviewable diff. That only stays true
//!   while scenarios take few, deliberate screenshots — one per scenario at the moment that
//!   matters, never one per step.
//!
//! PNG decoding is implemented here rather than pulled in as a dependency: the harness crate's
//! manifest belongs to another stage, and the comparison needs exactly one thing from a PNG
//! library — 8-bit RGBA samples out of what `grim` wrote. The decoder covers every non-interlaced
//! colour type and bit depth; the diff writer emits stored (uncompressed) deflate, which makes
//! for a large file and a small amount of code, and diff images live in the run directory where
//! nothing keeps them.

use crate::lane::Lane;
use anyhow::Context as _;
use std::path::{Path, PathBuf};

/// Per-channel difference a pixel may carry before it counts as different.
///
/// Antialiasing of the same glyph by two GPU drivers lands well inside this; a token change
/// does not, because a palette moves further than a rounding error.
const DEFAULT_CHANNEL_THRESHOLD: u8 = 8;
/// Share of an image allowed to differ before the shot fails.
///
/// 0.2% of a 1920×1080 output is about 4000 pixels — a caret, a focus ring or one relaid-out
/// label, and far less than any surface a human would notice.
const DEFAULT_DIFFERING_PIXEL_RATIO: f32 = 0.002;
/// Largest scanline payload the PNG decoder will allocate or inflate.
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
/// Largest widened RGBA pixel buffer the PNG decoder will allocate.
const MAX_RGBA_BYTES: usize = 64 * 1024 * 1024;

/// The two budgets a shot is allowed to spend against its baseline.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Tolerance {
    /// How far one channel may move before the pixel counts as different.
    pub channel_threshold: u8,
    /// The share of differing pixels the image may carry before it fails.
    pub differing_pixel_ratio: f32,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            channel_threshold: DEFAULT_CHANNEL_THRESHOLD,
            differing_pixel_ratio: DEFAULT_DIFFERING_PIXEL_RATIO,
        }
    }
}

/// The result of comparing one capture against one baseline.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Comparison {
    /// Whether the differing share stayed inside the tolerance budget.
    pub passed: bool,
    /// Pixels whose channels moved further than the per-channel threshold.
    pub differing_pixels: u64,
    /// Pixels in the image, which both files share because sizes must match.
    pub total_pixels: u64,
    /// The diff image, written only when the comparison failed.
    pub diff: Option<PathBuf>,
}

impl Comparison {
    /// The share of the image that differs, as a fraction between 0 and 1.
    pub fn ratio(&self) -> f64 {
        if self.total_pixels == 0 {
            0.0
        } else {
            self.differing_pixels as f64 / self.total_pixels as f64
        }
    }
}

/// What a shot's baseline check concluded, in the form `report.rs` renders beside the shot.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum BaselineOutcome {
    /// The lane produces no comparable pixels, so nothing was compared or recorded.
    Skipped {
        /// The lane that ran, for the line in the report that says why.
        lane: String,
    },
    /// Updating was requested outside the only lane whose captures may become baselines.
    NotRecorded {
        /// The lane that ran, for the line in the report that says why.
        lane: String,
    },
    /// No baseline exists yet. A new scenario is not a failure.
    Missing {
        /// Where `--update-baselines` would write it.
        baseline: PathBuf,
    },
    /// `--update-baselines` replaced the stored baseline with this run's capture.
    Recorded {
        /// The baseline that was written.
        baseline: PathBuf,
        /// Whether the bytes actually moved, so a rewrite of the whole corpus can say how much
        /// of it a reviewer has to look at.
        changed: bool,
    },
    /// The capture is within tolerance of its baseline.
    Matched {
        /// The baseline that was compared against.
        baseline: PathBuf,
        /// Pixels outside the per-channel threshold, inside the ratio budget.
        differing_pixels: u64,
        /// Pixels in the image.
        total_pixels: u64,
    },
    /// The capture spent more than its budget. This fails the shot.
    Differed {
        /// The baseline that was compared against.
        baseline: PathBuf,
        /// Pixels outside the per-channel threshold.
        differing_pixels: u64,
        /// Pixels in the image.
        total_pixels: u64,
        /// The diff image written beside the run's shot.
        diff: PathBuf,
    },
}

impl BaselineOutcome {
    /// Whether the run may continue past this shot. Only a blown budget says no.
    pub fn passed(&self) -> bool {
        !matches!(self, Self::Differed { .. })
    }

    /// One line for `report.md`, next to the shot it describes.
    pub fn summary(&self) -> String {
        match self {
            Self::Skipped { lane } => {
                format!("baseline: not compared in the {lane} lane")
            }
            Self::NotRecorded { lane } => format!("not recorded (lane: {lane})"),
            Self::Missing { baseline } => format!(
                "baseline: none yet at {} — record it with --update-baselines",
                baseline.display()
            ),
            Self::Recorded { baseline, changed } => format!(
                "baseline: {} {}",
                if *changed { "rewrote" } else { "unchanged" },
                baseline.display()
            ),
            Self::Matched {
                differing_pixels,
                total_pixels,
                ..
            } => format!(
                "baseline: matched ({})",
                share(*differing_pixels, *total_pixels)
            ),
            Self::Differed {
                differing_pixels,
                total_pixels,
                diff,
                ..
            } => format!(
                "baseline: differs by {} — see {}",
                share(*differing_pixels, *total_pixels),
                diff.display()
            ),
        }
    }
    /// The `baseline` journal event for the shot at `shot`, in the shape `report.rs` reads:
    /// `shot`, `passed`, `differing_pixels`, `total_pixels` and `diff`, plus the `status` and
    /// `summary` a richer rendering can use without this file changing again.
    ///
    /// An outcome that compared nothing answers `None`. A pixel count the comparison never made
    /// would otherwise reach `report.md` as a zero, and a report that says "matched 0 of 0
    /// pixels" about a scenario with no baseline is worse than one that says nothing.
    pub fn journal_event(&self, shot: &Path) -> Option<serde_json::Value> {
        let (baseline, differing_pixels, total_pixels, diff) = match self {
            // `docs/TESTING-HARNESS.md` §8 asks for every shot to be inlined "with its baseline
            // verdict", and "no baseline yet" is a verdict: a report that says nothing leaves the
            // reader unable to tell a matched shot from one nothing was compared against.
            Self::Skipped { .. } | Self::NotRecorded { .. } | Self::Missing { .. } => {
                return Some(serde_json::json!({
                    "shot": shot,
                    "baseline": self.baseline_path(),
                    "passed": true,
                    "differing_pixels": serde_json::Value::Null,
                    "total_pixels": serde_json::Value::Null,
                    "diff": serde_json::Value::Null,
                    "status": self.status(),
                    "summary": self.summary(),
                }));
            }
            Self::Recorded { baseline, changed } => {
                return Some(serde_json::json!({
                    "shot": shot,
                    "baseline": baseline,
                    "passed": true,
                    "differing_pixels": serde_json::Value::Null,
                    "total_pixels": serde_json::Value::Null,
                    "diff": serde_json::Value::Null,
                    "status": if *changed { "rewrote" } else { "unchanged" },
                    "summary": self.summary(),
                }));
            }
            Self::Matched {
                baseline,
                differing_pixels,
                total_pixels,
            } => (baseline, differing_pixels, total_pixels, None),
            Self::Differed {
                baseline,
                differing_pixels,
                total_pixels,
                diff,
            } => (baseline, differing_pixels, total_pixels, Some(diff)),
        };
        Some(serde_json::json!({
            "shot": shot,
            "baseline": baseline,
            "passed": self.passed(),
            "differing_pixels": differing_pixels,
            "total_pixels": total_pixels,
            "diff": diff,
            "status": self.status(),
            "summary": self.summary(),
        }))
    }

    /// The baseline this outcome is about, when it names one.
    fn baseline_path(&self) -> Option<&Path> {
        match self {
            Self::Skipped { .. } | Self::NotRecorded { .. } => None,
            Self::Missing { baseline }
            | Self::Recorded { baseline, .. }
            | Self::Matched { baseline, .. }
            | Self::Differed { baseline, .. } => Some(baseline),
        }
    }

    /// The one-word verdict, which the report groups shots by.
    fn status(&self) -> &'static str {
        match self {
            Self::Skipped { .. } => "skipped",
            Self::NotRecorded { .. } => "not-recorded",
            Self::Missing { .. } => "missing",
            Self::Recorded { changed: true, .. } => "rewrote",
            Self::Recorded { .. } => "unchanged",
            Self::Matched { .. } => "matched",
            Self::Differed { .. } => "differed",
        }
    }
}

/// Renders a differing count the way a reviewer reads it: pixels first, then the share.
fn share(differing: u64, total: u64) -> String {
    let percent = if total == 0 {
        0.0
    } else {
        differing as f64 * 100.0 / total as f64
    };
    format!("{differing} of {total} pixels, {percent:.3}%")
}

/// The committed baselines belonging to one scenario.
///
/// The layout is `scenarios/baselines/<lane>/<scenario>/<NNN>-<name>.png`, where `<scenario>` is
/// the scenario's path under `scenarios/` without its extension, so `scenarios/hub/help.txt`
/// keeps its baselines in `scenarios/baselines/virtual/hub/help/`. The shot's own file name is
/// reused verbatim, so a baseline can never drift away from the capture it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Baselines {
    root: PathBuf,
    scenario: String,
}

impl Baselines {
    /// Locates the baselines for the scenario file at `scenario`.
    ///
    /// The corpus root is the nearest ancestor directory named `scenarios`; a scenario kept
    /// outside the corpus — a scratch file a developer is iterating on — gets a `baselines`
    /// directory beside itself instead of writing into the committed corpus.
    pub fn for_scenario(scenario: &Path) -> Self {
        let stem = scenario
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let corpus = scenario
            .ancestors()
            .skip(1)
            .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "scenarios"));
        match corpus {
            Some(corpus) => {
                let relative = scenario
                    .strip_prefix(corpus)
                    .unwrap_or(Path::new(&stem))
                    .with_extension("");
                Self::new(corpus.join("baselines"), &relative.to_string_lossy())
            }
            None => Self::new(
                scenario
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("baselines"),
                &stem,
            ),
        }
    }

    /// Names the baseline root and scenario explicitly, for tests and for a caller that already
    /// knows both.
    pub fn new(root: impl Into<PathBuf>, scenario: &str) -> Self {
        Self {
            root: root.into(),
            scenario: sanitize(scenario),
        }
    }

    /// The directory this scenario's baselines live in for `lane`.
    pub fn directory(&self, lane: Lane) -> PathBuf {
        self.root.join(lane.as_str()).join(&self.scenario)
    }

    /// Where the baseline for the shot file at `shot` belongs.
    pub fn baseline_for(&self, lane: Lane, shot: &Path) -> anyhow::Result<PathBuf> {
        let name = shot
            .file_name()
            .with_context(|| format!("{} names no shot file", shot.display()))?;
        Ok(self.directory(lane).join(name))
    }

    /// Compares one freshly captured shot against its baseline, or records it.
    ///
    /// Comparison and recording both run only in the `virtual` lane: `headless` has no pixels,
    /// and recording the developer's own decorated session from `attach` would commit a baseline
    /// no other machine can reproduce.
    pub fn check(
        &self,
        lane: Lane,
        shot: &Path,
        update: bool,
        tolerance: Tolerance,
    ) -> anyhow::Result<BaselineOutcome> {
        if lane != Lane::Virtual {
            let lane = lane.as_str().to_owned();
            return Ok(if update {
                BaselineOutcome::NotRecorded { lane }
            } else {
                BaselineOutcome::Skipped { lane }
            });
        }
        let baseline = self.baseline_for(lane, shot)?;
        if update {
            let changed = !same_bytes(shot, &baseline)?;
            update_baseline(shot, &baseline)?;
            return Ok(BaselineOutcome::Recorded { baseline, changed });
        }
        if !baseline.exists() {
            return Ok(BaselineOutcome::Missing { baseline });
        }
        let comparison = compare(shot, &baseline, &diff_path(shot), tolerance)?;
        Ok(match comparison.diff {
            Some(diff) => BaselineOutcome::Differed {
                baseline,
                differing_pixels: comparison.differing_pixels,
                total_pixels: comparison.total_pixels,
                diff,
            },
            None => BaselineOutcome::Matched {
                baseline,
                differing_pixels: comparison.differing_pixels,
                total_pixels: comparison.total_pixels,
            },
        })
    }
}

/// Compares two PNGs under `tolerance`, writing `diff` when the comparison fails.
///
/// A size change is an error rather than a difference: nothing useful can be said about which
/// pixels moved when the window itself did, and the answer is always to re-record.
pub fn compare(
    actual: &Path,
    baseline: &Path,
    diff: &Path,
    tolerance: Tolerance,
) -> anyhow::Result<Comparison> {
    let captured = read_image(actual)?;
    let stored = read_image(baseline)?;
    anyhow::ensure!(
        captured.width == stored.width && captured.height == stored.height,
        "{} is {}×{} but its baseline {} is {}×{}; re-record with --update-baselines",
        actual.display(),
        captured.width,
        captured.height,
        baseline.display(),
        stored.width,
        stored.height
    );
    let total_pixels = u64::from(captured.width) * u64::from(captured.height);
    let differing = differing_mask(&captured, &stored, tolerance.channel_threshold);
    let differing_pixels = differing.iter().filter(|changed| **changed).count() as u64;
    let ratio = if total_pixels == 0 {
        0.0
    } else {
        differing_pixels as f64 / total_pixels as f64
    };
    let passed = ratio <= f64::from(tolerance.differing_pixel_ratio);
    let written = if passed {
        None
    } else {
        write_diff(diff, &stored, &differing)?;
        Some(diff.to_owned())
    };
    Ok(Comparison {
        passed,
        differing_pixels,
        total_pixels,
        diff: written,
    })
}

/// The share of `left` that differs from `right`, as a fraction between 0 and 1.
///
/// [`compare`] answers a baseline question — did this shot move against its golden image — and
/// writes evidence when it did. The `virtual` lane asks a different question of the same two
/// images: is the harness window in this picture at all? That needs the number and nothing else,
/// so this writes no diff and reaches no verdict.
pub fn differing_share(left: &Path, right: &Path, channel_threshold: u8) -> anyhow::Result<f64> {
    differing_share_within(left, right, channel_threshold, None)
}

/// One rectangle of an image, in pixels from the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// The share of `region` whose pixels differ by more than `channel_threshold` in any channel.
///
/// `None` compares the whole image. A region is what the lane's empty-output guard needs: two
/// captures of one output that both show the *same* session chrome differ by nothing inside the
/// window's own rectangle, which is the question being asked — "is Fleet in this picture?" —
/// rather than "did anything on this output move?", which a lock screen's own animation can
/// answer yes to. A region outside the image, or an empty one, compares nothing and answers 0.
pub fn differing_share_within(
    left: &Path,
    right: &Path,
    channel_threshold: u8,
    region: Option<Region>,
) -> anyhow::Result<f64> {
    let captured = read_image(left)?;
    let stored = read_image(right)?;
    anyhow::ensure!(
        captured.width == stored.width && captured.height == stored.height,
        "{} is {}×{} but {} is {}×{}; two captures of one output cannot differ in size",
        left.display(),
        captured.width,
        captured.height,
        right.display(),
        stored.width,
        stored.height
    );
    let mask = differing_mask(&captured, &stored, channel_threshold);
    let Some(region) = region else {
        let total_pixels = u64::from(captured.width) * u64::from(captured.height);
        if total_pixels == 0 {
            return Ok(0.0);
        }
        let differing = mask.iter().filter(|changed| **changed).count() as u64;
        return Ok(differing as f64 / total_pixels as f64);
    };
    let right_edge = region.x.saturating_add(region.width).min(captured.width);
    let bottom_edge = region.y.saturating_add(region.height).min(captured.height);
    if region.x >= right_edge || region.y >= bottom_edge {
        return Ok(0.0);
    }
    let mut differing = 0_u64;
    let mut counted = 0_u64;
    for row in region.y..bottom_edge {
        let start = (row as usize) * (captured.width as usize);
        for column in region.x..right_edge {
            counted += 1;
            if mask[start + column as usize] {
                differing += 1;
            }
        }
    }
    if counted == 0 {
        return Ok(0.0);
    }
    Ok(differing as f64 / counted as f64)
}

/// Replaces the baseline at `baseline` with the capture at `actual`.
///
/// The capture is decoded first: a baseline that is not a readable PNG would be committed once
/// and fail every run afterwards with a message about the wrong file.
pub fn update(actual: &Path, baseline: &Path) -> anyhow::Result<()> {
    update_baseline(actual, baseline)
}

/// Shared by `update` and `Baselines::check`, so both validate before they copy.
fn update_baseline(actual: &Path, baseline: &Path) -> anyhow::Result<()> {
    read_image(actual).with_context(|| format!("read the capture {}", actual.display()))?;
    if let Some(parent) = baseline.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::copy(actual, baseline).with_context(|| {
        format!(
            "record {} as the baseline {}",
            actual.display(),
            baseline.display()
        )
    })?;
    Ok(())
}

/// Whether two files hold the same bytes, treating a missing baseline as different.
fn same_bytes(actual: &Path, baseline: &Path) -> anyhow::Result<bool> {
    if !baseline.exists() {
        return Ok(false);
    }
    let captured = std::fs::read(actual).with_context(|| format!("read {}", actual.display()))?;
    let stored = std::fs::read(baseline).with_context(|| format!("read {}", baseline.display()))?;
    Ok(captured == stored)
}

/// The diff image sits beside the run's shot, named after it.
fn diff_path(shot: &Path) -> PathBuf {
    let stem = shot.file_stem().unwrap_or_default().to_string_lossy();
    shot.with_file_name(format!("{stem}-diff.png"))
}

/// Keeps a scenario path usable as one run of directory names.
fn sanitize(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = safe.trim_matches('/').replace("..", "-");
    if trimmed.is_empty() {
        "scenario".to_owned()
    } else {
        trimmed
    }
}

/// Marks every pixel whose channels moved further than `threshold`.
fn differing_mask(captured: &Image, stored: &Image, threshold: u8) -> Vec<bool> {
    captured
        .pixels
        .chunks_exact(4)
        .zip(stored.pixels.chunks_exact(4))
        .map(|(left, right)| {
            left.iter()
                .zip(right)
                .any(|(one, other)| one.abs_diff(*other) > threshold)
        })
        .collect()
}

/// Writes the evidence image: the baseline, dimmed, with every differing pixel in magenta.
fn write_diff(path: &Path, stored: &Image, differing: &[bool]) -> anyhow::Result<()> {
    let mut rgb = Vec::with_capacity(differing.len() * 3);
    for (pixel, changed) in stored.pixels.chunks_exact(4).zip(differing) {
        if *changed {
            rgb.extend_from_slice(&[255, 0, 255]);
        } else {
            // A washed-out grey keeps the layout readable without competing with the marks.
            let luminance =
                (u32::from(pixel[0]) * 30 + u32::from(pixel[1]) * 59 + u32::from(pixel[2]) * 11)
                    / 100;
            let washed = 128 + (luminance / 2) as u8;
            rgb.extend_from_slice(&[washed, washed, washed]);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, encode_rgb(stored.width, stored.height, &rgb))
        .with_context(|| format!("write the diff image {}", path.display()))
}

/// Reads and decodes one PNG, naming the file in every failure.
fn read_image(path: &Path) -> anyhow::Result<Image> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    decode(&bytes).with_context(|| format!("decode {}", path.display()))
}

/// A decoded image in 8-bit RGBA, the one shape the comparison works in.
#[derive(Debug, Clone, PartialEq)]
struct Image {
    width: u32,
    height: u32,
    /// Four bytes per pixel, row-major.
    pixels: Vec<u8>,
}

/// The eight bytes every PNG opens with.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// What `IHDR` says about the pixels.
#[derive(Debug, Clone, Copy)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    colour_type: u8,
}

impl Header {
    /// Samples per pixel for this colour type.
    fn channels(self) -> usize {
        match self.colour_type {
            0 | 3 => 1,
            4 => 2,
            2 => 3,
            _ => 4,
        }
    }
}

/// Decodes a non-interlaced PNG into 8-bit RGBA.
fn decode(bytes: &[u8]) -> anyhow::Result<Image> {
    anyhow::ensure!(
        bytes.len() > SIGNATURE.len() && bytes[..SIGNATURE.len()] == SIGNATURE,
        "not a PNG file"
    );
    let mut offset = SIGNATURE.len();
    let mut header = None;
    let mut palette = Vec::new();
    let mut transparency = Vec::new();
    let mut data = Vec::new();
    while offset + 12 <= bytes.len() {
        let length = u32::from_be_bytes(read4(bytes, offset)?) as usize;
        let end = offset
            .checked_add(12 + length)
            .context("a chunk length overflows the file")?;
        anyhow::ensure!(end <= bytes.len(), "the file ends inside a chunk");
        let kind: [u8; 4] = read4(bytes, offset + 4)?;
        let body = &bytes[offset + 8..offset + 8 + length];
        let stored = u32::from_be_bytes(read4(bytes, offset + 8 + length)?);
        anyhow::ensure!(
            crc32(&bytes[offset + 4..offset + 8 + length]) == stored,
            "chunk {} is corrupt",
            String::from_utf8_lossy(&kind)
        );
        match &kind {
            b"IHDR" => header = Some(parse_header(body)?),
            b"PLTE" => palette = body.to_vec(),
            b"tRNS" => transparency = body.to_vec(),
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        offset = end;
    }
    let header = header.context("the file carries no IHDR chunk")?;
    anyhow::ensure!(!data.is_empty(), "the file carries no image data");
    let ceiling = decoded_size(header)?;
    let pixel_capacity = rgba_capacity(header)?;
    let raw = zlib_decompress(&data, ceiling)?;
    expand(
        header,
        &raw,
        &palette,
        &transparency,
        ceiling,
        pixel_capacity,
    )
}

/// Computes the scanline payload declared by `IHDR` without narrowing or wrapping it.
fn decoded_size(header: Header) -> anyhow::Result<usize> {
    let row_bits = u64::from(header.width)
        .checked_mul(header.channels() as u64)
        .and_then(|bits| bits.checked_mul(u64::from(header.depth)))
        .context("the decoded image row size overflows u64")?;
    let bytes_per_row = row_bits.div_ceil(8);
    let expected = bytes_per_row
        .checked_add(1)
        .and_then(|stride| stride.checked_mul(u64::from(header.height)))
        .context("the decoded image data size overflows u64")?;
    anyhow::ensure!(
        expected <= MAX_DECODED_BYTES as u64,
        "the decoded image data is {expected} bytes, above the {MAX_DECODED_BYTES}-byte limit"
    );
    usize::try_from(expected).context("the decoded image data size does not fit this platform")
}

/// Computes and bounds the widened RGBA buffer before any image data is inflated.
fn rgba_capacity(header: Header) -> anyhow::Result<usize> {
    let capacity = u64::from(header.width)
        .checked_mul(u64::from(header.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .context("the decoded RGBA image capacity overflows u64")?;
    anyhow::ensure!(
        capacity <= MAX_RGBA_BYTES as u64,
        "the decoded RGBA image is {capacity} bytes, above the {MAX_RGBA_BYTES}-byte limit"
    );
    usize::try_from(capacity).context("the decoded RGBA image capacity does not fit this platform")
}

/// Reads the four bytes at `offset`, or says the file is truncated.
fn read4(bytes: &[u8], offset: usize) -> anyhow::Result<[u8; 4]> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .context("the file ends mid-chunk")
}

/// Reads `IHDR` and rejects the shapes the harness never needs to read.
fn parse_header(body: &[u8]) -> anyhow::Result<Header> {
    anyhow::ensure!(body.len() == 13, "IHDR is {} bytes, not 13", body.len());
    let header = Header {
        width: u32::from_be_bytes(read4(body, 0)?),
        height: u32::from_be_bytes(read4(body, 4)?),
        depth: body[8],
        colour_type: body[9],
    };
    anyhow::ensure!(
        header.width > 0 && header.height > 0,
        "the image is {}×{}",
        header.width,
        header.height
    );
    anyhow::ensure!(body[10] == 0, "unknown compression method {}", body[10]);
    anyhow::ensure!(body[11] == 0, "unknown filter method {}", body[11]);
    anyhow::ensure!(
        body[12] == 0,
        "interlaced PNGs are not read by the harness; save this baseline non-interlaced"
    );
    anyhow::ensure!(
        matches!(header.colour_type, 0 | 2 | 3 | 4 | 6),
        "unknown colour type {}",
        header.colour_type
    );
    let legal_depths: &[u8] = match header.colour_type {
        0 => &[1, 2, 4, 8, 16],
        3 => &[1, 2, 4, 8],
        _ => &[8, 16],
    };
    anyhow::ensure!(
        legal_depths.contains(&header.depth),
        "bit depth {} is not legal for colour type {}",
        header.depth,
        header.colour_type
    );
    Ok(header)
}

/// Unfilters the scanlines and widens every sample to 8-bit RGBA.
fn expand(
    header: Header,
    raw: &[u8],
    palette: &[u8],
    transparency: &[u8],
    ceiling: usize,
    pixel_capacity: usize,
) -> anyhow::Result<Image> {
    let channels = header.channels();
    let bits_per_pixel = channels * usize::from(header.depth);
    let bytes_per_pixel = bits_per_pixel.div_ceil(8);
    anyhow::ensure!(
        raw.len() == ceiling,
        "the image data is {} bytes, not the {ceiling} its header describes",
        raw.len()
    );
    let height =
        usize::try_from(header.height).context("the image height does not fit this platform")?;
    let row_stride = ceiling / height;
    let bytes_per_row = row_stride
        .checked_sub(1)
        .context("the decoded image row has no filter byte")?;
    let mut pixels = Vec::with_capacity(pixel_capacity);
    let mut previous = vec![0u8; bytes_per_row];
    let mut current = vec![0u8; bytes_per_row];
    for row in 0..height {
        let start = row * row_stride;
        let filter = raw[start];
        current.copy_from_slice(&raw[start + 1..start + 1 + bytes_per_row]);
        unfilter(filter, bytes_per_pixel, &mut current, &previous)?;
        for column in 0..header.width as usize {
            push_pixel(&mut pixels, header, &current, column, palette, transparency)?;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    Ok(Image {
        width: header.width,
        height: header.height,
        pixels,
    })
}

/// Reverses one scanline filter in place, as defined by the PNG specification.
fn unfilter(
    filter: u8,
    bytes_per_pixel: usize,
    current: &mut [u8],
    previous: &[u8],
) -> anyhow::Result<()> {
    match filter {
        0 => {}
        1 => {
            for index in bytes_per_pixel..current.len() {
                current[index] = current[index].wrapping_add(current[index - bytes_per_pixel]);
            }
        }
        2 => {
            for index in 0..current.len() {
                current[index] = current[index].wrapping_add(previous[index]);
            }
        }
        3 => {
            for index in 0..current.len() {
                let left = if index >= bytes_per_pixel {
                    u16::from(current[index - bytes_per_pixel])
                } else {
                    0
                };
                let above = u16::from(previous[index]);
                current[index] = current[index].wrapping_add(((left + above) / 2) as u8);
            }
        }
        4 => {
            for index in 0..current.len() {
                let (left, upper_left) = if index >= bytes_per_pixel {
                    (
                        current[index - bytes_per_pixel],
                        previous[index - bytes_per_pixel],
                    )
                } else {
                    (0, 0)
                };
                current[index] =
                    current[index].wrapping_add(paeth(left, previous[index], upper_left));
            }
        }
        other => anyhow::bail!("unknown scanline filter {other}"),
    }
    Ok(())
}

/// The PNG Paeth predictor: whichever neighbour the gradient points at.
fn paeth(left: u8, above: u8, upper_left: u8) -> u8 {
    let estimate = i32::from(left) + i32::from(above) - i32::from(upper_left);
    let to_left = (estimate - i32::from(left)).abs();
    let to_above = (estimate - i32::from(above)).abs();
    let to_corner = (estimate - i32::from(upper_left)).abs();
    if to_left <= to_above && to_left <= to_corner {
        left
    } else if to_above <= to_corner {
        above
    } else {
        upper_left
    }
}

/// Widens one pixel of an unfiltered scanline into RGBA.
fn push_pixel(
    pixels: &mut Vec<u8>,
    header: Header,
    row: &[u8],
    column: usize,
    palette: &[u8],
    transparency: &[u8],
) -> anyhow::Result<()> {
    let channels = header.channels();
    let at = |index: usize| {
        scale(
            sample(row, column * channels + index, header.depth),
            header.depth,
        )
    };
    match header.colour_type {
        0 => {
            let grey = at(0);
            pixels.extend_from_slice(&[grey, grey, grey, 255]);
        }
        2 => pixels.extend_from_slice(&[at(0), at(1), at(2), 255]),
        3 => {
            let index = sample(row, column, header.depth) as usize;
            let entry = palette
                .get(index * 3..index * 3 + 3)
                .with_context(|| format!("palette index {index} is outside the PLTE chunk"))?;
            let alpha = transparency.get(index).copied().unwrap_or(255);
            pixels.extend_from_slice(&[entry[0], entry[1], entry[2], alpha]);
        }
        4 => {
            let grey = at(0);
            pixels.extend_from_slice(&[grey, grey, grey, at(1)]);
        }
        _ => pixels.extend_from_slice(&[at(0), at(1), at(2), at(3)]),
    }
    Ok(())
}

/// Reads sample `index` out of a scanline at this bit depth, without scaling it.
fn sample(row: &[u8], index: usize, depth: u8) -> u16 {
    match depth {
        16 => {
            let high = row.get(index * 2).copied().unwrap_or(0);
            let low = row.get(index * 2 + 1).copied().unwrap_or(0);
            u16::from(high) << 8 | u16::from(low)
        }
        8 => u16::from(row.get(index).copied().unwrap_or(0)),
        _ => {
            let per_byte = 8 / usize::from(depth);
            let byte = row.get(index / per_byte).copied().unwrap_or(0);
            let shift = 8 - usize::from(depth) * (index % per_byte + 1);
            let mask = (1u16 << depth) - 1;
            (u16::from(byte) >> shift) & mask
        }
    }
}

/// Scales a raw sample to the 8 bits the comparison works in.
fn scale(value: u16, depth: u8) -> u8 {
    match depth {
        16 => (value >> 8) as u8,
        8 => value as u8,
        4 => (value * 17) as u8,
        2 => (value * 85) as u8,
        _ => (value * 255) as u8,
    }
}

/// Unwraps a zlib stream and checks both its header and its Adler-32 trailer.
fn zlib_decompress(bytes: &[u8], ceiling: usize) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(bytes.len() >= 6, "the zlib stream is truncated");
    let (compression, flags) = (bytes[0], bytes[1]);
    anyhow::ensure!(
        compression & 0x0f == 8,
        "unknown zlib compression method {}",
        compression & 0x0f
    );
    anyhow::ensure!(
        (u16::from(compression) * 256 + u16::from(flags)) % 31 == 0,
        "the zlib header is corrupt"
    );
    anyhow::ensure!(
        flags & 0x20 == 0,
        "zlib preset dictionaries are not supported"
    );
    let (output, consumed) = inflate(&bytes[2..], ceiling)?;
    let trailer = bytes
        .get(2 + consumed..2 + consumed + 4)
        .and_then(|slice| slice.try_into().ok())
        .context("the zlib stream carries no Adler-32 trailer")?;
    anyhow::ensure!(
        adler32(&output) == u32::from_be_bytes(trailer),
        "the decompressed image data fails its Adler-32 check"
    );
    Ok(output)
}

/// Bit-level reader over a deflate stream, least significant bit first.
struct Bits<'a> {
    bytes: &'a [u8],
    position: usize,
    buffer: u32,
    count: u32,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            buffer: 0,
            count: 0,
        }
    }

    /// Consumes `need` bits, refilling a byte at a time.
    fn take(&mut self, need: u32) -> anyhow::Result<u32> {
        while self.count < need {
            let byte = *self
                .bytes
                .get(self.position)
                .context("the deflate stream ends mid-symbol")?;
            self.position += 1;
            self.buffer |= u32::from(byte) << self.count;
            self.count += 8;
        }
        let value = self.buffer & ((1u32 << need) - 1);
        self.buffer >>= need;
        self.count -= need;
        Ok(value)
    }

    /// Drops the rest of the current byte, as a stored block requires.
    fn align(&mut self) -> anyhow::Result<()> {
        let partial = self.count % 8;
        self.take(partial)?;
        Ok(())
    }

    /// How many bytes of the stream have been consumed, once aligned.
    fn consumed(&self) -> usize {
        self.position - (self.count / 8) as usize
    }
}

/// A canonical Huffman code, in the counts-and-symbols form `puff` uses.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Builds the code described by one length per symbol.
    fn build(lengths: &[u8]) -> anyhow::Result<Self> {
        let mut counts = [0u16; 16];
        for &length in lengths {
            anyhow::ensure!(length < 16, "a Huffman code is {length} bits long");
            counts[usize::from(length)] += 1;
        }
        let mut left = 1i32;
        for count in counts.iter().skip(1) {
            left <<= 1;
            left -= i32::from(*count);
            anyhow::ensure!(
                left >= 0,
                "the deflate stream over-subscribes a Huffman code"
            );
        }
        let mut offsets = [0u16; 16];
        for length in 1..15 {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let slot = usize::from(offsets[usize::from(length)]);
                symbols[slot] = symbol as u16;
                offsets[usize::from(length)] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// Reads one symbol, one bit at a time.
    fn decode(&self, bits: &mut Bits<'_>) -> anyhow::Result<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for length in 1..16 {
            code |= bits.take(1)? as i32;
            let count = i32::from(self.counts[length]);
            if code - first < count {
                let slot = usize::try_from(index + (code - first))
                    .ok()
                    .and_then(|slot| self.symbols.get(slot))
                    .context("a Huffman symbol is outside its table")?;
                return Ok(*slot);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        anyhow::bail!("the deflate stream carries an invalid Huffman code")
    }
}

/// Base lengths for the literal/length symbols 257..=285.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// Extra bits for the literal/length symbols 257..=285.
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Base distances for the distance symbols 0..=29.
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
/// Extra bits for the distance symbols 0..=29.
const DISTANCE_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order `HCLEN` code lengths arrive in.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Decompresses a raw deflate stream, answering with its output and the bytes it consumed.
fn inflate(bytes: &[u8], ceiling: usize) -> anyhow::Result<(Vec<u8>, usize)> {
    let mut bits = Bits::new(bytes);
    let mut output = Vec::new();
    loop {
        let last = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => stored_block(&mut bits, &mut output, ceiling)?,
            1 => {
                let (literals, distances) = fixed_codes()?;
                compressed_block(&mut bits, &mut output, &literals, &distances, ceiling)?;
            }
            2 => {
                let (literals, distances) = dynamic_codes(&mut bits)?;
                compressed_block(&mut bits, &mut output, &literals, &distances, ceiling)?;
            }
            _ => anyhow::bail!("the deflate stream uses the reserved block type"),
        }
        if last {
            break;
        }
    }
    bits.align()?;
    Ok((output, bits.consumed()))
}

/// Copies one uncompressed block.
fn stored_block(bits: &mut Bits<'_>, output: &mut Vec<u8>, ceiling: usize) -> anyhow::Result<()> {
    bits.align()?;
    let length = bits.take(16)? as u16;
    let complement = bits.take(16)? as u16;
    anyhow::ensure!(
        length == !complement,
        "a stored deflate block fails its length check"
    );
    for _ in 0..length {
        let byte = bits.take(8)? as u8;
        push_inflated(output, byte, ceiling)?;
    }
    Ok(())
}

/// Expands one Huffman-coded block until its end-of-block symbol.
fn compressed_block(
    bits: &mut Bits<'_>,
    output: &mut Vec<u8>,
    literals: &Huffman,
    distances: &Huffman,
    ceiling: usize,
) -> anyhow::Result<()> {
    loop {
        let symbol = usize::from(literals.decode(bits)?);
        match symbol {
            0..=255 => push_inflated(output, symbol as u8, ceiling)?,
            256 => return Ok(()),
            _ => {
                let index = symbol - 257;
                anyhow::ensure!(
                    index < LENGTH_BASE.len(),
                    "length symbol {symbol} is invalid"
                );
                let length =
                    usize::from(LENGTH_BASE[index]) + bits.take(LENGTH_EXTRA[index])? as usize;
                let code = usize::from(distances.decode(bits)?);
                anyhow::ensure!(
                    code < DISTANCE_BASE.len(),
                    "distance symbol {code} is invalid"
                );
                let distance =
                    usize::from(DISTANCE_BASE[code]) + bits.take(DISTANCE_EXTRA[code])? as usize;
                anyhow::ensure!(
                    distance <= output.len(),
                    "a deflate back-reference points before the start of the stream"
                );
                let start = output.len() - distance;
                for step in 0..length {
                    let byte = output[start + step];
                    push_inflated(output, byte, ceiling)?;
                }
            }
        }
    }
}

/// Appends one inflated byte without allowing DEFLATE to outgrow the PNG header's payload.
fn push_inflated(output: &mut Vec<u8>, byte: u8, ceiling: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        output.len() < ceiling,
        "the decompressed image data exceeds its {ceiling}-byte ceiling"
    );
    output.push(byte);
    Ok(())
}

/// The two fixed codes every deflate implementation shares.
fn fixed_codes() -> anyhow::Result<(Huffman, Huffman)> {
    let mut lengths = [8u8; 288];
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    Ok((Huffman::build(&lengths)?, Huffman::build(&[5u8; 30])?))
}

/// Reads the code lengths a dynamic block carries in front of itself.
fn dynamic_codes(bits: &mut Bits<'_>) -> anyhow::Result<(Huffman, Huffman)> {
    let literal_count = bits.take(5)? as usize + 257;
    let distance_count = bits.take(5)? as usize + 1;
    let length_count = bits.take(4)? as usize + 4;
    let mut code_lengths = [0u8; 19];
    for slot in CODE_LENGTH_ORDER.iter().take(length_count) {
        code_lengths[*slot] = bits.take(3)? as u8;
    }
    let code_huffman = Huffman::build(&code_lengths)?;
    let mut lengths = vec![0u8; literal_count + distance_count];
    let mut index = 0;
    while index < lengths.len() {
        let symbol = code_huffman.decode(bits)?;
        let (value, repeat) = match symbol {
            0..=15 => (symbol as u8, 1),
            16 => {
                let previous = index
                    .checked_sub(1)
                    .map(|slot| lengths[slot])
                    .context("a dynamic block repeats a code length before defining one")?;
                (previous, 3 + bits.take(2)? as usize)
            }
            17 => (0, 3 + bits.take(3)? as usize),
            18 => (0, 11 + bits.take(7)? as usize),
            other => anyhow::bail!("code-length symbol {other} is invalid"),
        };
        anyhow::ensure!(
            index + repeat <= lengths.len(),
            "a dynamic block declares more code lengths than it has symbols"
        );
        lengths[index..index + repeat].fill(value);
        index += repeat;
    }
    Ok((
        Huffman::build(&lengths[..literal_count])?,
        Huffman::build(&lengths[literal_count..])?,
    ))
}

/// Encodes 8-bit RGB pixels as a PNG.
///
/// The image data goes out in stored deflate blocks: a diff image is written once, read once by
/// whoever is looking at the failure, and then thrown away with the run directory, so trading
/// file size for a compressor nobody has to maintain is the right way round.
fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgb.len() + height as usize);
    for row in rgb.chunks(width as usize * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8 bits per channel, colour type 2 (RGB), deflate, adaptive filtering, no interlace.
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = Vec::from(SIGNATURE);
    write_chunk(&mut out, b"IHDR", &header);
    write_chunk(&mut out, b"IDAT", &zlib_store(&raw));
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Appends one length-type-body-CRC chunk.
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let checksum = crc32(&out[start..]);
    out.extend_from_slice(&checksum.to_be_bytes());
}

/// Wraps bytes in a zlib stream of stored deflate blocks.
fn zlib_store(raw: &[u8]) -> Vec<u8> {
    // 0x78 0x01: deflate, 32 KiB window, no compression level claimed; 0x7801 is divisible by 31.
    let mut out = vec![0x78, 0x01];
    let mut blocks = raw.chunks(u16::MAX as usize).peekable();
    if raw.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        let length = block.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// The PNG chunk checksum, computed bit by bit because no table has to outlive a call.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The zlib stream checksum.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut low, mut high) = (1u32, 0u32);
    for byte in bytes {
        low = (low + u32::from(*byte)) % 65521;
        high = (high + low) % 65521;
    }
    high << 16 | low
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16×16 RGB image written by zlib at level 9, so its image data is a dynamic Huffman
    /// block and its scanlines use all five filters. Pixel (x, y) is
    /// `((x * 16 + y) % 256, (x * 7 + y * 13) % 256, ((x ^ y) * 15) % 256)`.
    const NOISE_PNG: &str = "\
     89504e470d0a1a0a0000000d494844520000001000000010080200000090916836000003194944415478da0dd0876208\
     460000d0ab3dcad9143dab6a9ed15aadb34771f6763163e5ac588943ac0427a8c43a2bb1e29058c149ac58172b66cede\
     0e356bb556adb6ef131e0000e4c90a4be646d50ae286c549fb32b46f4536b23a9f5a47443790ab9babc436fa5067733e\
     c0de0d74afb807dfe482790b8052c570f5d2a85105daa11ae9579b8faacfa63593f35b8b359df476a60ef7b3e941c607\
     fbd7a10e64288af295c2a5cb839faac2c6b558c77a3cb029194d697847b5a0875edb57ec182c8f8c702ec4df0b337f45\
     5890b11cce5f0595a9097f26a04913dea915ebdf818ee94e22fae8858354dc70b973acb013fd8570773fd2fe1d6d40a6\
     1aa4405dfa436356a3256fda1e74ee0607f4466307e2e9c3cca23176dd0467a6f9d459e262947ca0d49b580d3237a205\
     5b90b2ed78cdaeac592fd86500183814878c4633c6dbc5538d967ed73c7774b1bc1423fe88d36f1314c8d29615eac27f\
     ec496af5a7cd87a0aea3f0200142a7c099339dfaddaf5f649256d8636bd5e578fd3051bc4b96206b002f1cc8ca715a7b\
     24f96d1cee36190d9e01c7cd0572a15fb2dc6d586393379ae3dbf49524f52845be4f15205b9028122ccb87aa3a93748b\
     e9a6fb1c1bb4c089657ed66ab07403dcb815edde854fec27572d7d9cc63ea473903d447e17262a44e85f66ab96f36d8f\
     a586aff2e3d7bbc82d709901f1fbf09e23e8e4497aed3c797285ff739b811ce1aa68a4ae182d7e5d225bad744cfb219b\
     cd849d76f65eb4fc304e3801f69e836997d9f55bfce903f2f1190539a37431a52ac5cabaeb04dde40376b8a17becc443\
     66ce71bce22cda7409eebb094edde7379eb267afe9a70f047c1b638ac7d9ca098e6cf7ad778b9e07e5b0632aec8c9e7b\
     91c4dca09befb1fd4ff8e957e0e67bf8e757f4393306b9e2edf7890627fb7a075c9ba3b2d76931fc829e74fdff521afb\
     986c79c953deb1335fe0ad4ce0794efc251f02b9931c4af155524dfd53b6ad53bdafe91177c5e44772de0bb6f22ddffa\
     991cc848cfe640b7f3e21745c0d7121040eb4ba4b9aae9b6c155d3ee8eeef350053f9753de88a84f7c5506b62d3b3d98\
     879c2b8cef20f4b22cfcb732f80f816d761063878b240000000049454e44ae426082";

    /// An 8×8 image with a 6-entry palette at 4 bits per pixel, so the sub-byte sample reader is
    /// exercised. Pixel (x, y) is palette entry `(x + y) % 6`, entry `i` being
    /// `(i * 40, 255 - i * 30, i * 17)`.
    const PALETTE_PNG: &str = "\
     89504e470d0a1a0a0000000d49484452000000080000000804030000003621a3b800000012504c544500ff0028e11150\
     c32278a533a08744c869550506d706000000244944415478da636054766564103209106200329419800c130620c39501\
     c8086080cb0200618e050f07c8f14b0000000049454e44ae426082";

    /// Turns a fixture back into bytes.
    fn bytes(hexadecimal: &str) -> Vec<u8> {
        hexadecimal
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ascii"), 16)
                    .expect("hexadecimal fixture")
            })
            .collect()
    }

    /// A flat RGB image of one colour, as the encoder writes it.
    fn solid(width: u32, height: u32, colour: [u8; 3]) -> Vec<u8> {
        colour
            .iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 3)
            .collect()
    }

    /// Writes one RGB image to `path` through the encoder this module also decodes with.
    fn write(path: &Path, width: u32, height: u32, rgb: &[u8]) {
        std::fs::write(path, encode_rgb(width, height, rgb)).expect("write the image");
    }

    /// Builds a PNG with a caller-selected header and stored scanline payload.
    fn png_with_header_and_raw(
        width: u32,
        height: u32,
        depth: u8,
        colour_type: u8,
        raw: &[u8],
    ) -> Vec<u8> {
        let mut header = Vec::with_capacity(13);
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[depth, colour_type, 0, 0, 0]);
        let mut png = Vec::from(SIGNATURE);
        write_chunk(&mut png, b"IHDR", &header);
        write_chunk(&mut png, b"IDAT", &zlib_store(raw));
        write_chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn a_real_zlib_stream_decodes_to_the_pixels_that_went_into_it() {
        let image = decode(&bytes(&NOISE_PNG.replace(['\n', ' '], ""))).expect("decode the PNG");
        assert_eq!((image.width, image.height), (16, 16));
        for y in 0..16usize {
            for x in 0..16usize {
                let pixel = &image.pixels[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
                assert_eq!(
                    pixel,
                    [
                        ((x * 16 + y) % 256) as u8,
                        ((x * 7 + y * 13) % 256) as u8,
                        (((x ^ y) * 15) % 256) as u8,
                        255,
                    ],
                    "pixel ({x}, {y}) after unfiltering"
                );
            }
        }
    }

    #[test]
    fn a_hostile_ihdr_returns_a_named_size_error_instead_of_panicking() {
        let png = png_with_header_and_raw(u32::MAX, u32::MAX, 16, 6, &[0]);
        assert_eq!(
            png.len(),
            69,
            "the regression fixture keeps its minimal shape"
        );

        let error = decode(&png).expect_err("the declared scanline payload overflows u64");
        assert_eq!(
            format!("{error}"),
            "the decoded image data size overflows u64"
        );
    }

    #[test]
    fn the_decoder_ceiling_accepts_1080p_and_rejects_larger_declared_payloads() {
        let full_hd = decoded_size(Header {
            width: 1920,
            height: 1080,
            depth: 8,
            colour_type: 6,
        })
        .expect("a 1080p RGBA image is below the ceiling");
        assert_eq!(full_hd, 8_295_480);
        assert!(full_hd < MAX_DECODED_BYTES);
        assert_eq!(
            rgba_capacity(Header {
                width: 1920,
                height: 1080,
                depth: 8,
                colour_type: 6,
            })
            .expect("a 1080p RGBA buffer is below the ceiling"),
            8_294_400
        );

        let png = png_with_header_and_raw(4096, 4097, 8, 6, &[0]);
        let error = decode(&png).expect_err("the declared scanline payload is above 64 MiB");
        assert_eq!(
            format!("{error}"),
            "the decoded image data is 67129345 bytes, above the 67108864-byte limit"
        );
    }

    #[test]
    fn a_compact_one_bit_image_cannot_request_a_huge_rgba_allocation() {
        let header = Header {
            width: 134_217_728,
            height: 1,
            depth: 1,
            colour_type: 0,
        };
        assert_eq!(
            decoded_size(header).expect("the packed scanline is below 64 MiB"),
            16_777_217
        );
        let png = png_with_header_and_raw(
            header.width,
            header.height,
            header.depth,
            header.colour_type,
            &[0],
        );

        let error = decode(&png).expect_err("the widened image exceeds the RGBA ceiling");

        assert_eq!(
            error.to_string(),
            "the decoded RGBA image is 536870912 bytes, above the 67108864-byte limit"
        );
    }

    #[test]
    fn inflate_cannot_exceed_the_size_declared_by_ihdr() {
        let png = png_with_header_and_raw(1, 1, 8, 6, &[0, 1, 2, 3, 4, 5]);

        let error = decode(&png).expect_err("one RGBA scanline is exactly five bytes");
        assert_eq!(
            format!("{error}"),
            "the decompressed image data exceeds its 5-byte ceiling"
        );
    }

    #[test]
    fn a_small_png_still_decodes_within_the_ceiling() {
        let rgb = vec![1, 2, 3, 4, 5, 6];
        let image = decode(&encode_rgb(2, 1, &rgb)).expect("decode a small real PNG");

        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn a_palette_at_four_bits_a_pixel_decodes_through_its_table() {
        let image = decode(&bytes(&PALETTE_PNG.replace(['\n', ' '], ""))).expect("decode the PNG");
        assert_eq!((image.width, image.height), (8, 8));
        for y in 0..8usize {
            for x in 0..8usize {
                let entry = (x + y) % 6;
                let pixel = &image.pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
                assert_eq!(
                    pixel,
                    [
                        (entry * 40 % 256) as u8,
                        ((255 - entry * 30) % 256) as u8,
                        (entry * 17 % 256) as u8,
                        255,
                    ],
                    "pixel ({x}, {y}) through the palette"
                );
            }
        }
    }

    #[test]
    fn a_fixed_huffman_block_decodes_like_a_dynamic_one() {
        // Literals 0..=143 are eight bits wide with code `0x30 + symbol`, and the end-of-block
        // symbol is seven zero bits. A stream built by hand is the only way to reach the fixed
        // tables, because zlib picks dynamic codes for anything worth compressing.
        let raw = [0u8, 10, 20, 30, 40, 50, 60, 70, 80, 90];
        let (mut stream, mut buffer, mut count) = (Vec::new(), 0u32, 0u32);
        let mut push = |value: u32, width: u32, stream: &mut Vec<u8>| {
            buffer |= value << count;
            count += width;
            while count >= 8 {
                stream.push((buffer & 0xff) as u8);
                buffer >>= 8;
                count -= 8;
            }
        };
        push(1, 1, &mut stream);
        push(1, 2, &mut stream);
        for byte in raw {
            let code = 0x30 + u32::from(byte);
            for bit in (0..8).rev() {
                push(code >> bit & 1, 1, &mut stream);
            }
        }
        for _ in 0..7 {
            push(0, 1, &mut stream);
        }
        push(0, 7, &mut stream);

        let mut zlib = vec![0x78, 0x01];
        zlib.extend_from_slice(&stream);
        zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
        let mut png = Vec::from(SIGNATURE);
        let mut header = Vec::new();
        header.extend_from_slice(&3u32.to_be_bytes());
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]);
        write_chunk(&mut png, b"IHDR", &header);
        write_chunk(&mut png, b"IDAT", &zlib);
        write_chunk(&mut png, b"IEND", &[]);

        let image = decode(&png).expect("decode the hand-built PNG");
        assert_eq!(
            image.pixels,
            vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255]
        );
    }

    #[test]
    fn an_encoded_image_decodes_back_to_the_pixels_it_was_given() {
        let rgb: Vec<u8> = (0..300u32).map(|index| (index % 251) as u8).collect();
        let image = decode(&encode_rgb(10, 10, &rgb)).expect("decode what the encoder wrote");
        let flattened: Vec<u8> = image
            .pixels
            .chunks_exact(4)
            .flat_map(|pixel| pixel[..3].to_vec())
            .collect();
        assert_eq!(flattened, rgb);
        assert!(
            image.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255),
            "an RGB image is fully opaque"
        );
    }

    #[test]
    fn a_shift_inside_the_channel_threshold_is_not_a_difference() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (actual, baseline, diff) = (
            directory.path().join("actual.png"),
            directory.path().join("baseline.png"),
            directory.path().join("diff.png"),
        );
        write(&baseline, 20, 20, &solid(20, 20, [100, 100, 100]));
        write(&actual, 20, 20, &solid(20, 20, [108, 100, 92]));

        let comparison = compare(&actual, &baseline, &diff, Tolerance::default()).expect("compare");
        assert_eq!(comparison.differing_pixels, 0, "±8 is the whole budget");
        assert!(comparison.passed);
        assert!(!diff.exists(), "a passing comparison writes no diff image");

        let strict = Tolerance {
            channel_threshold: 2,
            ..Tolerance::default()
        };
        let comparison = compare(&actual, &baseline, &diff, strict).expect("compare");
        assert_eq!(comparison.differing_pixels, 400);
        assert!(!comparison.passed);
        assert_eq!(comparison.diff.as_deref(), Some(diff.as_path()));
        assert!(diff.is_file(), "a failing comparison leaves evidence");
    }

    #[test]
    fn a_caret_passes_and_a_recoloured_panel_fails() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (actual, baseline, diff) = (
            directory.path().join("actual.png"),
            directory.path().join("baseline.png"),
            directory.path().join("diff.png"),
        );
        write(&baseline, 100, 100, &solid(100, 100, [20, 20, 20]));

        // Ten thousand pixels, 0.2% of which is twenty: a blinking caret.
        let mut caret = solid(100, 100, [20, 20, 20]);
        for pixel in 0..20usize {
            caret[pixel * 3..pixel * 3 + 3].copy_from_slice(&[240, 240, 240]);
        }
        write(&actual, 100, 100, &caret);
        let comparison = compare(&actual, &baseline, &diff, Tolerance::default()).expect("compare");
        assert_eq!(comparison.differing_pixels, 20);
        assert!(comparison.passed, "{}", comparison.ratio());

        // One more differing pixel than the budget allows is a regression.
        let mut panel = caret;
        panel[20 * 3..21 * 3].copy_from_slice(&[240, 240, 240]);
        write(&actual, 100, 100, &panel);
        let comparison = compare(&actual, &baseline, &diff, Tolerance::default()).expect("compare");
        assert_eq!(comparison.differing_pixels, 21);
        assert!(!comparison.passed);

        let evidence = decode(&std::fs::read(&diff).expect("read the diff")).expect("decode");
        assert_eq!((evidence.width, evidence.height), (100, 100));
        assert_eq!(
            &evidence.pixels[..4],
            [255, 0, 255, 255],
            "a differing pixel is marked"
        );
        assert_eq!(
            &evidence.pixels[400..404],
            [138, 138, 138, 255],
            "an unchanged pixel is washed out, not black"
        );
    }

    #[test]
    fn a_resized_window_says_to_re_record_rather_than_reporting_a_difference() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (actual, baseline, diff) = (
            directory.path().join("actual.png"),
            directory.path().join("baseline.png"),
            directory.path().join("diff.png"),
        );
        write(&baseline, 20, 20, &solid(20, 20, [10, 10, 10]));
        write(&actual, 20, 21, &solid(20, 21, [10, 10, 10]));

        let error = compare(&actual, &baseline, &diff, Tolerance::default())
            .expect_err("the sizes disagree");
        let message = format!("{error}");
        assert!(message.contains("is 20×21"), "{message}");
        assert!(message.contains("is 20×20"), "{message}");
        assert!(message.contains("--update-baselines"), "{message}");
    }

    #[test]
    fn a_new_scenario_is_not_a_failure_and_update_baselines_records_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let shots = directory.path().join("shots");
        std::fs::create_dir_all(&shots).expect("create shots");
        let shot = shots.join("003-help.png");
        write(&shot, 8, 8, &solid(8, 8, [30, 40, 50]));
        let baselines = Baselines::new(directory.path().join("baselines"), "hub/help");

        let missing = baselines
            .check(Lane::Virtual, &shot, false, Tolerance::default())
            .expect("check");
        assert!(missing.passed(), "a scenario with no baseline runs green");
        assert!(matches!(missing, BaselineOutcome::Missing { .. }));

        let recorded = baselines
            .check(Lane::Virtual, &shot, true, Tolerance::default())
            .expect("record");
        let BaselineOutcome::Recorded { baseline, changed } = recorded else {
            panic!("--update-baselines records the shot: {recorded:?}");
        };
        assert!(changed, "the first recording is a change");
        assert_eq!(
            baseline,
            directory
                .path()
                .join("baselines/virtual/hub/help/003-help.png")
        );

        let matched = baselines
            .check(Lane::Virtual, &shot, false, Tolerance::default())
            .expect("check");
        assert!(matches!(
            matched,
            BaselineOutcome::Matched {
                differing_pixels: 0,
                ..
            }
        ));

        // A colour token moved: the comparison fails, the diff is written beside the shot, and
        // re-recording restores green.
        write(&shot, 8, 8, &solid(8, 8, [200, 40, 50]));
        let differed = baselines
            .check(Lane::Virtual, &shot, false, Tolerance::default())
            .expect("check");
        assert!(!differed.passed(), "{differed:?}");
        let BaselineOutcome::Differed {
            differing_pixels,
            diff,
            ..
        } = &differed
        else {
            panic!("a recoloured shot differs: {differed:?}");
        };
        assert_eq!(*differing_pixels, 64);
        assert_eq!(diff, &shots.join("003-help-diff.png"));
        assert!(diff.is_file());
        assert!(differed.summary().contains("64 of 64 pixels, 100.000%"));

        let rerecorded = baselines
            .check(Lane::Virtual, &shot, true, Tolerance::default())
            .expect("record");
        assert!(matches!(
            rerecorded,
            BaselineOutcome::Recorded { changed: true, .. }
        ));
        let green = baselines
            .check(Lane::Virtual, &shot, false, Tolerance::default())
            .expect("check");
        assert!(green.passed(), "{green:?}");
        assert!(matches!(green, BaselineOutcome::Matched { .. }));
    }

    /// Renders a JSON scalar as the report's own `scalar` helper would.
    fn scalar_of(value: &serde_json::Value) -> String {
        value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned)
    }

    /// The empty-output guard asks "is Fleet in this picture?", and a whole-image comparison
    /// answers the different question "did anything on this output move?" — which a locked
    /// session's own animation answers yes to, letting a lock screen through as evidence.
    #[test]
    fn a_region_comparison_ignores_what_moved_outside_the_window() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let (width, height) = (40_u32, 20_u32);
        // `before` is flat; `after` changes only the left half, which stands in for the part of
        // the output the harness window does *not* cover.
        let before = solid(width, height, [10, 10, 10]);
        let mut after = before.clone();
        for row in 0..height {
            for column in 0..width / 2 {
                let at = ((row * width + column) * 3) as usize;
                after[at..at + 3].copy_from_slice(&[250, 250, 250]);
            }
        }
        let before_path = dir.path().join("before.png");
        let after_path = dir.path().join("after.png");
        write(&before_path, width, height, &before);
        write(&after_path, width, height, &after);

        let whole = differing_share(&after_path, &before_path, 8).expect("whole");
        assert!(
            (whole - 0.5).abs() < 0.01,
            "half the output changed, so a whole-image comparison reads 50%: {whole}"
        );

        // The window is the right half: nothing inside it changed.
        let window = Region {
            x: width / 2,
            y: 0,
            width: width / 2,
            height,
        };
        let inside = differing_share_within(&after_path, &before_path, 8, Some(window))
            .expect("within the window");
        assert_eq!(
            inside, 0.0,
            "nothing inside the window changed, whatever the rest of the output did"
        );

        // And the guard still sees a window that really did paint.
        let covering = Region {
            x: 0,
            y: 0,
            width: width / 2,
            height,
        };
        let covered = differing_share_within(&after_path, &before_path, 8, Some(covering))
            .expect("over the changed half");
        assert_eq!(
            covered, 1.0,
            "a window that painted covers its whole rectangle"
        );
    }

    #[test]
    fn the_journal_event_carries_what_the_report_renders() {
        let shot = Path::new("/run/shots/003-help.png");
        // `docs/TESTING-HARNESS.md` §8 inlines every shot "with its baseline verdict", so an
        // uncompared shot is journalled too — as a verdict, never as a pixel count it does not
        // have. A silent shot is indistinguishable from one that matched.
        let uncompared = BaselineOutcome::Missing {
            baseline: PathBuf::from("scenarios/baselines/virtual/hub/help/003-help.png"),
        }
        .journal_event(shot)
        .expect("a missing baseline is still a verdict");
        assert_eq!(uncompared["status"], "missing");
        assert_eq!(uncompared["passed"], true);
        assert!(uncompared["differing_pixels"].is_null());
        assert!(
            scalar_of(&uncompared["summary"]).contains("record it with --update-baselines"),
            "the verdict says what to do about it: {uncompared}"
        );
        let skipped = BaselineOutcome::Skipped {
            lane: "headless".to_owned(),
        }
        .journal_event(shot)
        .expect("a lane that compares nothing still says so");
        assert_eq!(skipped["status"], "skipped");
        assert!(skipped["baseline"].is_null());
        let event = BaselineOutcome::Differed {
            baseline: PathBuf::from("scenarios/baselines/virtual/hub/help/003-help.png"),
            differing_pixels: 181_979,
            total_pixels: 2_073_600,
            diff: PathBuf::from("/run/shots/003-help-diff.png"),
        }
        .journal_event(shot)
        .expect("a comparison is journalled");
        assert_eq!(event["shot"], "/run/shots/003-help.png");
        assert_eq!(event["passed"], false);
        assert_eq!(event["differing_pixels"], 181_979);
        assert_eq!(event["total_pixels"], 2_073_600);
        assert_eq!(event["diff"], "/run/shots/003-help-diff.png");
        assert_eq!(event["status"], "differed");
        let matched = BaselineOutcome::Matched {
            baseline: PathBuf::from("scenarios/baselines/virtual/hub/help/003-help.png"),
            differing_pixels: 0,
            total_pixels: 2_073_600,
        }
        .journal_event(shot)
        .expect("a comparison is journalled");
        assert_eq!(matched["passed"], true);
        assert!(matched["diff"].is_null(), "a match writes no diff image");
    }

    #[test]
    fn recording_an_unchanged_baseline_says_a_reviewer_has_nothing_to_read() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let shot = directory.path().join("001-hub.png");
        write(&shot, 4, 4, &solid(4, 4, [1, 2, 3]));
        let baselines = Baselines::new(directory.path().join("baselines"), "hub/hub");
        for expected in [true, false] {
            let recorded = baselines
                .check(Lane::Virtual, &shot, true, Tolerance::default())
                .expect("record");
            assert!(
                matches!(recorded, BaselineOutcome::Recorded { changed, .. } if changed == expected),
                "{recorded:?}"
            );
        }
    }

    #[test]
    fn nothing_is_compared_or_recorded_outside_the_virtual_lane() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let shot = directory.path().join("001-hub.png");
        write(&shot, 4, 4, &solid(4, 4, [1, 2, 3]));
        let baselines = Baselines::new(directory.path().join("baselines"), "hub/hub");
        for lane in [Lane::Headless, Lane::Attach] {
            let not_recorded = baselines
                .check(lane, &shot, true, Tolerance::default())
                .expect("check");
            assert_eq!(
                not_recorded,
                BaselineOutcome::NotRecorded {
                    lane: lane.as_str().to_owned()
                }
            );
            assert!(not_recorded.passed());
            let event = not_recorded
                .journal_event(&shot)
                .expect("refusing an update is journalled");
            assert_eq!(event["status"], "not-recorded");
            assert_eq!(
                event["summary"],
                format!("not recorded (lane: {})", lane.as_str())
            );

            let skipped = baselines
                .check(lane, &shot, false, Tolerance::default())
                .expect("check");
            assert_eq!(
                skipped,
                BaselineOutcome::Skipped {
                    lane: lane.as_str().to_owned()
                }
            );
        }
        assert!(
            !directory.path().join("baselines").exists(),
            "the developer's own session never becomes a committed baseline"
        );
    }

    #[test]
    fn baselines_mirror_the_corpus_layout() {
        let baselines = Baselines::for_scenario(Path::new("scenarios/hub/help.txt"));
        assert_eq!(
            baselines.directory(Lane::Virtual),
            Path::new("scenarios/baselines/virtual/hub/help")
        );
        assert_eq!(
            Baselines::for_scenario(Path::new("/work/fleet/scenarios/daemon/down.txt"))
                .directory(Lane::Virtual),
            Path::new("/work/fleet/scenarios/baselines/virtual/daemon/down")
        );
        assert_eq!(
            Baselines::for_scenario(Path::new("/tmp/scratch.txt")).directory(Lane::Virtual),
            Path::new("/tmp/baselines/virtual/scratch"),
            "a scratch scenario never writes into the committed corpus"
        );
        assert_eq!(
            Baselines::new("scenarios/baselines", "../../etc/passwd").directory(Lane::Virtual),
            Path::new("scenarios/baselines/virtual/-/-/etc/passwd"),
            "a scenario name stays under the baseline root"
        );
    }

    #[test]
    fn a_baseline_that_is_not_a_png_is_named_in_the_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let baseline = directory.path().join("baseline.png");
        let actual = directory.path().join("actual.png");
        write(&actual, 4, 4, &solid(4, 4, [0, 0, 0]));
        std::fs::write(&baseline, b"report.md, not a screenshot").expect("write");
        let error = compare(
            &actual,
            &baseline,
            &directory.path().join("diff.png"),
            Tolerance::default(),
        )
        .expect_err("the baseline is not an image");
        let message = format!("{error:#}");
        assert!(message.contains("baseline.png"), "{message}");
        assert!(message.contains("not a PNG file"), "{message}");
    }

    #[test]
    fn a_truncated_image_is_reported_rather_than_decoded_as_garbage() {
        let mut png = encode_rgb(4, 4, &solid(4, 4, [9, 9, 9]));
        let length = png.len();
        png.truncate(length - 20);
        let error = decode(&png).expect_err("the file is cut short");
        assert!(
            format!("{error}").contains("ends"),
            "unexpected message: {error}"
        );
    }
}
