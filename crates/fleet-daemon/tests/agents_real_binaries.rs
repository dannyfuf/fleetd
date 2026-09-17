//! Handshake tests against the **real** harness binaries.
//!
//! Opt-in: `cargo test -p fleet-daemon --features real-agents --test agents_real_binaries`.
//! The default `make test` run spawns neither `claude` nor `codex` — every other agent test
//! drives a scripted `/bin/sh` mock peer — because a test suite that needs two vendor CLIs
//! installed is a test suite that silently skips on the machine that most needs it.
//!
//! Nothing here sends a prompt, so nothing here costs a token: the handshake is the whole point.
#![cfg(feature = "real-agents")]

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use fleet_core::{
    agents::{
        AccountStatus, AgentEvent, AgentKind, ApprovalPolicy, PermissionMode, SandboxPolicy,
        StartRequest, ThreadId,
    },
    config::AgentBinaries,
};
use fleet_daemon::agents::harness::{self, OpenSession, ShutdownReason, probe::ProbeCache};

/// The `config.agentBinaries` section this suite runs against.
///
/// The packaged default, so the opt-in run behaves exactly as it always did. It is a value
/// rather than a constant because the thing being exercised *is* the configured entry: point it
/// at a wrapper here and every test below follows, which is the only way this suite stays honest
/// about the path a real user's config takes.
fn binaries() -> AgentBinaries {
    AgentBinaries::default()
}

fn config(kind: AgentKind) -> harness::HarnessConfig {
    config_for(&binaries(), kind)
}

fn config_for(binaries: &AgentBinaries, kind: AgentKind) -> harness::HarnessConfig {
    harness::HarnessConfig {
        command: binaries.binary(kind).to_owned(),
        home: None,
        env: BTreeMap::new(),
        attachments_dir: None,
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
        raw_log_dir: None,
    }
}

fn request(kind: AgentKind, worktree: PathBuf) -> StartRequest {
    StartRequest {
        thread: ThreadId::new(),
        worktree_path: worktree,
        provider: kind,
        model: None,
        mode: PermissionMode::Ask,
        resume_cursor: None,
        fork: false,
        env: BTreeMap::new(),
        sandbox: SandboxPolicy::ReadOnly,
        approval_policy: ApprovalPolicy::Untrusted,
        permission_profile: None,
        title: None,
    }
}

/// The installed binary reports a version Fleet accepts, and the probe is cached.
#[tokio::test]
async fn both_installed_harnesses_probe() {
    let binaries = binaries();
    let probes = ProbeCache::new();
    for kind in [AgentKind::Claude, AgentKind::Codex] {
        let probed = probes
            .probe(kind, &config_for(&binaries, kind))
            .await
            .unwrap_or_else(|error| panic!("{kind:?} must probe: {error}"));
        assert_eq!(probed.kind, kind);
        assert!(
            probed.version.major > 0 || probed.version.minor > 0,
            "{probed:?}"
        );
        // The identity check is not a formality against the real binary either: its own
        // `--version` line is what has to name the product.
        let expected = match kind {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "codex",
        };
        assert!(
            probed
                .reported
                .to_ascii_lowercase()
                .contains(&expected.to_ascii_lowercase()),
            "{probed:?}"
        );
    }
}

/// A configured binary that is not the harness is refused, however well its version parses.
#[tokio::test]
async fn a_configured_binary_that_is_not_the_harness_is_refused() {
    let binaries = AgentBinaries {
        claude: "/bin/echo".to_owned(),
        ..binaries()
    };
    let reason = ProbeCache::new()
        .probe(AgentKind::Claude, &config_for(&binaries, AgentKind::Claude))
        .await
        .err()
        .unwrap_or_else(|| panic!("`/bin/echo` is not Claude Code"))
        .to_string();
    assert!(reason.contains("/bin/echo"), "{reason}");
    assert!(reason.contains("is not Claude Code"), "{reason}");
}

/// `codex app-server` completes the real handshake and starts a thread, and Fleet tears it down.
#[tokio::test]
async fn codex_completes_the_real_handshake() {
    let worktree = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let probes = ProbeCache::new();
    let mut codex = harness::spawn(AgentKind::Codex, &config(AgentKind::Codex), &probes)
        .await
        .unwrap_or_else(|error| panic!("spawn codex: {error}"));
    let mut events = codex.events();
    let opened = tokio::time::timeout(
        Duration::from_secs(30),
        codex.open(OpenSession {
            start: request(AgentKind::Codex, worktree.path().to_path_buf()),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("the handshake must not hang"))
    .unwrap_or_else(|error| panic!("open codex: {error}"));
    assert!(
        opened.resume_cursor.is_some(),
        "the resume cursor is the thread id and is durable before any turn"
    );
    assert!(codex.capabilities().version.minor > 0);
    // The handshake reads the account, and the read is **read-only**: nothing here signs in or
    // out, because this test runs against the developer's own Codex install. A machine with no
    // OpenAI account configured reports `SignedOut`, which is just as valid an answer — what is
    // asserted is that the signal arrives at all, and that it never carries a secret.
    let mut account = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::AccountChanged { account: status } = event.event {
            account = Some(status);
        }
    }
    let account = account.unwrap_or_else(|| {
        panic!("the handshake publishes the account Codex reported, signed in or not")
    });
    if let AccountStatus::SignedIn(info) = &account {
        assert!(
            info.label().is_none_or(|label| !label.is_empty()),
            "a label is a real label or no label at all: {info:?}"
        );
    }

    codex
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown codex: {error}"));
    drop(events.try_recv());
}

/// `claude -p` starts, survives its own startup, and tears down cleanly.
#[tokio::test]
async fn claude_starts_and_tears_down() {
    let worktree = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let probes = ProbeCache::new();
    let mut claude = harness::spawn(AgentKind::Claude, &config(AgentKind::Claude), &probes)
        .await
        .unwrap_or_else(|error| panic!("spawn claude: {error}"));
    let mut events = claude.events();
    claude
        .open(OpenSession {
            start: request(AgentKind::Claude, worktree.path().to_path_buf()),
        })
        .await
        .unwrap_or_else(|error| panic!("open claude: {error}"));
    claude
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown claude: {error}"));
    drop(events.try_recv());
}
