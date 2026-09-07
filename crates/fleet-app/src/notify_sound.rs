//! Bounded, nonblocking admission to a single notification playback worker.

use std::fmt;

pub(crate) trait NotificationSound: fmt::Debug + Send + Sync {
    fn play(&self);
}

#[derive(Debug, Default)]
pub(crate) struct SystemSound;

#[cfg(any(target_os = "macos", test))]
const PENDING_SOUNDS: usize = 4;

#[cfg(any(target_os = "macos", test))]
fn start_worker(
    play: impl Fn() + Send + 'static,
) -> std::io::Result<std::sync::mpsc::SyncSender<()>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(PENDING_SOUNDS);
    std::thread::Builder::new()
        .name("fleet-notification-sound".into())
        .spawn(move || {
            while receiver.recv().is_ok() {
                play();
            }
        })?;
    Ok(sender)
}

#[cfg(target_os = "macos")]
impl NotificationSound for SystemSound {
    fn play(&self) {
        use std::{
            process::{Command, Stdio},
            sync::{LazyLock, mpsc::TrySendError},
        };

        static WORKER: LazyLock<Option<std::sync::mpsc::SyncSender<()>>> = LazyLock::new(|| {
            match start_worker(|| {
                match Command::new("/usr/bin/afplay")
                    .arg("/System/Library/Sounds/Glass.aiff")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                {
                    Ok(status) if status.success() => {}
                    Ok(status) => tracing::warn!(%status, "notification playback failed"),
                    Err(error) => tracing::warn!(%error, "could not play notification sound"),
                }
            }) {
                Ok(sender) => Some(sender),
                Err(error) => {
                    tracing::warn!(%error, "could not start notification sound worker");
                    None
                }
            }
        });

        if let Some(worker) = &*WORKER {
            match worker.try_send(()) {
                Ok(()) => {}
                Err(TrySendError::Full(())) => tracing::debug!("notification sound queue is full"),
                Err(TrySendError::Disconnected(())) => {
                    tracing::warn!("notification sound worker stopped")
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl NotificationSound for SystemSound {
    fn play(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, TrySendError};
    use std::time::Duration;

    #[test]
    fn playback_is_serial_and_admission_is_bounded() {
        let (started, starts) = mpsc::channel();
        let (release, releases) = mpsc::channel();
        let worker = start_worker(move || {
            started.send(()).unwrap();
            releases.recv().unwrap();
        })
        .unwrap();
        worker.try_send(()).unwrap();
        starts.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..PENDING_SOUNDS {
            worker.try_send(()).unwrap();
        }
        assert!(matches!(worker.try_send(()), Err(TrySendError::Full(()))));
        assert!(starts.try_recv().is_err());
        for _ in 0..PENDING_SOUNDS {
            release.send(()).unwrap();
            starts.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        release.send(()).unwrap();
        drop(worker);
        assert!(starts.recv_timeout(Duration::from_secs(5)).is_err());
    }
}
