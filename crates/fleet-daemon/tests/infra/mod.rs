//! Shared RAII fixtures for daemon integration tests.

use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

const DEADLINE: Duration = Duration::from_secs(5);

pub struct DaemonProcess {
    child: Child,
}

impl DaemonProcess {
    pub fn start(home: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_fleetd"))
            .arg("--home")
            .arg(home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start fleetd");
        Self { child }
    }

    pub async fn wait(&mut self) {
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let Some(status) = self.child.try_wait().expect("wait for fleetd") {
                    assert!(status.success(), "fleetd exited with {status}");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("fleetd did not shut down within five seconds");
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
