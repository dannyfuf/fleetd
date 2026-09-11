//! The byte-exact frame assertion every wire golden in this crate is written against.
//!
//! Shared by `compatibility.rs` and `agent_compatibility.rs` so both suites check the same three
//! things and neither drifts into checking fewer: the four-byte length prefix, the exact payload
//! bytes, and that an independently written fixture decodes back to the same value. A round-trip
//! assertion alone catches nothing about a field rename or a casing change, which is the whole
//! reason `rust-ipc-protocol` Rule 9 asks for these.

use bytes::BytesMut;
use fleet_proto::codec::FleetCodec;
use serde::{Serialize, de::DeserializeOwned};
use tokio_util::codec::{Decoder, Encoder};

/// Asserts `message` encodes to exactly `golden`, and that `golden` decodes back to `message`.
#[track_caller]
pub fn assert_frame<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(
    message: T,
    golden: &str,
) {
    let mut codec = FleetCodec::<&T, T>::new();
    let mut frame = BytesMut::new();
    codec
        .encode(&message, &mut frame)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(&frame[..4], &(golden.len() as u32).to_be_bytes());
    assert_eq!(
        std::str::from_utf8(&frame[4..]).unwrap_or_else(|error| panic!("{error}")),
        golden
    );

    // Decode the fixed fixture independently of the encoder's output.
    let mut fixture = BytesMut::new();
    fixture.extend_from_slice(&(golden.len() as u32).to_be_bytes());
    fixture.extend_from_slice(golden.as_bytes());
    assert_eq!(
        codec
            .decode(&mut fixture)
            .unwrap_or_else(|error| panic!("{error}")),
        Some(message)
    );
    assert!(fixture.is_empty());
}
