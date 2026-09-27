//! Shared limits and base64 conversion for staged media uploads.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use thiserror::Error;

/// Maximum decoded bytes in one upload chunk (128 KiB).
///
/// Base64 expands this to at most 174,764 bytes (about 171 KiB), comfortably below the
/// 16 MiB frame limit and about 0.14 seconds of writing on a 10 Mbit/s link, far below the
/// 10-second write budget. A four-chunk host window therefore puts at most 699,056 encoded bytes
/// (about 683 KiB) ahead of interactive traffic, or roughly 0.56 seconds on that link.
pub const CHUNK_BYTES: usize = 128 * 1024;

/// Maximum chunks in flight to one host across all uploads.
pub const UPLOAD_WINDOW: usize = 4;

/// Maximum live uploads one daemon connection accepts for one owner.
pub const MAX_ACTIVE_UPLOADS_PER_OWNER: usize = 16;

/// App-side upload workers per destination, leaving daemon capacity for another client gesture.
pub const UPLOAD_CONCURRENCY: usize = 12;

/// Maximum total decoded bytes in one staged file or directory (1 GiB).
pub const MAX_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// Maximum file and directory manifest entries in one staged upload.
pub const MAX_UPLOAD_FILES: usize = 10_000;

/// Time an inactive partial upload remains resumable on the receiving daemon.
pub const UPLOAD_IDLE_EXPIRY: Duration = Duration::from_secs(3 * 60);

const MAX_ENCODED_CHUNK_BYTES: usize = CHUNK_BYTES.div_ceil(3) * 4;

/// Failure to decode a staged-media chunk.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// The encoded representation cannot describe a valid bounded chunk.
    #[error("encoded media chunk is {actual} bytes; maximum is {maximum}")]
    EncodedChunkTooLarge {
        /// Encoded bytes supplied by the peer.
        actual: usize,
        /// Largest accepted encoded representation.
        maximum: usize,
    },
    /// The base64 payload is malformed.
    #[error("invalid base64 media chunk: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    /// The decoded payload exceeds the chunk limit.
    #[error("decoded media chunk is {actual} bytes; maximum is {maximum}")]
    DecodedChunkTooLarge {
        /// Decoded bytes supplied by the peer.
        actual: usize,
        /// Largest accepted decoded chunk.
        maximum: usize,
    },
}

/// Encodes one media chunk for the JSON wire format.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decodes one media chunk, rejecting an oversized encoded value before allocating its output.
pub fn decode(data: &str) -> Result<Vec<u8>, DecodeError> {
    if data.len() > MAX_ENCODED_CHUNK_BYTES {
        return Err(DecodeError::EncodedChunkTooLarge {
            actual: data.len(),
            maximum: MAX_ENCODED_CHUNK_BYTES,
        });
    }

    let bytes = STANDARD.decode(data)?;
    if bytes.len() > CHUNK_BYTES {
        return Err(DecodeError::DecodedChunkTooLarge {
            actual: bytes.len(),
            maximum: CHUNK_BYTES,
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_at_the_limit_round_trips() {
        let bytes = vec![0xa5; CHUNK_BYTES];
        let encoded = encode(&bytes);

        assert_eq!(
            decode(&encoded).unwrap_or_else(|error| panic!("{error}")),
            bytes
        );
    }

    #[test]
    fn encoded_oversize_is_rejected_before_decoding() {
        let encoded = "!".repeat(MAX_ENCODED_CHUNK_BYTES + 1);

        assert!(matches!(
            decode(&encoded),
            Err(DecodeError::EncodedChunkTooLarge { .. })
        ));
    }

    #[test]
    fn decoded_oversize_is_rejected() {
        let encoded = "A".repeat(MAX_ENCODED_CHUNK_BYTES);

        assert!(matches!(
            decode(&encoded),
            Err(DecodeError::DecodedChunkTooLarge { .. })
        ));
    }

    #[test]
    fn malformed_base64_is_rejected() {
        assert!(matches!(
            decode("not base64"),
            Err(DecodeError::InvalidBase64(_))
        ));
    }
}
