//! Reading, validating and segmenting a transcript document.
//!
//! The document is the frozen shape in `docs/TESTING-HARNESS.md` §5 plus two additive pieces the
//! frozen surface explicitly allows ("commands and optional data may be added"):
//!
//! - **optional top-level identity** — `session_id`, `thread_id`, `model` and `context_window` —
//!   so a scenario that opens two scripted threads can tell them apart, and so a transcript that
//!   names none is still byte-identical between runs;
//! - **an `end_turn` step**, because both wire protocols have exactly one turn terminal
//!   (Claude's `result`, Codex's `turn/completed`) and a flat step list has no other way to say
//!   where one turn stops and the next begins. A transcript that never uses it is one turn.
//!
//! Validation happens once, at load, and is deliberately strict: an approval with nothing to
//! approve, a duplicate gate id, or a step after `exit` is a transcript bug that would otherwise
//! surface as a scenario that hangs on a gate answer nobody can send.

use super::{Transcript, TranscriptStep};

/// The frozen document version.
pub(crate) const VERSION: u32 = 1;

/// The turn terminal a scripted turn settles with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settlement {
    /// Claude `result/success`; Codex `turn/completed{status:"completed"}`.
    Completed,
    /// Claude `result/error_during_execution`; Codex `turn/completed{status:"failed"}`.
    Failed,
    /// Claude an aborted result; Codex `turn/completed{status:"interrupted"}`.
    Interrupted,
}

impl Settlement {
    /// The settlement an `end_turn` names, defaulting to a clean completion.
    fn parse(status: Option<&str>) -> anyhow::Result<Self> {
        match status {
            None | Some("completed") => Ok(Self::Completed),
            Some("failed") => Ok(Self::Failed),
            Some("interrupted") => Ok(Self::Interrupted),
            Some(other) => anyhow::bail!(
                "`end_turn` status {other:?} is not one of completed, failed or interrupted"
            ),
        }
    }
}

/// One turn's worth of steps plus how it ends.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TurnScript<'a> {
    /// The steps to play, in order, excluding the `end_turn` marker itself.
    pub(crate) steps: &'a [TranscriptStep],
    /// The terminal the turn settles with, before any `error` step overrides it.
    pub(crate) settlement: Settlement,
    /// True when the transcript had no steps left for this turn.
    pub(crate) exhausted: bool,
}

/// A cursor over a transcript's steps that hands out one turn at a time.
#[derive(Debug)]
pub(crate) struct Playback<'a> {
    steps: &'a [TranscriptStep],
    cursor: usize,
}

impl<'a> Playback<'a> {
    /// A cursor at the first step.
    pub(crate) const fn new(transcript: &'a Transcript) -> Self {
        Self {
            steps: transcript.steps.as_slice(),
            cursor: 0,
        }
    }

    /// The next turn's steps, consuming its `end_turn` marker.
    ///
    /// Once the steps run out every further turn is an empty, cleanly completed one. A scenario
    /// that sends a third message to a two-turn transcript gets an immediate empty answer rather
    /// than a thread that never settles, which is the failure mode worth designing out.
    pub(crate) fn next_turn(&mut self) -> TurnScript<'a> {
        if self.cursor >= self.steps.len() {
            return TurnScript {
                steps: &self.steps[self.steps.len()..],
                settlement: Settlement::Completed,
                exhausted: true,
            };
        }
        let start = self.cursor;
        let mut end = self.steps.len();
        let mut settlement = Settlement::Completed;
        for (offset, step) in self.steps[start..].iter().enumerate() {
            if let TranscriptStep::EndTurn { status } = step {
                end = start + offset;
                settlement = Settlement::parse(status.as_deref()).unwrap_or(Settlement::Completed);
                break;
            }
        }
        self.cursor = (end + 1).min(self.steps.len());
        TurnScript {
            steps: &self.steps[start..end],
            settlement,
            exhausted: false,
        }
    }
}

/// The model catalogue a `models` step configures.
///
/// `models` is not a stream event on either harness: Claude answers a `list_models` control
/// request and Codex answers `model/list`, both on demand. So the step is hoisted out of the
/// step list at load and becomes the answer those calls give, and playing one emits nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Catalogue {
    /// Model ids, in the order the transcript listed them. The first is the default.
    pub(crate) models: Vec<String>,
    /// Reasoning efforts every listed model supports.
    pub(crate) efforts: Vec<String>,
}

impl Catalogue {
    /// The catalogue a transcript declares, falling back to the thread's own model.
    pub(crate) fn of(transcript: &Transcript) -> Self {
        let mut models = Vec::new();
        let mut efforts = Vec::new();
        for step in &transcript.steps {
            if let TranscriptStep::Models {
                models: listed,
                reasoning_efforts,
            } = step
            {
                models.clone_from(listed);
                efforts.clone_from(reasoning_efforts);
            }
        }
        if models.is_empty() {
            models.push(transcript.model.clone());
        }
        if efforts.is_empty() {
            efforts = ["low", "medium", "high"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect();
        }
        Self { models, efforts }
    }

    /// The default model: the first the transcript listed.
    pub(crate) fn default_model(&self) -> &str {
        self.models.first().map_or("scripted-model", String::as_str)
    }

    /// The default reasoning effort: the last the transcript listed, which is the strongest in
    /// both vendors' own orderings.
    pub(crate) fn default_effort(&self) -> &str {
        self.efforts.last().map_or("medium", String::as_str)
    }
}

/// Rejects a document this player cannot faithfully play.
pub(crate) fn validate(transcript: &Transcript) -> anyhow::Result<()> {
    if transcript.version != VERSION {
        anyhow::bail!(
            "this build speaks transcript version {VERSION}; the document declares version {}",
            transcript.version
        );
    }
    let mut gates: Vec<&str> = Vec::new();
    let mut exited = false;
    for (index, step) in transcript.steps.iter().enumerate() {
        if exited {
            anyhow::bail!("step {index} comes after `exit` and can never be played");
        }
        match step {
            TranscriptStep::Text { pace_ms, .. } if *pace_ms > MAX_PACE_MS => anyhow::bail!(
                "step {index} paces text at {pace_ms} ms, which is longer than a scenario's \
                 default await; keep a scripted delta under {MAX_PACE_MS} ms"
            ),
            TranscriptStep::FileChange { diff, .. } if diff.trim().is_empty() => {
                anyhow::bail!("step {index} is a file change with an empty diff");
            }
            // The approval carries no diff of its own on either wire — Claude reads it out of the
            // `Edit` input and Codex out of the `fileChange` item — so the change it gates has to
            // be the step right before it, and the players pair them on exactly that rule.
            TranscriptStep::Approval { .. }
                if !matches!(
                    index
                        .checked_sub(1)
                        .and_then(|previous| transcript.steps.get(previous)),
                    Some(TranscriptStep::FileChange { .. })
                ) =>
            {
                anyhow::bail!(
                    "step {index} asks for an edit approval, but the step before it is not the \
                     `file_change` it would gate"
                );
            }
            TranscriptStep::EndTurn { status } => {
                Settlement::parse(status.as_deref())?;
            }
            TranscriptStep::Exit { .. } => exited = true,
            _ => {}
        }
        if let Some(gate) = gate_id(step) {
            if gates.contains(&gate) {
                anyhow::bail!("gate id {gate:?} is used twice; a client answer would be ambiguous");
            }
            gates.push(gate);
        }
    }
    Ok(())
}

/// The longest a single text delta may pace for.
///
/// A scenario's default `await` is 5 s (`docs/TESTING-HARNESS.md` §1). A transcript that paces a
/// delta longer than that can only ever be a typo, and it would present as a hung scenario.
const MAX_PACE_MS: u64 = 5_000;

/// The gate id a step introduces, if it introduces one.
const fn gate_id(step: &TranscriptStep) -> Option<&str> {
    match step {
        TranscriptStep::Permission { id, .. } | TranscriptStep::Approval { id, .. } => {
            Some(id.as_str())
        }
        _ => None,
    }
}

/// Splits a unified diff into the text before and the text after.
///
/// Claude's permission card and its tool diff are both derived from an `Edit` tool's
/// `old_string`/`new_string`, not from a patch, so a transcript's diff has to be turned back into
/// its two sides to reach that surface. Context lines belong to both sides; the "\ No newline"
/// marker belongs to neither.
pub(crate) fn diff_sides(diff: &str) -> (String, String) {
    let mut before = String::new();
    let mut after = String::new();
    for line in diff.lines() {
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@") {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'\\') => {}
            Some(b'-') => push_line(&mut before, &line[1..]),
            Some(b'+') => push_line(&mut after, &line[1..]),
            _ => {
                let body = line.strip_prefix(' ').unwrap_or(line);
                push_line(&mut before, body);
                push_line(&mut after, body);
            }
        }
    }
    (before, after)
}

fn push_line(buffer: &mut String, line: &str) {
    buffer.push_str(line);
    buffer.push('\n');
}

/// The added and removed line counts of a unified diff.
pub(crate) fn diff_counts(diff: &str) -> (u64, u64) {
    let added = diff
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let removed = diff
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    (added as u64, removed as u64)
}

/// Splits assistant prose into the chunks a streaming model would emit.
///
/// Word-sized, because that is what a real `text_delta` sequence looks like and what makes a
/// half-streamed `await` meaningful. `split_inclusive` keeps the separator, so concatenating the
/// chunks reproduces the text byte for byte — the property the snapshot's text is compared on.
pub(crate) fn text_chunks(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split_inclusive(char::is_whitespace).collect()
}
