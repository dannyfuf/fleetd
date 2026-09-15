//! Deterministic ids and timestamps.
//!
//! Nothing here is random and nothing reads the clock. A fixture preset's promise is that two
//! runs produce identical dumps, and a scripted agent that minted a fresh UUID per message would
//! break that promise on every frame it wrote — so every id is a counter rendered into the shape
//! its protocol expects, and time advances by a fixed tick per frame.

/// The epoch every scripted run starts at: 2026-09-11T00:00:00Z, in milliseconds.
///
/// A fixed origin rather than `SystemTime::now()`, for the reason above. It is in the past, so a
/// consumer that treats a timestamp as "recent" still sees a plausible value.
const ORIGIN_MS: i64 = 1_789_084_800_000;

/// How far the scripted clock moves per stamped frame.
const TICK_MS: i64 = 7;

/// A counter that mints every id and timestamp one scripted session needs.
#[derive(Debug, Default)]
pub(crate) struct Ids {
    next: u64,
}

impl Ids {
    /// A fresh counter.
    pub(crate) const fn new() -> Self {
        Self { next: 0 }
    }

    /// The next raw ordinal.
    fn bump(&mut self) -> u64 {
        self.next = self.next.saturating_add(1);
        self.next
    }

    /// A UUIDv4-shaped id with `prefix` folded into it.
    ///
    /// Claude's `uuid` fields and resume cursor are UUIDs, and `argv::resume_cursor` drops a
    /// stored cursor whose version nibble is not 4 — so the shape matters even though the value
    /// is a counter. The `4` and the `8` are the version and variant nibbles.
    pub(crate) fn uuid(&mut self, prefix: u32) -> String {
        let ordinal = self.bump();
        format!("{prefix:08x}-0000-4000-8000-{ordinal:012x}")
    }

    /// A Claude `tool_use` id.
    pub(crate) fn tool_use(&mut self) -> String {
        format!("toolu_{:016x}", self.bump())
    }

    /// A Claude message id.
    pub(crate) fn message(&mut self) -> String {
        format!("msg_{:016x}", self.bump())
    }

    /// A Codex item id.
    pub(crate) fn item(&mut self) -> String {
        format!("item_{:012x}", self.bump())
    }

    /// A Codex turn id.
    pub(crate) fn turn(&mut self) -> String {
        format!("01999c4a-7f00-7001-8000-{:012x}", self.bump())
    }

    /// The next timestamp, in milliseconds since the Unix epoch.
    pub(crate) fn stamp_ms(&mut self) -> i64 {
        ORIGIN_MS + TICK_MS * i64::try_from(self.bump()).unwrap_or(i64::MAX)
    }

    /// The next timestamp, in whole seconds.
    pub(crate) fn stamp_secs(&mut self) -> i64 {
        self.stamp_ms() / 1_000
    }
}
