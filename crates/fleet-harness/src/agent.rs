//! Scripted Claude and Codex transcript runner.
//!
//! `fleet-harness agent --provider <claude|codex> --transcript <file>` is a stand-in for a vendor
//! CLI: it speaks Claude Code's stream-json surface or `codex app-server`'s JSON-RPC-shaped
//! surface from a transcript document, so a scenario can rehearse a whole agent conversation —
//! streaming prose, tool calls, an edit approval with a real diff, an error mid-stream — with no
//! vendor binary installed, no network and no token spent.
//!
//! **It imitates the wire protocol, not the vendor's behaviour.** Proving that Fleet reacts
//! correctly to a given sequence of protocol messages is not proving that Claude or Codex emits
//! that sequence; the opt-in `--features real-agents` tests in `fleet-daemon` remain the check on
//! reality. The frames below are transcribed from `docs/research/harness-protocols.md` and
//! `docs/research/harness-codex-app-server.md`, which are the two authoritative captures, and
//! when a real protocol changes those documents and these players move together.
//!
//! Two things the player deliberately does not do. It never reads stdin while it is playing a
//! turn except at a gate, so a mid-stream `interrupt` is noticed at the next gate or at the turn
//! terminal rather than instantly — a scripted turn is milliseconds long and a racing reader
//! would cost determinism for nothing. And it mints every id from a fixed counter, so two runs of
//! the same transcript are byte-identical, timestamps included.
//!
//! The daemon probes a harness binary with `<command> --version` before it spawns a session, and
//! then appends the vendor's own launch flags to the configured command line. Neither is
//! something a `clap` subcommand can absorb, so a fixture points `AgentKind::executable()` at the
//! `/bin/sh` launcher [`write_launcher`] writes — the same technique `fleet-daemon`'s
//! `agents::harness::mockpeer` uses, and for the same reason.

mod claude;
mod codex;
mod ids;
mod launcher;
mod peer;
mod transcript;

#[cfg(test)]
mod tests;

pub(crate) use launcher::shell_word;
pub use launcher::{launcher_script, write_launcher};

use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

/// Gate waits are sized against `DEFAULT_AWAIT_TIMEOUT_MS`, the scenario await default.
pub(crate) const GATE_BUDGET: Duration = Duration::from_secs(5);

/// Which native agent protocol a scripted run imitates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Claude Code's stream-json over stdio.
    Claude,
    /// `codex app-server`'s JSON-RPC-shaped surface over stdio.
    Codex,
}

impl Provider {
    /// The lower-case name a launcher and a config use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The line this provider's real binary answers `--version` with.
    ///
    /// `fleet-daemon`'s pre-spawn probe runs `<command> --version`, requires the output to
    /// **name the harness** (`Claude Code`, `codex`), parses the first token that looks like
    /// semver (`agents::harness::probe::parse_version`), and refuses a Claude older than 2.1
    /// because the launch line itself is version-shaped. So these lines are copied verbatim from
    /// the real binaries — a paraphrase fails the identity check — and they are the versions the
    /// two research captures were taken against.
    #[must_use]
    pub const fn version_line(self) -> &'static str {
        match self {
            Self::Claude => "2.1.266 (Claude Code)",
            Self::Codex => "codex-cli 0.147.0",
        }
    }
}

/// One scripted step.
///
/// The eight frozen variants of `docs/TESTING-HARNESS.md` §5, plus the additive `shell` and
/// `end_turn` steps. `end_turn` is how a flat step list says where one turn stops and the next
/// begins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptStep {
    /// Assistant prose, streamed one word-sized delta at a time.
    Text {
        /// The whole message. Concatenated deltas reproduce it byte for byte.
        text: String,
        /// Milliseconds between two deltas. Zero is immediate.
        #[serde(default)]
        pace_ms: u64,
    },
    /// A tool call and its result.
    ToolCall {
        /// The provider-side tool call id, also the gate-free correlation key.
        id: String,
        /// Claude's structured tool name; Codex renders the same call as a shell command.
        name: String,
        /// The tool input.
        arguments: serde_json::Value,
        /// The tool's output, as the transcript wants it rendered.
        #[serde(default)]
        output: String,
    },
    /// A real shell command run by the scripted player in its inherited environment.
    Shell {
        /// The string passed as the second argument to `sh -c`.
        command: String,
        /// The tool name presented to Fleet.
        #[serde(default = "default_shell_name")]
        name: String,
    },
    /// A file the agent changed, carrying the real unified diff.
    FileChange {
        /// The path, as the agent names it.
        path: String,
        /// A unified diff. An `approval` after it gates this change.
        diff: String,
    },
    /// A command the agent asks permission to run.
    Permission {
        /// The gate id. Unique across the transcript.
        id: String,
        /// The command under review; it is what the card shows literally.
        command: String,
    },
    /// An approval for the file change that precedes it in this turn.
    Approval {
        /// The gate id. Unique across the transcript.
        id: String,
        /// Why the agent is asking.
        summary: String,
    },
    /// The model catalogue and reasoning-effort ladder this session advertises.
    Models {
        /// Model ids, strongest listed however the transcript likes. The first is the default.
        models: Vec<String>,
        /// The efforts every listed model supports.
        reasoning_efforts: Vec<String>,
    },
    /// A provider error, mid-stream. It settles the turn as failed.
    Error {
        /// The message the provider reports.
        message: String,
    },
    /// The turn terminal.
    EndTurn {
        /// `completed`, `failed` or `interrupted`; absent means completed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    /// The process exits with this code, whatever is left in the transcript.
    Exit {
        /// The exit status.
        code: i32,
    },
}

/// A whole scripted conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// The document version. Currently `1`.
    pub version: u32,
    /// The steps, in order.
    pub steps: Vec<TranscriptStep>,
    /// The durable session id Claude reports, which becomes Fleet's resume cursor.
    #[serde(default = "default_session_id")]
    pub session_id: String,
    /// The Codex thread id, which becomes Fleet's resume cursor there.
    #[serde(default = "default_thread_id")]
    pub thread_id: String,
    /// The model this session reports running.
    #[serde(default = "default_model")]
    pub model: String,
    /// The context window the token meter divides by.
    #[serde(default = "default_context_window")]
    pub context_window: i64,
}

/// The default Claude session id: a fixed UUIDv4, because a random one would make two runs of
/// one transcript differ in the snapshot.
fn default_session_id() -> String {
    "6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e".to_owned()
}

/// The default Codex thread id.
fn default_thread_id() -> String {
    "01999c4a-7f00-7000-8000-0000000000a1".to_owned()
}

fn default_model() -> String {
    "scripted-model".to_owned()
}

fn default_shell_name() -> String {
    "Bash".to_owned()
}

const fn default_context_window() -> i64 {
    200_000
}

/// Reads and validates one transcript document.
///
/// # Errors
///
/// Fails when the file cannot be read, is not JSON, declares a version this build does not speak,
/// or describes a conversation the players cannot faithfully play — an approval with nothing to
/// approve, a duplicate gate id, or a step after `exit`.
pub async fn load(path: &Path) -> anyhow::Result<Transcript> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| anyhow::anyhow!("read the transcript {}: {error}", path.display()))?;
    let transcript: Transcript = serde_json::from_str(&raw)
        .map_err(|error| anyhow::anyhow!("parse the transcript {}: {error}", path.display()))?;
    transcript::validate(&transcript)
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    Ok(transcript)
}

/// Plays `path` as `provider`, speaking that provider's wire protocol on stdin and stdout.
///
/// # Errors
///
/// Fails when the transcript cannot be loaded, or when the client's half of the conversation
/// breaks — a closed pipe mid-turn, or a gate the client does not answer or withdraw within
/// [`GATE_BUDGET`].
pub async fn run_transcript(provider: Provider, path: &Path) -> anyhow::Result<()> {
    let transcript = load(path).await?;
    let code = tokio::task::spawn_blocking(move || {
        let mut peer = peer::Peer::new(
            std::io::BufReader::new(std::io::stdin()),
            std::io::stdout().lock(),
            peer::Pace::Real,
        );
        play(provider, &transcript, &mut peer)
    })
    .await
    .map_err(|error| anyhow::anyhow!("the scripted agent thread did not finish: {error}"))??;
    if code != 0 {
        // The transcript's own `exit` step is the whole point of the variant: a scenario that
        // wants to photograph Fleet's "the harness died" surface needs the status the real CLI
        // would have exited with, and `main` can only ever return 0 or 1.
        std::process::exit(code);
    }
    Ok(())
}

/// Runs one scripted conversation over an already-built peer, returning the exit status.
fn play<R: std::io::BufRead + Send + 'static, W: std::io::Write>(
    provider: Provider,
    transcript: &Transcript,
    peer: &mut peer::Peer<R, W>,
) -> anyhow::Result<i32> {
    match provider {
        Provider::Claude => claude::play(transcript, peer),
        Provider::Codex => codex::play(transcript, peer),
    }
}
