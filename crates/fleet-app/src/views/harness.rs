//! Composite harness target names, formatted only while the harness is recording.
//!
//! `docs/TESTING-HARNESS.md` §3 fixes target names at `<surface>.<part>[<index>]`. Most of
//! Fleet's names are either fixed (`sticky_error.retry`) or one part plus one index
//! (`worktrees.row[2]`), and [`fleet_ui_kit::HarnessTargetExt`] already carries both cheaply:
//! `harness_target` takes a `&'static str`, and `harness_target_indexed` formats its index only
//! while recording.
//!
//! One name in the frozen vocabulary has *two* indices — `board.column[0].card[2]` — and the
//! kit deliberately offers no builder for it, because a `format!` per card per frame is exactly
//! what `gpui-performance` forbids in a paint path. [`name`] is the escape hatch: the closure
//! runs only when [`fleet_ui_kit::harness::is_recording`] is true, so production pays one
//! thread-local `bool` read and nothing else.

use gpui::SharedString;

/// The name a composite target is recorded under, built only while the harness is recording.
///
/// Off-harness this returns an empty name that is never read: the kit's recorder branches on
/// the same flag before it touches the name at all.
///
/// ```ignore
/// tile.harness_target(harness::name(|| format!("board.column[{column}].card[{row}]")))
/// ```
#[must_use]
pub(crate) fn name(build: impl FnOnce() -> String) -> SharedString {
    if fleet_ui_kit::harness::is_recording() {
        SharedString::from(build())
    } else {
        SharedString::new_static("")
    }
}
