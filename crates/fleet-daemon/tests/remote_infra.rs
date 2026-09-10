use std::time::Duration;

use fleet_daemon::machines::MachineProvider;

mod infra;

#[tokio::test]
async fn loopback_command_machine_probes_a_real_fleetd_binary() {
    let _ = infra::DaemonProcess::wait;
    let temp = tempfile::tempdir().expect("remote home");
    let remote = infra::RemoteDaemon::start(temp.path());
    infra::assert_remote_contract(&remote);
    let report = remote.machine.probe(Duration::from_secs(5)).await;
    assert!(report.reachable, "{:?}", report.error);
    assert!(
        report
            .version
            .as_deref()
            .is_some_and(|version| version.contains("fleetd"))
    );
}
