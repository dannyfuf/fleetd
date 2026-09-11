//! Fleet's length-prefixed JSON wire contract between clients and the daemon, covering requests, responses, events, snapshots, jobs, terminal frames, and errors.

/// Current wire protocol version spoken by compatible Fleet clients and daemons.
pub const PROTOCOL_VERSION: u32 = 7;

/// Capability advertised by daemons that support machine federation.
pub const REMOTE_MACHINES_CAPABILITY: &str = "remote-machines";

/// Capability advertised by daemons that serve bounded transcript windows and paginate history.
///
/// A client must never send a window field — `turn_limit`, `after_seq`, `before_cursor`,
/// `request_sync_marker` — to a daemon that did not advertise this: an older daemon ignores the
/// unknown fields and answers the *whole* transcript, which is precisely the frame the window
/// exists to avoid. Capabilities are per connection and reset to `false` on disconnect, so the
/// check is made per request and never cached across a reconnect.
pub const AGENT_WINDOW_CAPABILITY: &str = "agent.window";

/// Capability advertised by daemons that emit an explicit catch-up completion marker.
///
/// Without it a client cannot distinguish "still replaying history" from "live", and a mirrored
/// thread has no honest transition out of its cached state.
pub const AGENT_SYNC_MARKER_CAPABILITY: &str = "agent.sync_marker";

/// Capability advertised by daemons that report a live-stream budget overflow as a resumable
/// resync instead of dropping the subscriber.
pub const AGENT_RESYNC_CAPABILITY: &str = "agent.resync";

/// Capability advertised by daemons that can serve a stored item body in offset ranges.
pub const AGENT_ITEM_BODY_CAPABILITY: &str = "agent.item_body";

/// Capability advertised by daemons that keep Fleet-owned turn checkpoints and can revert one.
///
/// Gating matters more here than for a read-only addition: a client that assumes checkpoints
/// exist draws `[u] revert turn` on every turn footer, and a `[u]` that answers "unsupported" is
/// the affordance `DESIGN-SYSTEM.md` §7 forbids. A daemon advertising this has the service, the
/// `refs/fleet/checkpoints/` namespace, and the garbage collector behind it.
pub const AGENT_CHECKPOINTS_CAPABILITY: &str = "agent.checkpoints";

/// Capability advertised by daemons that can run the Codex app-server harness.
///
/// This is what a mixed-version fleet is gated on rather than a `PROTOCOL_VERSION` bump: the
/// daemon matches the version literally and a remote link requires an exact match, so a bump
/// would force every machine to upgrade in lockstep for a harness only one of them can run.
pub const AGENT_CODEX_CAPABILITY: &str = "agent.codex";

/// Every `agent.*` capability this build implements, in advertisement order.
///
/// Both directions publish it. A daemon puts it in `HelloResponse.capabilities`, and a client
/// puts it in [`HelloClient::capabilities`](request::HelloClient::capabilities) — negotiation
/// runs both ways because an adjacently tagged `Event` variant a peer has no arm for kills its
/// whole frame, so the daemon must know which event families this connection can decode before
/// it sends one. Either side asks `supports` for the single behaviour it is about to use, never
/// for the slice.
pub const AGENT_CAPABILITIES: &[&str] = &[
    AGENT_WINDOW_CAPABILITY,
    AGENT_SYNC_MARKER_CAPABILITY,
    AGENT_RESYNC_CAPABILITY,
    AGENT_ITEM_BODY_CAPABILITY,
    AGENT_CHECKPOINTS_CAPABILITY,
    AGENT_CODEX_CAPABILITY,
];

pub mod agents;
pub mod codec;
pub mod error;
pub mod event;
pub mod job;
pub mod request;
pub mod response;
pub mod snapshot;
pub mod terminal;
pub mod watch;

/// Board mutation event reasons.
pub use event::BoardChangeReason;

/// Asserts a protocol value survives a JSON round trip, the invariant every message
/// type on this wire must hold.
#[cfg(test)]
fn assert_round_trip<T>(value: T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(&value).unwrap_or_else(|error| panic!("{error}"));
    let decoded: T = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(decoded, value);
}
