//! Opt-in GUI scripts from `FLEET_DRIVE`, with local-time `<script>.log` replies.
//! Commands: `key`, `type`, `wait`, `shot` (all displays), and `quit`.

use fleet_lazygit::drive::support::{self, Dialect};
use gpui::{App, Task, Window};
use std::path::PathBuf;

pub(crate) fn script_path() -> Option<PathBuf> {
    std::env::var_os("FLEET_DRIVE")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

pub(crate) fn spawn(script: PathBuf, window: &Window, cx: &App) -> Task<()> {
    support::spawn(script, window, cx, Dialect::Fleet, || {
        chrono::Local::now().format("%H:%M:%S%.3f").to_string()
    })
}
