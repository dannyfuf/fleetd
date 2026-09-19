//! Handshake tests against the **real** harness binaries.
//!
//! Opt-in: `cargo test -p fleet-daemon --features real-agents --test agents_real_binaries`.
//! The default `make test` run spawns neither `claude` nor `codex` — every other agent test
//! drives a scripted `/bin/sh` mock peer — because a test suite that needs two vendor CLIs
//! installed is a test suite that silently skips on the machine that most needs it.
//!
//! The handshake tests send no prompt and cost no token. The two `*_answers_*` tests below do:
//! each drives one or two real turns, because a harness that greets and then never settles a turn
//! is exactly the failure a user reports as "it looks stuck".
#![cfg(feature = "real-agents")]

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use fleet_core::{
    agents::{
        AccountStatus, AgentEvent, AgentKind, ApprovalPolicy, PermissionMode, SandboxPolicy,
        SessionState, StartRequest, ThreadId, TurnId, TurnOutcome, UserInput,
    },
    config::AgentBinaries,
};
use fleet_daemon::agents::harness::{
    self, Harness, HarnessEvents, OpenSession, ShutdownReason, Submit, SubmitIntent,
    probe::ProbeCache,
};

/// How long one real turn may take. Generous: a cold model call over a slow link is not a bug.
const TURN_DEADLINE: Duration = Duration::from_secs(120);

/// Submits `text` as a fresh turn and waits for its settlement, collecting every event seen.
async fn answer(
    harness: &mut Box<dyn Harness>,
    events: &mut HarnessEvents,
    text: &str,
    seen: &mut Vec<AgentEvent>,
) -> TurnOutcome {
    let turn = TurnId::new();
    harness
        .submit(Submit {
            turn,
            input: UserInput {
                text: text.to_owned(),
                ..UserInput::default()
            },
            intent: SubmitIntent::Fresh,
        })
        .await
        .unwrap_or_else(|error| panic!("submit: {error}"));
    tokio::time::timeout(TURN_DEADLINE, async {
        while let Some(event) = events.recv().await {
            let settled = match &event.event {
                AgentEvent::TurnSettled {
                    turn: settled,
                    outcome,
                    ..
                } if *settled == turn => Some(outcome.clone()),
                _ => None,
            };
            seen.push(event.event);
            if let Some(outcome) = settled {
                return outcome;
            }
        }
        panic!("the event stream closed before the turn settled");
    })
    .await
    .unwrap_or_else(|_| panic!("the turn never settled; seen: {}", summary(seen)))
}

fn summary(events: &[AgentEvent]) -> String {
    events
        .iter()
        .map(|event| format!("{event:?}").chars().take(60).collect::<String>())
        .collect::<Vec<_>>()
        .join(" | ")
}

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
        path_prepend: None,
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

/// Two real Claude turns settle, the session is `Ready` before anyone types, and the repeated
/// `system/init` the CLI sends with every prompt configures the session exactly once.
#[tokio::test]
async fn claude_answers_two_turns_and_publishes_ready_before_the_first_prompt() {
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
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        seen.push(event.event);
    }
    assert!(
        seen.iter()
            .any(|event| matches!(event, AgentEvent::SessionStateChanged(SessionState::Ready))),
        "a child that survived its startup is ready before the first prompt: {}",
        summary(&seen)
    );

    let first = answer(
        &mut claude,
        &mut events,
        "Reply with exactly the single word: pong",
        &mut seen,
    )
    .await;
    assert_eq!(first, TurnOutcome::Completed, "{}", summary(&seen));
    let second = answer(
        &mut claude,
        &mut events,
        "Reply with exactly the single word: pong",
        &mut seen,
    )
    .await;
    assert_eq!(second, TurnOutcome::Completed, "{}", summary(&seen));
    assert_eq!(
        seen.iter()
            .filter(|event| matches!(event, AgentEvent::SessionConfigured { .. }))
            .count(),
        1,
        "init is re-sent every turn and must configure the session once: {}",
        summary(&seen)
    );
    claude
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown claude: {error}"));
}

/// One real Codex turn settles. A machine with no signed-in account cannot run a turn, and says
/// so instead of failing.
#[tokio::test]
async fn codex_answers_a_turn() {
    let worktree = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let probes = ProbeCache::new();
    let mut codex = harness::spawn(AgentKind::Codex, &config(AgentKind::Codex), &probes)
        .await
        .unwrap_or_else(|error| panic!("spawn codex: {error}"));
    let mut events = codex.events();
    codex
        .open(OpenSession {
            start: request(AgentKind::Codex, worktree.path().to_path_buf()),
        })
        .await
        .unwrap_or_else(|error| panic!("open codex: {error}"));
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        seen.push(event.event);
    }
    if seen.iter().any(|event| {
        matches!(
            event,
            AgentEvent::AccountChanged {
                account: AccountStatus::SignedOut
            }
        )
    }) {
        eprintln!("codex is signed out on this machine; a turn cannot be exercised");
        return;
    }
    let outcome = answer(
        &mut codex,
        &mut events,
        "Reply with exactly the single word: pong",
        &mut seen,
    )
    .await;
    assert_eq!(outcome, TurnOutcome::Completed, "{}", summary(&seen));
    codex
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown codex: {error}"));
}
