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

use fleet_core::agents::{
    AgentKind, ApprovalPolicy, PermissionMode, SandboxPolicy, StartRequest, ThreadId,
};
use fleet_daemon::agents::harness::{self, OpenSession, ShutdownReason, probe::ProbeCache};

fn config(kind: AgentKind) -> harness::HarnessConfig {
    harness::HarnessConfig {
        command: kind.executable().to_owned(),
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
    let probes = ProbeCache::new();
    for kind in [AgentKind::Claude, AgentKind::Codex] {
        let probed = probes
            .probe(kind, &config(kind))
            .await
            .unwrap_or_else(|error| panic!("{kind:?} must probe: {error}"));
        assert_eq!(probed.kind, kind);
        assert!(
            probed.version.major > 0 || probed.version.minor > 0,
            "{probed:?}"
        );
    }
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
