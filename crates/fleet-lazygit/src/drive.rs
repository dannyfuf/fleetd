//! Opt-in GUI scripts from `FLEET_LAZYGIT_DRIVE`, with timestamped `<script>.log` replies.
//! Commands: `key`, `type`, `wheel`, `hwheel`, `wait`, `shot`, and `quit`.

pub mod support;

use gpui::{App, Task, Window};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// The nonempty opt-in script path, when configured.
pub fn script_path() -> Option<PathBuf> {
    std::env::var_os("FLEET_LAZYGIT_DRIVE")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Runs the standalone driver until quit or window closure.
pub fn spawn(script: PathBuf, window: &Window, cx: &App) -> Task<()> {
    support::spawn(script, window, cx, support::Dialect::Lazygit, || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default()
            .to_string()
    })
}
