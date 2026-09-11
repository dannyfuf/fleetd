//! Pre-spawn capability probe and its cache.
//!
//! Two facts shape this module (spec A.6.1). A probe **runs a binary**, so it is cached per
//! `(binary, home, cwd)` with a 5-minute TTL — an agent tab that re-probes on every keystroke is
//! a fork bomb with a spinner. And a probe **must not** make an API request, persist a session,
//! run hooks or start an IDE-discovery process tree, which is why the environment it uses is not
//! the environment a session uses.
//!
//! What a probe can learn is deliberately small: the parsed version and, for Claude, nothing
//! else — its real capability list arrives on `system/init` and Codex declares nothing at all.
//! Everything richer is a behaviour probe at handshake time, where it costs nothing extra.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use semver::Version;
use tokio::sync::Mutex;

use super::{HarnessConfig, HarnessError, HarnessKind, HarnessResult, process};

/// How long a probe result stands before the binary is asked again.
const TTL: Duration = Duration::from_secs(5 * 60);

/// Claude's probe budget.
///
/// 25 s, not 8 s: Bedrock initialises far slower, and an expired probe leaves the provider
/// unverified and unselectable — a worse failure than a slow tab (spec A.6.1).
const CLAUDE_PROBE_BUDGET: Duration = Duration::from_secs(25);

/// Codex's probe budget. The whole probe is bounded, not each step.
const CODEX_PROBE_BUDGET: Duration = Duration::from_secs(10);

/// The oldest Claude Code whose launch line Fleet speaks.
///
/// `--effort`, `--permission-prompts` and `--permission-prompt-tool stdio` are the flags the
/// adapter cannot work without, and they are 2.1 flags. The *capability* gate is
/// `system/init.capabilities`, never a version compare (§4.5); this floor only refuses a CLI
/// whose argv Fleet would be guessing at.
const CLAUDE_MIN: Version = Version::new(2, 1, 0);

/// What a probe learned about an installed harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probed {
    /// Which harness.
    pub kind: HarnessKind,
    /// The parsed version.
    pub version: Version,
    /// The binary's own version line, for the log.
    pub reported: String,
}

/// The probe implementation, injectable so tests never spawn a binary.
type Prober = Arc<
    dyn Fn(
            HarnessKind,
            HarnessConfig,
        ) -> Pin<Box<dyn Future<Output = HarnessResult<Probed>> + Send>>
        + Send
        + Sync,
>;

/// Cache of probe results, keyed by what can change the answer.
pub struct ProbeCache {
    entries: Mutex<HashMap<Key, Entry>>,
    prober: Prober,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    kind: HarnessKind,
    command: String,
    home: Option<PathBuf>,
}

struct Entry {
    at: Instant,
    probed: Probed,
}

impl std::fmt::Debug for ProbeCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProbeCache")
    }
}

impl Default for ProbeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeCache {
    /// A cache that probes the real binaries.
    #[must_use]
    pub fn new() -> Self {
        Self::with_prober(Arc::new(|kind, cfg| Box::pin(probe_binary(kind, cfg))))
    }

    /// A cache with an injected probe, for tests and for the mock peers.
    #[must_use]
    pub fn with_prober(prober: Prober) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            prober,
        }
    }

    /// Returns the cached probe, or runs one.
    pub async fn probe(&self, kind: HarnessKind, cfg: &HarnessConfig) -> HarnessResult<Probed> {
        let key = Key {
            kind,
            command: cfg.command.clone(),
            home: cfg.home.clone(),
        };
        {
            let entries = self.entries.lock().await;
            if let Some(entry) = entries.get(&key)
                && entry.at.elapsed() < TTL
            {
                return Ok(entry.probed.clone());
            }
        }
        let probed = (self.prober)(kind, cfg.clone()).await?;
        let mut entries = self.entries.lock().await;
        entries.insert(
            key,
            Entry {
                at: Instant::now(),
                probed: probed.clone(),
            },
        );
        Ok(probed)
    }

    /// Drops every cached result, so the next probe asks the binary again.
    pub async fn invalidate(&self) {
        self.entries.lock().await.clear();
    }
}

/// Probes an installed harness by asking it for its version.
///
/// Probe hygiene lives here: the environment strips the harness's own inherited variables and,
/// for Claude, disables IDE auto-connect. Without that the probe spawns an IDE-discovery process
/// tree every few minutes for the life of the daemon (spec A.2.2).
async fn probe_binary(kind: HarnessKind, cfg: HarnessConfig) -> HarnessResult<Probed> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let inherited = process::login_environment(&cwd).await;
    let mut overrides = BTreeMap::new();
    if kind == HarnessKind::Claude {
        overrides.insert("CLAUDE_CODE_AUTO_CONNECT_IDE".to_owned(), "0".to_owned());
        overrides.insert(
            "CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL".to_owned(),
            "1".to_owned(),
        );
    }
    if let Some(home) = &cfg.home {
        let variable = match kind {
            HarnessKind::Claude => "CLAUDE_CONFIG_DIR",
            HarnessKind::Codex => "CODEX_HOME",
        };
        overrides.insert(
            variable.to_owned(),
            process::expand_home(home).to_string_lossy().into_owned(),
        );
    }
    let environment = process::filter_environment(inherited, strip_list(kind), &overrides);
    let budget = match kind {
        HarnessKind::Claude => CLAUDE_PROBE_BUDGET,
        HarnessKind::Codex => CODEX_PROBE_BUDGET,
    };
    let reported = process::read_version(kind, &cfg.command, &cwd, &environment, budget).await?;
    let version = parse_version(&reported).ok_or_else(|| HarnessError::Unavailable {
        reason: format!(
            "{} is installed but did not report a version Fleet understands.",
            kind.display_name()
        ),
    })?;
    // Claude's real capability gate is `system/init.capabilities` (§4.5). This floor exists only
    // because the launch line itself is version-shaped: an older CLI would be handed flags it
    // does not have. Codex declares nothing and is gated behaviourally instead, so it has no
    // floor at all — refusing an install Fleet has never seen would be a guess.
    if kind == HarnessKind::Claude && version < CLAUDE_MIN {
        return Err(HarnessError::Unavailable {
            reason: format!(
                "Claude Code {version} is too old for Fleet's agent tab. Upgrade to {CLAUDE_MIN} or newer."
            ),
        });
    }
    tracing::debug!(
        target: "fleet::agents",
        harness = %kind.display_name(),
        %version,
        "probed an agent harness"
    );
    Ok(Probed {
        kind,
        version,
        reported: reported.trim().to_owned(),
    })
}

/// Variables removed from the inherited environment, per harness.
///
/// Data, not code: inherited, every one of these silently re-points or re-configures every thread
/// Fleet starts, and a list is what a test can assert.
#[must_use]
pub const fn strip_list(kind: HarnessKind) -> &'static [&'static str] {
    match kind {
        HarnessKind::Claude => &[
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_EFFORT",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_MESSAGING_SOCKET",
        ],
        // `CODEX_HOME` selects which `config.toml` Codex reads, so an inherited one silently
        // moves a thread's account and history to whatever home the daemon was started with.
        HarnessKind::Codex => &[
            "CODEX_HOME",
            "CODEX_SANDBOX",
            "CODEX_SANDBOX_NETWORK_DISABLED",
        ],
    }
}

/// Parses the semver out of a harness's own version line.
///
/// `claude --version` answers `2.1.266 (Claude Code)` and `codex --version` answers
/// `codex-cli 0.147.0`, so the first token that parses as semver is the version.
#[must_use]
pub fn parse_version(reported: &str) -> Option<Version> {
    reported.split_whitespace().find_map(|word| {
        let trimmed = word.trim_matches(|character: char| !character.is_ascii_digit());
        Version::parse(trimmed)
            .ok()
            .or_else(|| Version::parse(&format!("{trimmed}.0")).ok())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn probed(kind: HarnessKind) -> Probed {
        Probed {
            kind,
            version: Version::new(2, 1, 266),
            reported: "2.1.266 (Claude Code)".to_owned(),
        }
    }

    #[test]
    fn both_harness_version_lines_parse() {
        assert_eq!(
            parse_version("2.1.266 (Claude Code)"),
            Some(Version::new(2, 1, 266))
        );
        assert_eq!(
            parse_version("codex-cli 0.147.0"),
            Some(Version::new(0, 147, 0))
        );
        assert_eq!(parse_version("unknown"), None);
    }

    #[test]
    fn the_claude_strip_list_names_every_variable_the_capture_carried() {
        let strip = strip_list(HarnessKind::Claude);
        for key in [
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_EFFORT",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_MESSAGING_SOCKET",
        ] {
            assert!(strip.contains(&key), "{key} must not be inherited");
        }
        assert!(
            !strip.contains(&"HOME"),
            "HOME is never overridden: it relocates the macOS login keychain"
        );
    }

    #[tokio::test]
    async fn a_probe_is_run_once_per_key_and_reused_inside_the_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let cache = ProbeCache::with_prober(Arc::new(move |kind, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(Ok(probed(kind))))
        }));
        let claude = HarnessConfig {
            command: "claude".to_owned(),
            ..HarnessConfig::default()
        };
        let other = HarnessConfig {
            command: "claude-canary".to_owned(),
            ..HarnessConfig::default()
        };
        for _ in 0..3 {
            cache
                .probe(HarnessKind::Claude, &claude)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
        cache
            .probe(HarnessKind::Claude, &other)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one probe per key");

        cache.invalidate().await;
        cache
            .probe(HarnessKind::Claude, &claude)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_probe_failure_is_not_cached() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let cache = ProbeCache::with_prober(Arc::new(move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(Err(HarnessError::Unavailable {
                reason: "Codex (`codex`) was not found on PATH.".to_owned(),
            })))
        }));
        let cfg = HarnessConfig::default();
        for _ in 0..2 {
            let error = cache
                .probe(HarnessKind::Codex, &cfg)
                .await
                .err()
                .unwrap_or_else(|| panic!("the probe must fail"));
            assert!(error.to_string().contains("not found on PATH"));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "an install that appears mid-session must be picked up"
        );
    }
}
