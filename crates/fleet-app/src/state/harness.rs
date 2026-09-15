//! Stable, serializable projection consumed by the external test harness, plus idle accounting
//! shared with the daemon bridge.
//!
//! The snapshot is a *pure projection* of [`super::AppState`]: fields derive from app state; the
//! Jobs panel remains authoritative while `JobsPanelMirror` synchronizes its cursor and filter for
//! projection. `docs/TESTING-HARNESS.md`
//! §3 is the frozen contract for the shape and the vocabulary; `docs/APP-CONTRACTS.md`'s
//! "render prepares nothing" rule applies to it unchanged, so the builder is memoised behind a
//! revision and is never reachable from a `render` body. The bridge-owned atomics and wake callback
//! below are the narrow cross-thread seam that lets this projection report quiescence accurately.

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

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

/// The seven sources of pending work, plus the `idle` they derive.
///
/// `docs/TESTING-HARNESS.md` §2 defines `idle` from all seven pending-work inputs. Each input
/// ships beside the verdict so an `await idle` timeout names *which* half is still busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct IdleSnapshot {
    pub idle: bool,
    pub in_flight_requests: u32,
    pub running_jobs: u32,
    pub pending_frame: bool,
    pub live_toast_timers: u32,
    pub armed_debounces: u32,
    pub settling_mutations: u32,
    pub link_opening: bool,
}

impl IdleSnapshot {
    /// Builds the idle report with `idle` derived, never asserted.
    ///
    /// `docs/TESTING-HARNESS.md` defines `idle` as exactly "none of the seven sources of pending
    /// work remains", so it is computed here rather than set by a caller that could contradict
    /// the counters it ships alongside.
    #[must_use]
    pub fn new(
        in_flight_requests: u32,
        running_jobs: u32,
        pending_frame: bool,
        live_toast_timers: u32,
        armed_debounces: u32,
        settling_mutations: u32,
        link_opening: bool,
    ) -> Self {
        Self {
            idle: in_flight_requests == 0
                && running_jobs == 0
                && !pending_frame
                && live_toast_timers == 0
                && armed_debounces == 0
                && settling_mutations == 0
                && !link_opening,
            in_flight_requests,
            running_jobs,
            pending_frame,
            live_toast_timers,
            armed_debounces,
            settling_mutations,
            link_opening,
        }
    }
}

/// Wakes the harness waiter from any thread. Installed by the bridge; a no-op in production.
#[derive(Clone)]
pub struct IdleWake(Arc<dyn Fn() + Send + Sync>);

impl IdleWake {
    /// Wraps a thread-safe wake callback.
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Notifies the installed harness waiter.
    pub fn wake(&self) {
        (self.0)();
    }
}

impl fmt::Debug for IdleWake {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IdleWake(..)")
    }
}

/// Mutations whose daemon reply has arrived but whose snapshot the shell has not yet applied.
#[derive(Debug, Default)]
pub struct SettleCounter {
    pending: AtomicU32,
    generation: AtomicU64,
}

impl SettleCounter {
    const LOCKED: u64 = 1 << 63;

    /// The daemon answered one mutation. Returns the generation to hand to `expire`.
    pub fn begin(&self) -> u64 {
        let generation = self.lock_generation();
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.unlock_generation(generation);
        generation
    }

    /// The shell applied a snapshot: every pending mutation is covered.
    pub fn settled(&self) {
        let generation = self.lock_generation();
        self.pending.store(0, Ordering::Release);
        self.unlock_generation(generation.wrapping_add(1) & !Self::LOCKED);
    }

    /// Releases one mutation only when no snapshot has been applied since it began.
    pub fn expire(&self, generation: u64) -> bool {
        if !self.lock_generation_if(generation) {
            return false;
        }
        let released = self
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                pending.checked_sub(1)
            })
            .is_ok();
        self.unlock_generation(generation);
        released
    }

    /// Mutations still waiting for their snapshot or grace expiry.
    pub fn pending(&self) -> u32 {
        self.pending.load(Ordering::Acquire)
    }

    fn lock_generation(&self) -> u64 {
        loop {
            let generation = self.generation.load(Ordering::Acquire);
            if generation & Self::LOCKED != 0 {
                std::hint::spin_loop();
                continue;
            }
            if self
                .generation
                .compare_exchange_weak(
                    generation,
                    generation | Self::LOCKED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return generation;
            }
        }
    }

    fn lock_generation_if(&self, expected: u64) -> bool {
        loop {
            let generation = self.generation.load(Ordering::Acquire);
            if generation & Self::LOCKED != 0 {
                std::hint::spin_loop();
                continue;
            }
            if generation != expected {
                return false;
            }
            if self
                .generation
                .compare_exchange_weak(
                    generation,
                    generation | Self::LOCKED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    fn unlock_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }
}

/// Maximum time a replied mutation waits for a following snapshot before settling itself.
pub const MUTATION_SETTLE_GRACE: Duration = Duration::from_millis(250);
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
/// Window geometry, recorded target rectangles and the pending-work counters cannot be
/// derived from any existing field — the window belongs to gpui, targets to the element tree,
/// and "a request is in flight" to the bridge. They live here rather than being invented by the
/// builder, which keeps the projection pure. Everything is zero-sized until something writes it,
/// so an app started without the harness socket pays nothing.
#[derive(Debug, Default)]
pub struct HarnessState {
    window: WindowSnapshot,
    targets: BTreeMap<String, TargetSnapshot>,
    targets_revision: u64,
    /// Shared with [`crate::bridge::Bridge`], which claims a slot when a request is admitted.
    /// Mutation replies transfer their claim to `settle` until the following snapshot or grace
    /// expiry; reply-lane claims remain until their receiver closes. Rejected and shed requests
    /// release directly. A cell rather than a mirrored count, because the two ends live on
    /// different threads.
    in_flight_requests: Arc<AtomicU32>,
    settle: Arc<SettleCounter>,
    wake: Option<IdleWake>,
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
pub struct ArmedDebounce {
    counter: Option<Arc<AtomicU32>>,
    wake: Option<IdleWake>,
}

/// A bridge request claim used only by state tests.
#[cfg(test)]
#[derive(Debug)]
pub(super) struct RequestClaim {
    counter: Arc<AtomicU32>,
    wake: Option<IdleWake>,
}

#[cfg(test)]
impl Drop for RequestClaim {
    fn drop(&mut self) {
        // Every claim increments exactly once, so this can never wrap.
        self.counter.fetch_sub(1, Ordering::AcqRel);
        if let Some(wake) = self.wake.take() {
            wake.wake();
        }
    }
}

impl ArmedDebounce {
    /// Disarms this window now. Idempotent, so a task may release it before the work it gated
    /// and still be dropped normally afterwards.
    pub fn disarm(&mut self) {
        let Some(counter) = self.counter.take() else {
            return;
        };
        // Every guard increments exactly once, so this can never wrap.
        counter.fetch_sub(1, Ordering::AcqRel);
        if let Some(wake) = self.wake.take() {
            wake.wake();
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

    /// Installs the bridge-owned idle counters and wake callback.
    pub fn attach_bridge(
        &mut self,
        in_flight: Arc<AtomicU32>,
        settle: Arc<SettleCounter>,
        wake: IdleWake,
    ) {
        self.in_flight_requests = in_flight;
        self.settle = settle;
        self.wake = Some(wake);
    }

    /// How many requests the bridge has admitted and not yet finished with.
    #[must_use]
    pub fn in_flight_requests(&self) -> u32 {
        self.in_flight_requests.load(Ordering::Acquire)
    }

    /// Claims a request for state tests without exposing paired increment/decrement methods.
    #[cfg(test)]
    pub(super) fn claim_request(&self) -> RequestClaim {
        self.in_flight_requests.fetch_add(1, Ordering::AcqRel);
        RequestClaim {
            counter: Arc::clone(&self.in_flight_requests),
            wake: self.wake.clone(),
        }
    }

    /// The bridge counter whose claims await a following snapshot.
    #[must_use]
    pub fn settle(&self) -> &Arc<SettleCounter> {
        &self.settle
    }

    /// Mutations still waiting for the shell to apply their snapshot.
    #[must_use]
    pub fn settling_mutations(&self) -> u32 {
        self.settle.pending()
    }

    /// Records whether a frame the harness must wait for has been requested.
    pub fn set_pending_frame(&mut self, pending: bool) {
        self.pending_frame = pending;
    }

    /// Arms one debounce window. Dropping the returned guard disarms it.
    #[must_use]
    pub fn arm_debounce(&self) -> ArmedDebounce {
        self.armed_debounces.fetch_add(1, Ordering::AcqRel);
        ArmedDebounce {
            counter: Some(Arc::clone(&self.armed_debounces)),
            wake: self.wake.clone(),
        }
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

#[cfg(test)]
mod contract_tests {
    use super::tests::state;
    use super::*;

    fn idle_snapshot(harness: &HarnessState, link_opening: bool) -> IdleSnapshot {
        IdleSnapshot::new(
            harness.in_flight_requests(),
            0,
            false,
            0,
            harness.armed_debounces(),
            harness.settling_mutations(),
            link_opening,
        )
    }

    #[test]
    fn a_settled_generation_cannot_be_released_again_by_its_expiry() {
        let mut app = state();
        app.harness.attach_bridge(
            Arc::new(AtomicU32::new(0)),
            Arc::new(SettleCounter::default()),
            IdleWake::new(|| {}),
        );

        let generation = app.harness.settle().begin();
        assert_eq!(app.harness.settling_mutations(), 1);
        assert!(!idle_snapshot(&app.harness, false).idle);

        app.harness.settle().settled();
        assert_eq!(app.harness.settling_mutations(), 0);
        assert!(idle_snapshot(&app.harness, false).idle);
        assert!(!app.harness.settle().expire(generation));
        assert_eq!(app.harness.settling_mutations(), 0);
    }

    #[test]
    fn expiry_before_any_settlement_releases_its_mutation() {
        let settle = SettleCounter::default();
        let generation = settle.begin();

        assert!(settle.expire(generation));
        assert_eq!(settle.pending(), 0);
        assert!(!settle.expire(generation));
    }

    #[test]
    fn dropping_an_armed_debounce_invokes_the_wake_exactly_once() {
        let wakes = Arc::new(AtomicU32::new(0));
        let callback_wakes = Arc::clone(&wakes);
        let mut app = state();
        app.harness.attach_bridge(
            Arc::new(AtomicU32::new(0)),
            Arc::new(SettleCounter::default()),
            IdleWake::new(move || {
                callback_wakes.fetch_add(1, Ordering::AcqRel);
            }),
        );

        let debounce = app.harness.arm_debounce();
        assert_eq!(app.harness.armed_debounces(), 1);
        drop(debounce);

        assert_eq!(app.harness.armed_debounces(), 0);
        assert_eq!(wakes.load(Ordering::Acquire), 1);
    }

    #[test]
    fn an_opening_link_defeats_idle_with_every_counter_clear() {
        let app = state();

        let snapshot = idle_snapshot(&app.harness, true);

        assert_eq!(snapshot.in_flight_requests, 0);
        assert_eq!(snapshot.running_jobs, 0);
        assert!(!snapshot.pending_frame);
        assert_eq!(snapshot.live_toast_timers, 0);
        assert_eq!(snapshot.armed_debounces, 0);
        assert_eq!(snapshot.settling_mutations, 0);
        assert!(snapshot.link_opening);
        assert!(!snapshot.idle);
    }

    #[test]
    fn idle_snapshot_serializes_fields_in_contract_order() {
        let snapshot = IdleSnapshot::new(1, 2, true, 3, 4, 5, true);

        assert_eq!(
            serde_json::to_string(&snapshot).expect("IdleSnapshot serialization must succeed"),
            r#"{"idle":false,"in_flight_requests":1,"running_jobs":2,"pending_frame":true,"live_toast_timers":3,"armed_debounces":4,"settling_mutations":5,"link_opening":true}"#
        );
    }

    #[test]
    fn the_test_request_claim_releases_and_wakes_on_drop() {
        let wakes = Arc::new(AtomicU32::new(0));
        let callback_wakes = Arc::clone(&wakes);
        let mut app = state();
        app.harness.attach_bridge(
            Arc::new(AtomicU32::new(0)),
            Arc::new(SettleCounter::default()),
            IdleWake::new(move || {
                callback_wakes.fetch_add(1, Ordering::AcqRel);
            }),
        );

        let claim = app.harness.claim_request();
        assert_eq!(app.harness.in_flight_requests(), 1);
        drop(claim);

        assert_eq!(app.harness.in_flight_requests(), 0);
        assert_eq!(wakes.load(Ordering::Acquire), 1);
    }
}
