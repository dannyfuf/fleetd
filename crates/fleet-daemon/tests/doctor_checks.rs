use std::sync::Arc;

use fleet_daemon::{
    adapters::shell::ShellResult,
    machines::{
        ExecOutput, MachineAddress, MachineProvider, ProbeReport, RemoteEndpoint, RemoteHello,
    },
    services::{Services, doctor::Doctor},
    stores::config::ConfigStore,
    testing::fakes::{FakeFiles, FakeGithub, FakeShell},
    testing::{FakeMachine, FakeRemote},
};
use fleet_proto::{response::DoctorStatus, snapshot::LinkState};

#[tokio::test]
async fn doctor_reports_local_runtime_and_probes_remote_hosts() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let files = Arc::new(FakeFiles::new(
        home.join("trash"),
        vec![home.join("repos"), home.join("worktrees")],
    ));
    files.insert_text(home.join("fleetd.sock"), "socket placeholder");
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    config
        .update(serde_json::json!({
            "hosts": {
                "devbox": {"ssh": "devbox.example.com", "swarmCommand": "swarm"},
                "offline": {"ssh": "offline.example.com", "swarmCommand": "swarm"}
            }
        }))
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let shell = Arc::new(FakeShell::new());
    shell.when(
        |command| command.program == "git" && command.args == ["--version"],
        ShellResult {
            status: 0,
            stdout: "git version 2.51.0\n".to_owned(),
            stderr: String::new(),
        },
    );
    shell.when(
        |command| command.program == "gh" && command.args == ["auth", "status"],
        ShellResult {
            status: 0,
            stdout: "authenticated\n".to_owned(),
            stderr: String::new(),
        },
    );
    shell.when(
        |command| {
            command.program == "ssh" && command.args.contains(&"devbox.example.com".to_owned())
        },
        ShellResult {
            status: 0,
            stdout: r#"{"protocol":1,"version":"swarm 0.1.0+a5a11f0"}"#.to_owned(),
            stderr: String::new(),
        },
    );
    shell.when(
        |command| {
            command.program == "ssh" && command.args.contains(&"offline.example.com".to_owned())
        },
        ShellResult {
            status: 255,
            stdout: String::new(),
            stderr: "warning\nPermission denied (publickey)\n".to_owned(),
        },
    );
    let github = Arc::new(FakeGithub::new(shell.clone()));
    let doctor = Doctor::new(config, shell, github, files);

    let checks = doctor
        .check()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let find = |name: &str| {
        checks
            .iter()
            .find(|check| check.check == name)
            .unwrap_or_else(|| panic!("missing doctor check {name}"))
    };
    assert_eq!(find("git").detail, "git version 2.51.0");
    assert_eq!(find("gh auth").status, DoctorStatus::Ok);
    assert_eq!(find("copy-on-write").status, DoctorStatus::Ok);
    assert_eq!(find("runtime").status, DoctorStatus::Ok);
    assert_eq!(find("daemon socket").status, DoctorStatus::Ok);
    assert_eq!(find("FLEET_HOME writable").status, DoctorStatus::Ok);
    assert_eq!(find("host devbox").status, DoctorStatus::Ok);
    assert_eq!(
        find("host devbox").detail,
        "devbox.example.com · swarm 0.1.0+a5a11f0"
    );
    assert_eq!(find("host offline").status, DoctorStatus::Fail);
    assert_eq!(find("host offline").detail, "Permission denied (publickey)");
    assert_eq!(find("host devbox provider").detail, "legacy");
    assert_eq!(find("host devbox address").detail, "devbox.example.com");
    assert_eq!(
        find("host offline ssh/probe").detail,
        "warning\nPermission denied (publickey)\n"
    );
    assert!(
        find("host devbox migration")
            .detail
            .contains(r#"{"provider":"tailscale","node":"<node>""#)
    );
}

#[tokio::test]
async fn doctor_reports_provider_address_version_link_and_protocol_for_ready_machine() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let files = Arc::new(FakeFiles::new(
        home.join("trash"),
        vec![home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let shell = Arc::new(FakeShell::new());
    let github = Arc::new(FakeGithub::new(shell.clone()));
    let doctor = Doctor::new(config, shell, github, files);
    let id = fleet_core::ids::HostId::try_from("dev-box").unwrap_or_else(|error| panic!("{error}"));
    let machine = Arc::new(FakeMachine::new(id.clone()));
    machine.push_resolve(Ok(MachineAddress {
        host: "100.77.28.11".to_owned(),
        user: Some("df".to_owned()),
        display: "dev-box".to_owned(),
        online: Some(true),
    }));
    machine.push_probe(ProbeReport {
        reachable: true,
        latency_ms: Some(8),
        version: Some(Services::version()),
        error: None,
        stderr: None,
    });
    machine.push_exec(Ok(ExecOutput {
        status: 0,
        stdout: "claude\n".to_owned(),
        stderr: String::new(),
    }));
    let remote = Arc::new(FakeRemote::new(id));
    remote.set_state(LinkState::Ready);
    remote.set_hello(RemoteHello {
        version: Services::version(),
        daemon_id: "remote-daemon".to_owned(),
        build_commit: Some("abc123".to_owned()),
        capabilities: vec!["remote-machines".to_owned()],
    });
    let provider: Arc<dyn MachineProvider> = machine;
    let endpoint: Arc<dyn RemoteEndpoint> = remote;

    let checks = doctor.check_machine(provider, Some(endpoint)).await;
    let find = |name: &str| {
        checks
            .iter()
            .find(|check| check.check == name)
            .unwrap_or_else(|| panic!("missing doctor check {name}"))
    };
    assert_eq!(find("host dev-box provider").detail, "command");
    assert_eq!(find("host dev-box address").detail, "100.77.28.11");
    assert_eq!(find("host dev-box link").detail, "ready");
    assert_eq!(find("host dev-box protocol").status, DoctorStatus::Ok);
    assert_eq!(find("host dev-box claude").status, DoctorStatus::Ok);
    assert_eq!(find("host dev-box opencode").status, DoctorStatus::Fail);
    assert!(
        find("host dev-box protocol")
            .detail
            .contains("matches local")
    );
    assert!(
        find("host dev-box fleetd version")
            .detail
            .contains("remote fleetd")
    );
    assert_eq!(
        find("host dev-box").detail,
        format!(
            "provider command · address 100.77.28.11 · version {} · link ready",
            Services::version()
        )
    );
}
