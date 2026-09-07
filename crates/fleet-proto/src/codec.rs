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
        let frame_start = destination.len();
        destination.put_u32(0);
        let mut writer = FrameWriter {
            destination,
            payload_length: 0,
        };
        let result = serde_json::to_writer(&mut writer, &item);
        let length = writer.payload_length;
        if let Err(error) = result {
            roll_back(destination, frame_start);
            return Err(CodecError::Json(error));
        }
        if length > MAX_FRAME_SIZE {
            roll_back(destination, frame_start);
            return Err(CodecError::FrameTooLarge {
                actual: length,
                max: MAX_FRAME_SIZE,
            });
        }
        // MAX_FRAME_SIZE fits in the u32 wire prefix.
        destination[frame_start..frame_start + 4].copy_from_slice(&(length as u32).to_be_bytes());
        Ok(())
    }
}

/// Capacity a rejected frame may leave attached to a connection's write buffer.
const RETAINED_CAPACITY: usize = 64 * 1024;

/// Drops a partially written frame, releasing the capacity a rejected frame reserved.
fn roll_back(destination: &mut BytesMut, frame_start: usize) {
    destination.truncate(frame_start);
    if destination.capacity() > RETAINED_CAPACITY {
        let retained = BytesMut::from(&destination[..]);
        *destination = retained;
    }
}

struct FrameWriter<'a> {
    destination: &'a mut BytesMut,
    payload_length: usize,
}

impl io::Write for FrameWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.payload_length = self.payload_length.saturating_add(bytes.len());
        // Count the remainder without retaining it so oversized errors still report the
        // complete JSON length and serialization failures keep their original category.
        if self.payload_length <= MAX_FRAME_SIZE {
            self.destination.extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
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
    fn appends_frames_using_existing_storage_and_utf8_byte_lengths() {
        let mut codec = FleetCodec::<&str, String>::new();
        let mut destination = BytesMut::with_capacity(128);
        let allocation = destination.as_ptr();
        codec.encode("λ\n", &mut destination).unwrap();
        assert_eq!(&destination[..], b"\0\0\0\x06\"\xce\xbb\\n\"");
        codec.encode("next", &mut destination).unwrap();
        assert_eq!(destination.as_ptr(), allocation);
        assert_eq!(
            codec.decode(&mut destination).unwrap(),
            Some("λ\n".to_owned())
        );
        assert_eq!(
            codec.decode(&mut destination).unwrap(),
            Some("next".to_owned())
        );
        assert!(destination.is_empty());
    }

    #[test]
    fn accepts_the_exact_payload_limit_and_rolls_back_oversized_encoding() {
        let payload = "x".repeat(MAX_FRAME_SIZE - 2);
        let mut codec = FleetCodec::<&str, String>::new();
        let mut destination = BytesMut::new();
        codec.encode(&payload, &mut destination).unwrap();
        assert_eq!(destination.len(), MAX_FRAME_SIZE + 4);
        assert_eq!(&destination[..4], &(MAX_FRAME_SIZE as u32).to_be_bytes());
        assert_eq!(
            codec.decode(&mut destination).unwrap().as_deref(),
            Some(payload.as_str())
        );

        codec.encode("queued", &mut destination).unwrap();
        let queued = destination.clone();
        let oversized = "x".repeat(MAX_FRAME_SIZE - 1);
        assert!(matches!(codec.encode(&oversized, &mut destination),
            Err(CodecError::FrameTooLarge { actual, max })
                if actual == MAX_FRAME_SIZE + 1 && max == MAX_FRAME_SIZE));
        assert_eq!(destination, queued);
        assert!(
            destination.capacity() <= RETAINED_CAPACITY,
            "a rejected frame kept {} bytes of write capacity",
            destination.capacity()
        );
        codec.encode("next", &mut destination).unwrap();
        assert_eq!(
            codec.decode(&mut destination).unwrap(),
            Some("queued".to_owned())
        );
        assert_eq!(
            codec.decode(&mut destination).unwrap(),
            Some("next".to_owned())
        );
    }

    #[test]
    fn counts_json_escaping_against_the_payload_limit() {
        let payload = "\0".repeat(MAX_FRAME_SIZE / 6 + 1);
        assert!(payload.len() < MAX_FRAME_SIZE);
        let mut codec = FleetCodec::<&str, String>::new();
        let mut destination = BytesMut::from(&b"queued"[..]);
        assert!(matches!(
            codec.encode(&payload, &mut destination),
            Err(CodecError::FrameTooLarge { actual, max })
                if actual == payload.len() * 6 + 2 && max == MAX_FRAME_SIZE
        ));
        assert_eq!(&destination[..], b"queued");
    }

    #[test]
    fn serialization_failure_preserves_queued_frames() {
        struct FailingMessage<'a>(&'a str);
        impl Serialize for FailingMessage<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                let mut sequence = serializer.serialize_seq(Some(2))?;
                sequence.serialize_element(self.0)?;
                Err(serde::ser::Error::custom("failed after writing"))
            }
        }
        let mut codec = FleetCodec::<FailingMessage, String>::new();
        let mut destination = BytesMut::from(&b"queued"[..]);
        let oversized = "x".repeat(MAX_FRAME_SIZE);
        for payload in ["partial output", &oversized] {
            assert!(
                matches!(codec.encode(FailingMessage(payload), &mut destination),
                Err(CodecError::Json(error)) if error.to_string() == "failed after writing")
            );
            assert_eq!(&destination[..], b"queued");
        }
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
