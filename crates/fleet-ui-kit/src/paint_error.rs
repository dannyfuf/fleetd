use std::sync::atomic::{AtomicBool, Ordering};

// Font failures can repeat for every cell on every frame. Report the first failure only.
pub(crate) fn log_once(error: &dyn std::fmt::Display) {
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if !REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("fleet-ui-kit: text paint failed (further paint errors suppressed): {error}");
    }
}
