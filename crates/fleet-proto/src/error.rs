//! Stable protocol error codes and payloads.

use serde::{Deserialize, Serialize};

/// Stable error category used by CLI JSON envelopes and protocol responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorKind {
    /// A requested entity does not exist.
    NotFound,
    /// Existing state conflicts with the requested change.
    Conflict,
    /// A Git operation failed.
    Git,
    /// A terminal-session operation failed; serialized as swarm's `tmux` name.
    #[serde(rename = "tmux")]
    Tmux,
    /// A filesystem operation failed.
    Fs,
    /// A GitHub operation failed.
    Github,
    /// A remote-host operation failed.
    Remote,
    /// User input or persisted data failed validation.
    Validation,
    /// An explicit cancellation stopped the operation.
    Cancelled,
    /// The requested operation is unsupported.
    Unsupported,
    /// No more specific category is available.
    Unknown,
}

/// Serializable protocol error payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoError {
    /// Stable machine-readable category.
    pub kind: ErrorKind,
    /// Human-readable single-line detail.
    pub message: String,
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ProtoError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_round_trip;

    #[test]
    fn error_kind_round_trips_with_swarm_names() {
        assert_eq!(
            serde_json::to_string(&ErrorKind::Tmux).unwrap_or_else(|error| panic!("{error}")),
            "\"tmux\""
        );
        assert_round_trip(ErrorKind::Tmux);
    }
}
