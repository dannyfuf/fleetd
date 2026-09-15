use super::*;
use crate::env::HarnessEnv;
use std::os::unix::fs::PermissionsExt as _;
use tokio::io::{AsyncBufReadExt as _, BufReader};

#[tokio::test]
async fn a_competing_spawn_cannot_win_while_the_adopted_replacement_takes_ownership() {
    let temporary = tempfile::tempdir().expect("temporary home");
    let environment = HarnessEnv::rooted(temporary.path()).expect("lay out the environment");
    let pid_file = FleetHome::new(environment.fleet_home.clone()).pid_path();
    fs::create_dir(&pid_file).expect("seal the home");

    let binary = temporary.path().join("fake-fleetd");
    fs::write(
        &binary,
        "#!/bin/sh\n[ \"$1\" = --home ] || exit 71\nprintf '%s\\n' \"$$\" > \"$2/fleetd.pid\"\nprintf 'ready\\n'\nexec sleep 30\n",
    )
    .expect("write fake fleetd");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .expect("make fake fleetd executable");

    let mut fleet = Process::new("sh");
    fleet
        .arg("-c")
        .arg(
            "printf 'fleet-ready\\n'\nIFS= read -r ready\npid=$(cat \"$1/fleetd.pid\" 2>/dev/null) || { printf 'won\\n'; exit 0; }\nif kill -0 \"$pid\" 2>/dev/null; then printf 'lost:%s\\n' \"$pid\"; else printf 'won\\n'; fi\n",
        )
        .arg("fleet-competitor")
        .arg(&environment.fleet_home);
    environment.apply_to_app(&mut fleet);
    fleet
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut fleet = fleet.spawn().expect("spawn fake Fleet");
    let fleet_pid = fleet.id().expect("fake Fleet has a process id");
    let mut fleet_input = fleet.stdin.take().expect("take fake Fleet input");
    let mut fleet_output = BufReader::new(fleet.stdout.take().expect("take fake Fleet output"));
    let mut line = String::new();
    fleet_output
        .read_line(&mut line)
        .await
        .expect("wait for fake Fleet");
    assert_eq!(line, "fleet-ready\n");
    assert_eq!(
        environment.app_pid().expect("read fake Fleet PID"),
        fleet_pid
    );

    signal(fleet_pid, "STOP").await.expect("suspend fake Fleet");
    await_stopped(fleet_pid)
        .await
        .expect("confirm fake Fleet is stopped");
    fleet_input
        .write_all(b"start\n")
        .await
        .expect("queue the competing spawn attempt");

    let mut adopted = replacement_command(&binary, &environment.fleet_home)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn adopted replacement");
    let adopted_pid = adopted.id().expect("adopted replacement has a process id");
    let mut gate = adopted.stdin.take().expect("take replacement gate");
    let mut adopted_output = BufReader::new(
        adopted
            .stdout
            .take()
            .expect("take adopted replacement output"),
    );
    fs::remove_dir(&pid_file).expect("unseal for adopted replacement");
    gate.write_all(b"start\n")
        .await
        .expect("release replacement");
    gate.shutdown().await.expect("close replacement gate");
    line.clear();
    adopted_output
        .read_line(&mut line)
        .await
        .expect("wait for adopted ownership");
    assert_eq!(line, "ready\n");

    let recorded_pid: u32 = fs::read_to_string(&pid_file)
        .expect("read replacement PID")
        .trim()
        .parse()
        .expect("parse replacement PID");
    assert_eq!(
        recorded_pid, adopted_pid,
        "readiness names the adopted child"
    );

    signal(fleet_pid, "CONT").await.expect("resume fake Fleet");
    line.clear();
    fleet_output
        .read_line(&mut line)
        .await
        .expect("wait for the competing attempt");
    assert_eq!(line, format!("lost:{adopted_pid}\n"));
    let fleet_status = fleet.wait().await.expect("reap fake Fleet");
    assert!(
        fleet_status.success(),
        "fake Fleet exited with {fleet_status}"
    );
    adopted.kill().await.expect("stop adopted replacement");
}
