use std::sync::Arc;

use fleet_core::{config::default_config, ids::HostId, model::HostConfigEntry};
use fleet_daemon::{
    adapters::shell::ShellResult,
    machines::{
        ExecOutput, MachineAddress, MachineProvider, Machines, ProbeReport, RemoteEndpoint,
        RemoteHello,
    },
    services::hosts::Hosts,
    testing::fakes::FakeShell,
    testing::{FakeMachine, FakeRemote},
};
use fleet_proto::snapshot::{AgentBinaries, LinkState};

fn host_id(value: &str) -> HostId {
    HostId::try_from(value).unwrap_or_else(|error| panic!("{error}"))
}

#[tokio::test]
async fn fake_machine_and_remote_assemble_ready_status_and_skip_probe() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let hosts = Hosts::new(temp.path(), Arc::new(FakeShell::new()));
    let id = host_id("dev-box");
    let machine = Arc::new(FakeMachine::new(id.clone()));
    machine.push_resolve(Ok(MachineAddress {
        host: "100.77.28.11".to_owned(),
        user: Some("df".to_owned()),
        display: "dev-box".to_owned(),
        online: Some(true),
    }));
    machine.push_probe(ProbeReport {
        reachable: false,
        latency_ms: Some(5),
        version: None,
        error: Some("probe should remain queued".to_owned()),
        stderr: Some("probe should remain queued\n".to_owned()),
    });
    machine.push_exec(Ok(ExecOutput {
        status: 0,
        stdout: "claude\nopencode\n".to_owned(),
        stderr: String::new(),
    }));
    machine.push_exec(Ok(ExecOutput {
        status: 0,
        stdout: "claude\n".to_owned(),
        stderr: String::new(),
    }));
    let remote = Arc::new(FakeRemote::new(id));
    remote.set_hello(RemoteHello {
        version: "fleetd 0.1.0".to_owned(),
        daemon_id: "remote-daemon".to_owned(),
        build_commit: Some("abc123".to_owned()),
        capabilities: vec!["remote-machines".to_owned()],
    });
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let endpoint: Arc<dyn RemoteEndpoint> = remote.clone();
    let _state = remote.state_changes();

    let ready = hosts
        .refresh_machine(Arc::clone(&provider), Some(Arc::clone(&endpoint)))
        .await;
    assert_eq!(ready.provider, "command");
    assert_eq!(ready.address.as_deref(), Some("100.77.28.11"));
    assert_eq!(ready.version.as_deref(), Some("fleetd 0.1.0"));
    assert_eq!(ready.link, LinkState::Ready);
    assert!(ready.reachable);
    assert_eq!(ready.error, None);
    assert_eq!(
        ready.agent_binaries,
        Some(AgentBinaries {
            claude: true,
            opencode: true,
        })
    );
    assert_eq!(
        machine.exec_calls()[0],
        vec![
            "bash",
            "-lc",
            concat!(
                "if command -v claude >/dev/null 2>&1; then printf 'claude\\n'; fi; ",
                "if command -v opencode >/dev/null 2>&1; then printf 'opencode\\n'; fi",
            ),
        ]
    );

    remote.set_state(LinkState::Down);
    let down = hosts.probe_machine(provider, Some(endpoint)).await;
    assert_eq!(down.link, LinkState::Down);
    assert!(!down.reachable);
    assert_eq!(down.error.as_deref(), Some("probe should remain queued"));
    assert_eq!(
        down.agent_binaries,
        Some(AgentBinaries {
            claude: true,
            opencode: false,
        })
    );
}

#[tokio::test]
async fn fake_machine_preserves_down_probe_error_and_resolved_address() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let hosts = Hosts::new(temp.path(), Arc::new(FakeShell::new()));
    let machine = Arc::new(FakeMachine::new(host_id("offline")));
    machine.push_resolve(Ok(MachineAddress {
        host: "100.64.0.9".to_owned(),
        user: None,
        display: "offline".to_owned(),
        online: Some(false),
    }));
    machine.push_probe(ProbeReport {
        reachable: false,
        latency_ms: Some(12),
        version: None,
        error: Some("authentication failed".to_owned()),
        stderr: Some("Permission denied (publickey)\n".to_owned()),
    });
    machine.push_exec(Ok(ExecOutput {
        status: 0,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let provider: Arc<dyn MachineProvider> = machine;

    let status = hosts.probe_machine(provider, None).await;
    assert_eq!(status.provider, "command");
    assert_eq!(status.address.as_deref(), Some("100.64.0.9"));
    assert_eq!(status.link, LinkState::Down);
    assert!(!status.reachable);
    assert_eq!(status.error.as_deref(), Some("authentication failed"));
    assert_eq!(
        status.agent_binaries,
        Some(AgentBinaries {
            claude: false,
            opencode: false,
        })
    );
}

#[tokio::test]
async fn legacy_entries_keep_the_swarm_probe_and_report_legacy_link() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let shell = Arc::new(FakeShell::new());
    shell.when(
        |command| command.program == "ssh",
        ShellResult {
            status: 0,
            stdout: r#"{"protocol":1,"version":"swarm 0.1.0"}"#.to_owned(),
            stderr: String::new(),
        },
    );
    let hosts = Hosts::new(temp.path(), shell);
    let id = host_id("legacy-box");
    let mut config = default_config(temp.path());
    config.hosts.insert(
        id.clone(),
        HostConfigEntry::Legacy {
            ssh: "legacy.example.com".to_owned(),
            swarm_command: "swarm".to_owned(),
        },
    );
    let machines = Machines::from_config(&config);

    let statuses = hosts.probe_all(&config, &machines).await;
    let status = &statuses[&id];
    assert_eq!(status.provider, "legacy");
    assert_eq!(status.address.as_deref(), Some("legacy.example.com"));
    assert_eq!(status.version.as_deref(), Some("swarm 0.1.0"));
    assert_eq!(status.link, LinkState::Legacy);
    assert!(status.reachable);
}
