//! Fleet's length-prefixed JSON wire contract between clients and the daemon, covering requests, responses, events, snapshots, jobs, terminal frames, errors, and socket paths.

/// Current wire protocol version spoken by compatible Fleet clients and daemons.
pub const PROTOCOL_VERSION: u32 = 3;

pub mod codec;
pub mod error;
pub mod event;
pub mod job;
pub mod paths;
pub mod request;
pub mod response;
pub mod snapshot;
pub mod terminal;
pub mod watch;
