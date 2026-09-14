//! Stable, serializable projection consumed only by the external test harness.
//!
//! The snapshot is a *pure projection* of [`super::AppState`]: every field is derived from state the
//! app already keeps, and nothing here is a second source of truth. `docs/TESTING-HARNESS.md`
//! §3 is the frozen contract for the shape and the vocabulary; `docs/APP-CONTRACTS.md`'s
//! "render prepares nothing" rule applies to it unchanged, so the builder is memoised behind a
//! revision and is never reachable from a `render` body.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::Serialize;

mod projection;
#[cfg(test)]
mod tests;

pub(super) use projection::HarnessCache;

/// The first and current frozen snapshot schema.
pub const SNAPSHOT_VERSION: u32 = 1;

/// Complete version-one UI snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiSnapshot {
    pub version: u32,
    pub screen: String,
    pub mode: String,
    pub hub_pane: Option<String>,
    pub hub_tab: Option<String>,
    pub overlay: Option<String>,
    pub key_contexts: Vec<String>,
    pub focused: Option<String>,
    pub lists: BTreeMap<String, ListSnapshot>,
    pub dialog: Option<DialogSnapshot>,
    pub toasts: Vec<ToastSnapshot>,
    pub sticky_error: Option<String>,
    pub jobs: Vec<JobSnapshot>,
    pub agents: AgentsSnapshot,
    pub terminal: Option<TerminalSnapshot>,
    pub targets: BTreeMap<String, TargetSnapshot>,
    pub daemon: DaemonSnapshot,
    pub idle: IdleSnapshot,
    pub window: WindowSnapshot,
}

impl UiSnapshot {
    /// The one-line summary `dump` echoes so a run reads without opening a file.
    #[must_use]
    pub fn summary(&self) -> String {
        let focused = self.focused.as_deref().unwrap_or("-");
        let selected = self
            .lists
            .values()
            .find_map(|list| list.selected.as_ref())
            .map_or("-", |row| row.label.as_str());
        format!(
            "screen={} mode={} focus={focused} selected={selected} idle={}",
            self.screen, self.mode, self.idle.idle
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListSnapshot {
    pub rows: Vec<RowSnapshot>,
    pub selected: Option<RowSnapshot>,
    pub filter: String,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowSnapshot {
    pub id: String,
    pub label: String,
    pub badges: Vec<String>,
    pub marks: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DialogSnapshot {
    pub name: String,
    pub fields: Vec<FieldSnapshot>,
    pub buttons: Vec<String>,
    pub message: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldSnapshot {
    pub name: String,
    pub value: String,
    pub focused: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToastSnapshot {
    pub level: String,
    pub text: String,
    pub count: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobSnapshot {
    pub id: String,
    pub status: String,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentsSnapshot {
    pub popup: Option<String>,
    pub threads: Vec<AgentThreadSnapshot>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentThreadSnapshot {
    pub id: String,
    pub provider: String,
    pub state: String,
    pub unread: bool,
    pub pending_gate: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerminalSnapshot {
    pub rows: Vec<String>,
    pub text: String,
    pub cursor: CursorSnapshot,
    pub viewport: ViewportSnapshot,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CursorSnapshot {
    pub row: u32,
    pub col: u32,
    pub shape: String,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewportSnapshot {
    pub top: u32,
    pub rows: u32,
    pub history: u32,
}
/// The daemon link, which `key_contexts` cannot always show.
///
/// `AppState::context_chain` appends `["Daemon", "Banner"]` only *after* a chain that can carry
/// it, so on a first-run Fleet or behind an open overlay a lost daemon is invisible to every
/// predicate. This reports it unconditionally, which is what lets a fault scenario assert on
/// the link itself rather than on the chrome that happens to be showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DaemonSnapshot {
    /// `starting`, `failed`, `connected`, `lost` or `reconnected`.
    pub link: &'static str,
    /// Failed reconnect attempts, zero outside `lost`.
    pub attempt: u32,
    /// Whether `Esc` dismissed the reconnect banner.
    pub dismissed: bool,
    /// Whether the daemon that came back is a *new* fleetd, so terminals did not survive.
    pub restarted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TargetSnapshot {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub frame: u64,
}

/// The five sources of pending work, plus the `idle` they derive.
///
/// `docs/TESTING-HARNESS.md` §2 defines `idle` as exactly "no in-flight app requests, no
/// running jobs, no pending frame, no live toast timer and no armed debounce". Each counter
/// ships beside the verdict so an `await idle` timeout names *which* half is still busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct IdleSnapshot {
    pub idle: bool,
    pub in_flight_requests: u32,
    pub running_jobs: u32,
    pub pending_frame: bool,
    pub live_toast_timers: u32,
    pub armed_debounces: u32,
}

impl IdleSnapshot {
    /// Builds the idle report with `idle` derived, never asserted.
    ///
    /// `docs/TESTING-HARNESS.md` defines `idle` as exactly "none of the five sources of pending
    /// work remains", so it is computed here rather than set by a caller that could contradict
    /// the counters it ships alongside.
    #[must_use]
    pub fn new(
        in_flight_requests: u32,
        running_jobs: u32,
        pending_frame: bool,
        live_toast_timers: u32,
        armed_debounces: u32,
    ) -> Self {
        Self {
            idle: in_flight_requests == 0
                && running_jobs == 0
                && !pending_frame
                && live_toast_timers == 0
                && armed_debounces == 0,
            in_flight_requests,
            running_jobs,
            pending_frame,
            live_toast_timers,
            armed_debounces,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WindowSnapshot {
    pub bounds: BoundsSnapshot,
    pub scale_factor: f32,
    pub title: String,
    pub frame: u64,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct BoundsSnapshot {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The harness-only half of [`super::AppState`]: what the app knows but does not otherwise keep.
///
/// Window geometry, recorded target rectangles and the three pending-work counters cannot be
/// derived from any existing field — the window belongs to gpui, targets to the element tree,
/// and "a request is in flight" to the bridge. They live here rather than being invented by the
/// builder, which keeps the projection pure. Everything is zero-sized until something writes it,
/// so an app started without the harness socket pays nothing.
#[derive(Debug, Default)]
pub struct HarnessState {
    window: WindowSnapshot,
    targets: BTreeMap<String, TargetSnapshot>,
    targets_revision: u64,
    /// Shared with [`crate::bridge::Bridge`], which claims a slot when a request is admitted and
    /// releases it on the runtime thread when the request is answered, rejected or shed. A cell
    /// rather than a mirrored count, because the two ends live on different threads.
    in_flight_requests: Arc<AtomicU32>,
    pending_frame: bool,
    /// Shared with the debounce guards, which are dropped from spawned tasks rather than from an
    /// update path, so the count has to live somewhere a `&mut AppState` is not needed to touch.
    armed_debounces: Arc<AtomicU32>,
}

/// One armed debounce window, disarmed when it fires or when its task is dropped.
///
/// The timers Fleet debounces behind — the clone-repo search and the Hub's auto-inspect — are
/// futures owned by a `Task` that a newer keystroke replaces. A guard is the only shape that
/// counts the replaced one down as well as the one that fires.
#[derive(Debug)]
pub struct ArmedDebounce(Option<Arc<AtomicU32>>);

impl ArmedDebounce {
    /// Disarms this window now. Idempotent, so a task may release it before the work it gated
    /// and still be dropped normally afterwards.
    pub fn disarm(&mut self) {
        if let Some(counter) = self.0.take() {
            // Every guard increments exactly once, so this can never wrap.
            counter.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for ArmedDebounce {
    fn drop(&mut self) {
        self.disarm();
    }
}

impl HarnessState {
    /// Records the window metrics of the frame that has just painted.
    pub fn set_window(
        &mut self,
        bounds: BoundsSnapshot,
        scale_factor: f32,
        title: impl Into<String>,
        frame: u64,
    ) {
        self.window = WindowSnapshot {
            bounds,
            scale_factor,
            title: title.into(),
            frame,
        };
    }

    /// The last recorded window metrics.
    #[must_use]
    pub fn window(&self) -> &WindowSnapshot {
        &self.window
    }

    /// Replaces the whole target table with one frame's painted rectangles.
    ///
    /// `fleet_ui_kit::harness` records targets into a thread-local table during `paint`, which
    /// the snapshot builder cannot reach — it holds `&AppState`, not `&Window`. The command
    /// that is about to answer a `dump` copies that frame's table in through here, in an update
    /// path. The revision only moves when the table actually changed, so a repainted but
    /// identical frame does not invalidate the memo.
    pub fn set_targets(&mut self, targets: BTreeMap<String, TargetSnapshot>) {
        if self.targets != targets {
            self.targets = targets;
            self.targets_revision = self.targets_revision.wrapping_add(1);
        }
    }

    /// The recorded targets, in name order.
    #[must_use]
    pub fn targets(&self) -> &BTreeMap<String, TargetSnapshot> {
        &self.targets
    }

    /// Reads the bridge's in-flight request counter through the shared cell.
    pub fn track_requests(&mut self, counter: Arc<AtomicU32>) {
        self.in_flight_requests = counter;
    }

    /// How many requests the bridge has admitted and not yet finished with.
    #[must_use]
    pub fn in_flight_requests(&self) -> u32 {
        self.in_flight_requests.load(Ordering::Acquire)
    }

    /// Records that one more app request is waiting for the daemon.
    ///
    /// The bridge claims and releases its own slots; this is the seam for a caller that owns a
    /// request the bridge cannot see, and the one the unit tests drive.
    pub fn begin_request(&self) {
        self.in_flight_requests.fetch_add(1, Ordering::AcqRel);
    }

    /// Records that one request answered, failed, or was abandoned.
    pub fn finish_request(&self) {
        let _ =
            self.in_flight_requests
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_sub(1))
                });
    }

    /// Records whether a frame the harness must wait for has been requested.
    pub fn set_pending_frame(&mut self, pending: bool) {
        self.pending_frame = pending;
    }

    /// Arms one debounce window. Dropping the returned guard disarms it.
    #[must_use]
    pub fn arm_debounce(&self) -> ArmedDebounce {
        self.armed_debounces.fetch_add(1, Ordering::AcqRel);
        ArmedDebounce(Some(Arc::clone(&self.armed_debounces)))
    }

    /// How many debounce windows are armed.
    #[must_use]
    pub fn armed_debounces(&self) -> u32 {
        self.armed_debounces.load(Ordering::Acquire)
    }
}

/// The memoised projection and the revision it was built at.
///
/// `await` compares revisions: two dumps with no intervening state change carry the same one,
/// and a changed revision is the only signal that re-evaluating a predicate is worth it.
#[derive(Debug, Clone)]
pub struct HarnessProjection {
    /// Bumped only when the projected content actually differs from the previous one.
    pub revision: u64,
    /// The snapshot itself, shared rather than copied.
    pub snapshot: Rc<UiSnapshot>,
}
