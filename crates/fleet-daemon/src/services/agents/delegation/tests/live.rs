//! Opt-in smoke tests for the real native-agent binaries.
//!
//! Run them on a signed-in workstation with:
//! `TMPDIR=/home/df/.cache/fleet-tmp cargo test -p fleet-daemon --features real-agents delegation::live -- --test-threads=1`.

use std::{os::unix::fs::PermissionsExt as _, path::Path, time::Duration};

use fleet_core::agents::{AgentKind, DelegationStatus, ResultSource, SessionState, TurnState};
use fleet_proto::response::ResponseBody;

use super::{
    CompleteRequest,
    tests::run::{Harness, request, started},
};

const LIVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

async fn real_provider_reports(provider: AgentKind) {
    let report_directory = tempfile::tempdir().expect("create live-provider report directory");
    let bin = report_directory.path().join("bin");
    std::fs::create_dir(&bin).expect("create live-provider bin directory");
    let captured_report = report_directory.path().join("reported.md");
    write_fleet_capture(&bin.join("fleet"), &captured_report);
    let captured_token = report_directory.path().join("token");
    let provider_wrapper = bin.join("live-provider");
    write_provider_wrapper(&provider_wrapper, provider, &bin, &captured_token);
    let target = report_directory.path().join("hello.txt");

    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    let provider_wrapper = provider_wrapper.display().to_string();
    let command = shell_words::quote(&provider_wrapper);
    match provider {
        AgentKind::Claude => harness.set_claude_binary(&command).await,
        AgentKind::Codex => harness.set_codex_binary(&command).await,
    }
    let mut run = request(caller);
    run.provider = provider;
    run.brief = format!("write hello to {} and report the path", target.display());
    run.expectation = "the report names the path that contains hello".to_owned();

    tokio::time::resume();
    tokio::time::timeout(LIVE_TIMEOUT, async {
        let (delegation, _) = started(
            harness
                .service()
                .run(run)
                .await
                .expect("start real provider delegation"),
        );
        let token = std::fs::read_to_string(&captured_token)
            .expect("real provider wrapper captured the delegation token");
        loop {
            let projection = harness
                .wait_for(delegation.child, |projection| {
                    matches!(projection.turn, TurnState::Settled(_, _))
                        || matches!(
                            projection.session,
                            SessionState::Error | SessionState::Stopped
                        )
                        || !projection.gates.is_empty()
                })
                .await;
            if matches!(projection.turn, TurnState::Settled(_, _)) {
                break;
            }
            if matches!(
                projection.session,
                SessionState::Error | SessionState::Stopped
            ) {
                panic!("real provider stopped before settling: {projection:?}");
            }
            let gate = projection
                .gates
                .first()
                .expect("the live-provider wait returned an open gate");
            assert!(
                matches!(gate.kind, fleet_core::agents::GateKind::Permission { .. }),
                "real provider opened a non-permission gate: {projection:?}"
            );
            let gate = gate.id;
            harness.allow_permission(delegation.child, gate).await;
            harness
                .wait_for(delegation.child, |projection| {
                    projection.gates.iter().all(|open| open.id != gate)
                })
                .await;
        }

        let hello = std::fs::read_to_string(&target).expect("real provider wrote hello.txt");
        assert_eq!(hello.trim(), "hello");
        let report = std::fs::read_to_string(&captured_report)
            .expect("real provider invoked fleet subagent complete");
        let completed = harness
            .service()
            .complete(CompleteRequest {
                delegation: delegation.id,
                child: delegation.child,
                token,
                result: report,
                blocked: false,
            })
            .await
            .expect("accept captured live-provider report");
        let ResponseBody::Delegation(completed) = completed else {
            panic!("expected Delegation response");
        };
        assert_eq!(completed.status, DelegationStatus::Succeeded);
        assert_eq!(
            completed.result.map(|result| result.source),
            Some(ResultSource::Reported)
        );
    })
    .await
    .expect("real provider starts, completes, and settles within ten minutes");
}

fn write_fleet_capture(executable: &Path, captured_report: &Path) {
    let script = format!(
        "#!/bin/sh\nreport=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--result-file\" ]; then\n    shift\n    report=$1\n  fi\n  shift\ndone\n[ -n \"$report\" ] || exit 64\ncp \"$report\" '{}'\n",
        captured_report.display()
    );
    std::fs::write(executable, script).expect("write live-provider fleet capture");
    let mut permissions = std::fs::metadata(executable)
        .expect("read live-provider fleet capture metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(executable, permissions)
        .expect("make live-provider fleet capture executable");
}

fn write_provider_wrapper(wrapper: &Path, provider: AgentKind, bin: &Path, captured_token: &Path) {
    let executable = match provider {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    };
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"$FLEET_DELEGATION_TOKEN\" > '{}'\nPATH='{}':\"$PATH\"\nexport PATH\nexec {executable} \"$@\"\n",
        captured_token.display(),
        bin.display()
    );
    std::fs::write(wrapper, script).expect("write live-provider wrapper");
    let mut permissions = std::fs::metadata(wrapper)
        .expect("read live-provider wrapper metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(wrapper, permissions).expect("make live-provider wrapper executable");
}

/// Run with the module command above on a workstation where `claude` is signed in.
#[tokio::test(start_paused = true)]
async fn claude_reports_one_live_delegation() {
    real_provider_reports(AgentKind::Claude).await;
}

/// Run with the module command above on a workstation where `codex` is signed in.
#[tokio::test(start_paused = true)]
async fn codex_reports_one_live_delegation() {
    real_provider_reports(AgentKind::Codex).await;
}
