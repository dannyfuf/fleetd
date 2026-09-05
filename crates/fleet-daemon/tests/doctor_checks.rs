use std::sync::Arc;

use fleet_daemon::{
    adapters::shell::ShellResult,
    jobs::JobManager,
    services::doctor::Doctor,
    stores::config::ConfigStore,
    testing::fakes::{FakeFiles, FakeGit, FakeGithub, FakeShell, FixedClock},
};

#[tokio::test]
async fn doctor_reports_local_runtime_and_unsupported_remote_hosts() {
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
                "devbox": {"ssh": "devbox.example.com", "swarmCommand": "swarm"}
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
    let git = Arc::new(FakeGit::new(shell.clone()));
    let github = Arc::new(FakeGithub::new(shell.clone()));
    let now = chrono::Utc::now();
    let jobs = Arc::new(JobManager::with_clock(
        &home,
        Arc::new(FixedClock::new(now)),
    ));
    let doctor = Doctor::new(jobs, config, shell, git, github, files);

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
    use fleet_proto::response::DoctorStatus;
    assert_eq!(find("gh auth").status, DoctorStatus::Ok);
    assert_eq!(find("copy-on-write").status, DoctorStatus::Ok);
    assert_eq!(find("runtime").status, DoctorStatus::Ok);
    assert_eq!(find("daemon socket").status, DoctorStatus::Ok);
    assert_eq!(find("FLEET_HOME writable").status, DoctorStatus::Ok);
    assert_eq!(find("host devbox").status, DoctorStatus::Warn);
    assert_eq!(
        find("host devbox").detail,
        "remote hosts are not supported yet"
    );
}
