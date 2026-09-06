//! Audible agent-finished notifications kept behind a tiny testable boundary.

use std::fmt;

/// Plays the configured agent-finished notification sound.
pub trait NotificationSound: fmt::Debug + Send + Sync {
    /// Starts playback without blocking the caller.
    fn play(&self);
}

/// The platform notification sound used by the native app.
#[derive(Debug, Default)]
pub struct SystemSound;

#[cfg(target_os = "macos")]
impl NotificationSound for SystemSound {
    fn play(&self) {
        std::thread::spawn(|| {
            use std::process::{Command, Stdio};

            let _ignored = Command::new("/usr/bin/afplay")
                .arg("/System/Library/Sounds/Glass.aiff")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        });
    }
}

#[cfg(not(target_os = "macos"))]
impl NotificationSound for SystemSound {
    fn play(&self) {}
}

/// Silent sound implementation for tests and unsupported platforms.
#[derive(Debug, Default)]
pub struct NoopSound;

impl NotificationSound for NoopSound {
    fn play(&self) {}
}
