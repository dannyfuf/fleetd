use std::{collections::HashMap, ffi::OsString, path::Path, sync::Arc};

use fleet_daemon::{
    adapters::shell::ShellResult,
    machines::{
        ExecOutput, MachineAddress, MachineProvider, ProbeReport, RemoteEndpoint, RemoteHello,
    },
    services::{
        Services,
        doctor::{Doctor, subagent_fleet_check},
    },
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

/// A login environment whose `PATH` holds exactly one directory.
fn path_environment(directory: &Path) -> HashMap<OsString, OsString> {
    HashMap::from([(OsString::from("PATH"), directory.as_os_str().to_os_string())])
}

/// Writes an empty file where a binary would be; `resolve_program` only asks whether the
/// candidate is a file, so no execute bit is needed to stand in for one.
fn touch_binary(directory: &Path, name: &str) -> std::path::PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, b"#!/bin/sh\n").unwrap_or_else(|error| panic!("{error}"));
    path
}

#[test]
fn subagent_fleet_check_reports_the_resolved_child_path() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let on_path = temp.path().join("bin");
    std::fs::create_dir_all(&on_path).unwrap_or_else(|error| panic!("{error}"));
    let fleet = touch_binary(&on_path, "fleet");
    let environment = path_environment(&on_path);

    let check = subagent_fleet_check(&environment, None);

    assert_eq!(check.check, "subagent fleet CLI");
    assert_eq!(check.status, DoctorStatus::Ok);
    assert_eq!(check.detail, fleet.display().to_string());

    let libexec = temp.path().join("libexec");
    std::fs::create_dir_all(&libexec).unwrap_or_else(|error| panic!("{error}"));
    touch_binary(&libexec, "fleet");
    let daemon_exe = libexec.join("fleetd");

    let check = subagent_fleet_check(&environment, Some(&daemon_exe));

    assert_eq!(check.status, DoctorStatus::Ok);
    assert!(
        check.detail.starts_with(&fleet.display().to_string()),
        "the resolved path stays first: {}",
        check.detail
    );
    assert!(
        check.detail.contains(&libexec.display().to_string()),
        "the injected directory is named too: {}",
        check.detail
    );
}

/// The case the daemon-side fallback exists for: the login shell knows nothing about `fleet`,
/// yet a child still reports because Fleet prepends the directory holding `fleetd`'s sibling.
#[test]
fn subagent_fleet_check_passes_on_the_injectable_directory_alone() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let empty = temp.path().join("bin");
    std::fs::create_dir_all(&empty).unwrap_or_else(|error| panic!("{error}"));
    let libexec = temp.path().join("libexec");
    std::fs::create_dir_all(&libexec).unwrap_or_else(|error| panic!("{error}"));
    touch_binary(&libexec, "fleet");
    let daemon_exe = libexec.join("fleetd");

    let check = subagent_fleet_check(&path_environment(&empty), Some(&daemon_exe));

    assert_eq!(check.check, "subagent fleet CLI");
    assert_eq!(check.status, DoctorStatus::Ok);
    assert!(
        check.detail.contains(&libexec.display().to_string()),
        "the injected directory is named: {}",
        check.detail
    );
    assert!(
        !check.detail.contains("symlink"),
        "nothing to fix, so no PATH advice: {}",
        check.detail
    );
}

#[test]
fn subagent_fleet_check_fails_only_when_there_is_nothing_to_inject() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let empty = temp.path().join("bin");
    std::fs::create_dir_all(&empty).unwrap_or_else(|error| panic!("{error}"));
    let daemon_exe = temp.path().join("libexec").join("fleetd");

    let check = subagent_fleet_check(&path_environment(&empty), Some(&daemon_exe));

    assert_eq!(check.check, "subagent fleet CLI");
    assert_eq!(check.status, DoctorStatus::Fail);
    assert_eq!(
        check.detail,
        concat!(
            "subagents cannot report: fleet is not on the harness child's PATH and there ",
            "is none beside fleetd to inject; put the built fleet on PATH or symlink it ",
            "into a directory already there"
        )
    );
}
