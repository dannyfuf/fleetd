//! Length-prefixed JSON frame encoding and decoding.

use std::{io, marker::PhantomData};

use bytes::{Buf, BufMut, BytesMut};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder};

/// Maximum accepted JSON payload size, excluding the four-byte prefix.
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Failure while encoding or decoding a Fleet protocol frame.
#[derive(Debug, Error)]
pub enum CodecError {
    /// Buffer management or transport error.
    #[error("protocol I/O error: {0}")]
    Io(#[from] io::Error),
    /// JSON serialization or deserialization error.
    #[error("invalid protocol JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Payload exceeds the fixed protocol safety bound.
    #[error("protocol frame is {actual} bytes; maximum is {max}")]
    FrameTooLarge {
        /// Actual or announced payload size.
        actual: usize,
        /// Maximum permitted payload size.
        max: usize,
    },
}

/// Tokio codec for u32-big-endian-length-prefixed JSON messages.
#[derive(Debug)]
pub struct FleetCodec<EncodeItem, DecodeItem> {
    marker: PhantomData<fn(EncodeItem) -> DecodeItem>,
}

impl<EncodeItem, DecodeItem> FleetCodec<EncodeItem, DecodeItem> {
    /// Creates an empty protocol codec.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            marker: PhantomData,
        }
    }
}

impl<EncodeItem, DecodeItem> Default for FleetCodec<EncodeItem, DecodeItem> {
    fn default() -> Self {
        Self::new()
    }
}

impl<EncodeItem, DecodeItem> Encoder<EncodeItem> for FleetCodec<EncodeItem, DecodeItem>
where
    EncodeItem: Serialize,
{
    type Error = CodecError;

    fn encode(&mut self, item: EncodeItem, destination: &mut BytesMut) -> Result<(), Self::Error> {
        let payload = serde_json::to_vec(&item)?;
        if payload.len() > MAX_FRAME_SIZE {
            return Err(CodecError::FrameTooLarge {
                actual: payload.len(),
                max: MAX_FRAME_SIZE,
            });
        }
        let length = u32::try_from(payload.len()).map_err(|_| CodecError::FrameTooLarge {
            actual: payload.len(),
            max: MAX_FRAME_SIZE,
        })?;
        destination.reserve(4 + payload.len());
        destination.put_u32(length);
        destination.extend_from_slice(&payload);
        Ok(())
    }
}

impl<EncodeItem, DecodeItem> Decoder for FleetCodec<EncodeItem, DecodeItem>
where
    DecodeItem: DeserializeOwned,
{
    type Item = DecodeItem;
    type Error = CodecError;

    fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if source.len() < 4 {
            source.reserve(4 - source.len());
            return Ok(None);
        }
        let length = u32::from_be_bytes([source[0], source[1], source[2], source[3]]) as usize;
        if length > MAX_FRAME_SIZE {
            return Err(CodecError::FrameTooLarge {
                actual: length,
                max: MAX_FRAME_SIZE,
            });
        }
        let frame_length = 4 + length;
        if source.len() < frame_length {
            source.reserve(frame_length - source.len());
            return Ok(None);
        }
        source.advance(4);
        let payload = source.split_to(length);
        Ok(Some(serde_json::from_slice(&payload)?))
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Message {
        value: String,
    }

    #[test]
    fn round_trips_a_frame_incrementally() {
        let mut codec = FleetCodec::<Message, Message>::new();
        let mut encoded = BytesMut::new();
        codec
            .encode(
                Message {
                    value: "hello".to_owned(),
                },
                &mut encoded,
            )
            .unwrap_or_else(|error| panic!("{error}"));
        let tail = encoded.split_off(3);
        assert!(
            codec
                .decode(&mut encoded)
                .unwrap_or_else(|error| panic!("{error}"))
                .is_none()
        );
        encoded.extend_from_slice(&tail);
        let decoded = codec
            .decode(&mut encoded)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            decoded,
            Some(Message {
                value: "hello".to_owned()
            })
        );
        assert!(encoded.is_empty());
    }

    #[test]
    fn rejects_announced_oversized_frames() {
        let mut bytes = BytesMut::new();
        bytes.put_u32((MAX_FRAME_SIZE as u32) + 1);
        let mut codec = FleetCodec::<Message, Message>::new();
        assert!(matches!(
            codec.decode(&mut bytes),
            Err(CodecError::FrameTooLarge { .. })
        ));
    }
}
