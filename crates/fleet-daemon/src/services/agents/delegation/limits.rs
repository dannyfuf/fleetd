//! Limits and retry intervals shared by delegation operations.
use std::time::Duration;

/// Deepest delegation chain a caller may extend.
pub(crate) const MAX_DEPTH: u8 = 3;
/// Live children one caller may hold at once.
pub(crate) const MAX_LIVE_CHILDREN_PER_CALLER: usize = 4;
/// Live delegations this daemon may hold at once.
pub(crate) const MAX_LIVE_DELEGATIONS: usize = 8;
/// Completion nudges a settled child is sent before it is called `Incomplete`.
pub(crate) const MAX_NUDGES: u8 = 2;
/// How long a reported child's background work is waited on before it is finalized anyway.
pub(crate) const SETTLE_GRACE: Duration = Duration::from_secs(30);
/// How often the worker re-drains the outbox so a transient failure heals without a wake.
pub(crate) const RETRY_TICK: Duration = Duration::from_secs(60);
/// Ceiling on a reported result, matching the transcript item-body budget.
pub(crate) const RESULT_CAP_BYTES: usize = fleet_proto::agents::ITEM_BODY_MAX_CHUNK_BYTES as usize;
/// Default and ceiling, in seconds, for `fleet subagent wait`.
#[allow(dead_code)] // The cli-subagent stage applies this default before sending the wait request.
pub(crate) const WAIT_DEFAULT_SECS: u64 = 540;

#[cfg(test)]
mod tests {
    use super::MAX_LIVE_DELEGATIONS;
    use fleet_core::board::MAX_LIVE_RUNS_PER_BOARD;

    #[test]
    fn a_board_may_never_ask_for_more_live_runs_than_this_daemon_will_hold() {
        // `fleet-core` validates `max_live_runs` against its own ceiling because the refusal
        // sentence has to be printable by the app and the CLI without a daemon. The two
        // constants are therefore written twice, and a board allowed more live runs than the
        // daemon will hold delegations would queue work the daemon then refuses.
        assert_eq!(MAX_LIVE_DELEGATIONS, MAX_LIVE_RUNS_PER_BOARD as usize);
    }
}
