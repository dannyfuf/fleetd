//! Fleet's length-prefixed JSON wire contract between clients and the daemon, covering requests, responses, events, snapshots, jobs, terminal frames, and errors.

/// Current wire protocol version spoken by compatible Fleet clients and daemons.
pub const PROTOCOL_VERSION: u32 = 7;

/// Capability advertised by daemons that support machine federation.
pub const REMOTE_MACHINES_CAPABILITY: &str = "remote-machines";

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
