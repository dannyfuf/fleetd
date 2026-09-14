//! The framed request/event protocol spoken over a holder's Unix socket.
//!
//! Every frame is `tag: u8 | length: u32 big-endian | payload[length]`. Both peers are blocking
//! threads, so the codec is a pair of blocking helpers rather than a stream adapter: the daemon
//! side runs one reader thread per holder, and the holder runs one per connection.

use std::io::{self, Read, Write};

use thiserror::Error;

/// Version of the holder wire protocol, carried by every `Hello`.
///
/// The two peers are *different `fleetd` builds by design*: a holder started before an upgrade is
/// still running the old binary when the new daemon reattaches. The version is what turns that
/// into a clean, reported refusal instead of a corrupt stream, so bump it whenever a frame's
/// meaning changes and never repurpose a tag.
pub const HOLDER_PROTOCOL_VERSION: u8 = 1;

/// Largest payload accepted in one frame.
///
/// A paste is the only realistic large input and the PTY writer queue caps it well below this;
/// output arrives in 16 KiB reader chunks. The limit exists so a corrupt length header cannot
/// make either peer allocate without bound. It also bounds the replay tail a holder may retain,
/// because a replay larger than one frame could never be delivered.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Byte length of a `Hello` payload: version, child pid, columns, rows.
const HELLO_BYTES: usize = 9;

const TAG_INPUT: u8 = 0x01;
const TAG_RESIZE: u8 = 0x02;
const TAG_KILL: u8 = 0x03;
const TAG_DETACH: u8 = 0x04;
const TAG_HELLO: u8 = 0x81;
const TAG_OUTPUT: u8 = 0x82;
const TAG_EXITED: u8 = 0x83;

/// A malformed, oversized or unreadable holder frame.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// The socket failed while reading or writing a frame.
    #[error("holder socket I/O failed: {0}")]
    Io(#[from] io::Error),
    /// A frame announced more bytes than the protocol admits.
    #[error("holder frame of {length} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    Oversize {
        /// Length announced by the frame header.
        length: usize,
    },
    /// A frame carried a tag this peer does not define.
    #[error("holder frame carried unknown tag {tag:#04x}")]
    UnknownTag {
        /// The undefined tag byte.
        tag: u8,
    },
    /// A frame's payload did not match the shape its tag requires.
    #[error("holder frame is malformed: {0}")]
    Malformed(&'static str),
    /// The holder greeted with a protocol version this build does not speak.
    #[error(
        "holder speaks protocol version {version}, this build speaks {HOLDER_PROTOCOL_VERSION}"
    )]
    UnsupportedVersion {
        /// Version the holder announced.
        version: u8,
    },
}

/// One instruction sent by the daemon to a holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HolderRequest {
    /// Already encoded bytes to write to the child.
    Input(Vec<u8>),
    /// A new authoritative window size.
    Resize {
        /// Column count.
        cols: u16,
        /// Row count.
        rows: u16,
    },
    /// Terminate the child and stop the holder.
    Kill,
    /// Drop this connection and keep the child running.
    Detach,
}

/// One event sent by a holder to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HolderEvent {
    /// The first frame of every connection, identifying the held child and its terminal.
    Hello {
        /// Child process identifier, when the platform exposed one.
        child_pid: Option<u32>,
        /// Columns the kernel currently holds for the PTY.
        ///
        /// Authoritative, and the size a reattaching daemon must build its emulator at: the
        /// replay tail was produced for this grid, not for the size the terminal was created at.
        cols: u16,
        /// Rows the kernel currently holds for the PTY.
        rows: u16,
    },
    /// Bytes read from the child, replayed first and live afterwards.
    Output(Vec<u8>),
    /// The child exited, with its status code when one was observed.
    Exited(Option<i32>),
}

impl HolderRequest {
    fn encode(&self) -> (u8, Vec<u8>) {
        match self {
            Self::Input(bytes) => (TAG_INPUT, bytes.clone()),
            Self::Resize { cols, rows } => {
                let mut payload = Vec::with_capacity(4);
                payload.extend_from_slice(&cols.to_be_bytes());
                payload.extend_from_slice(&rows.to_be_bytes());
                (TAG_RESIZE, payload)
            }
            Self::Kill => (TAG_KILL, Vec::new()),
            Self::Detach => (TAG_DETACH, Vec::new()),
        }
    }

    fn decode(tag: u8, payload: Vec<u8>) -> Result<Self, ProtocolError> {
        match tag {
            TAG_INPUT => Ok(Self::Input(payload)),
            TAG_RESIZE => {
                let [cols_high, cols_low, rows_high, rows_low] = payload[..] else {
                    return Err(ProtocolError::Malformed("resize needs four payload bytes"));
                };
                Ok(Self::Resize {
                    cols: u16::from_be_bytes([cols_high, cols_low]),
                    rows: u16::from_be_bytes([rows_high, rows_low]),
                })
            }
            TAG_KILL => Ok(Self::Kill),
            TAG_DETACH => Ok(Self::Detach),
            tag => Err(ProtocolError::UnknownTag { tag }),
        }
    }

    /// Writes this request as one frame.
    pub(crate) fn write_to(&self, writer: &mut impl Write) -> Result<(), ProtocolError> {
        let (tag, payload) = self.encode();
        write_frame(writer, tag, &payload)
    }

    /// Reads the next request, returning `None` at a clean end of stream.
    pub(crate) fn read_from(reader: &mut impl Read) -> Result<Option<Self>, ProtocolError> {
        match read_frame(reader)? {
            Some((tag, payload)) => Self::decode(tag, payload).map(Some),
            None => Ok(None),
        }
    }
}

impl HolderEvent {
    fn encode(&self) -> (u8, Vec<u8>) {
        match self {
            Self::Hello {
                child_pid,
                cols,
                rows,
            } => {
                let mut payload = Vec::with_capacity(HELLO_BYTES);
                payload.push(HOLDER_PROTOCOL_VERSION);
                payload.extend_from_slice(&child_pid.unwrap_or(0).to_be_bytes());
                payload.extend_from_slice(&cols.to_be_bytes());
                payload.extend_from_slice(&rows.to_be_bytes());
                (TAG_HELLO, payload)
            }
            Self::Output(bytes) => (TAG_OUTPUT, bytes.clone()),
            Self::Exited(code) => (
                TAG_EXITED,
                code.map(|code| code.to_be_bytes().to_vec())
                    .unwrap_or_default(),
            ),
        }
    }

    fn decode(tag: u8, payload: Vec<u8>) -> Result<Self, ProtocolError> {
        match tag {
            TAG_HELLO => decode_hello(&payload),
            TAG_OUTPUT => Ok(Self::Output(payload)),
            TAG_EXITED => Ok(Self::Exited(
                optional_word(&payload, "exit carries zero or four bytes")?.map(i32::from_be_bytes),
            )),
            tag => Err(ProtocolError::UnknownTag { tag }),
        }
    }

    /// Writes this event as one frame.
    pub(crate) fn write_to(&self, writer: &mut impl Write) -> Result<(), ProtocolError> {
        let (tag, payload) = self.encode();
        write_frame(writer, tag, &payload)
    }

    /// Reads the next event, returning `None` at a clean end of stream.
    pub(crate) fn read_from(reader: &mut impl Read) -> Result<Option<Self>, ProtocolError> {
        match read_frame(reader)? {
            Some((tag, payload)) => Self::decode(tag, payload).map(Some),
            None => Ok(None),
        }
    }
}

/// Decodes a greeting, reporting a version mismatch before anything else.
///
/// The version is the first byte precisely so that a peer from another build is recognised rather
/// than mis-parsed: every future layout change keeps that byte where it is.
fn decode_hello(payload: &[u8]) -> Result<HolderEvent, ProtocolError> {
    let Some(version) = payload.first().copied() else {
        return Err(ProtocolError::Malformed("hello carries a version byte"));
    };
    if version != HOLDER_PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion { version });
    }
    let [_version, pid @ .., cols_high, cols_low, rows_high, rows_low] = payload else {
        return Err(ProtocolError::Malformed("hello is nine bytes"));
    };
    let [a, b, c, d] = pid else {
        return Err(ProtocolError::Malformed("hello is nine bytes"));
    };
    Ok(HolderEvent::Hello {
        // Zero is never a child of this process, so it is the absent pid.
        child_pid: Some(u32::from_be_bytes([*a, *b, *c, *d])).filter(|pid| *pid != 0),
        cols: u16::from_be_bytes([*cols_high, *cols_low]),
        rows: u16::from_be_bytes([*rows_high, *rows_low]),
    })
}

/// Decodes the zero-or-four byte payload `Exited` uses for "absent or a word".
fn optional_word(payload: &[u8], message: &'static str) -> Result<Option<[u8; 4]>, ProtocolError> {
    match payload {
        [] => Ok(None),
        [a, b, c, d] => Ok(Some([*a, *b, *c, *d])),
        _ => Err(ProtocolError::Malformed(message)),
    }
}

fn write_frame(writer: &mut impl Write, tag: u8, payload: &[u8]) -> Result<(), ProtocolError> {
    let length = payload.len();
    if length > MAX_FRAME_BYTES {
        return Err(ProtocolError::Oversize { length });
    }
    // One buffered write per frame: two syscalls would let a concurrent frame interleave.
    let mut frame = Vec::with_capacity(length + 5);
    frame.push(tag);
    frame.extend_from_slice(&u32::try_from(length).unwrap_or(u32::MAX).to_be_bytes());
    frame.extend_from_slice(payload);
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

fn read_frame(reader: &mut impl Read) -> Result<Option<(u8, Vec<u8>)>, ProtocolError> {
    let mut header = [0_u8; 5];
    match reader.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(ProtocolError::Oversize { length });
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Some((header[0], payload)))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn requests_round_trip_through_one_stream() {
        let requests = vec![
            HolderRequest::Input(b"ls -la\r".to_vec()),
            HolderRequest::Resize {
                cols: 203,
                rows: 61,
            },
            HolderRequest::Kill,
            HolderRequest::Detach,
            HolderRequest::Input(Vec::new()),
        ];
        let mut encoded = Vec::new();
        for request in &requests {
            request
                .write_to(&mut encoded)
                .unwrap_or_else(|error| panic!("{error}"));
        }

        let mut reader = Cursor::new(encoded);
        let mut decoded = Vec::new();
        while let Some(request) =
            HolderRequest::read_from(&mut reader).unwrap_or_else(|error| panic!("{error}"))
        {
            decoded.push(request);
        }
        assert_eq!(decoded, requests);
    }

    #[test]
    fn events_round_trip_including_absent_pid_and_exit_code() {
        let events = vec![
            HolderEvent::Hello {
                child_pid: Some(4_242),
                cols: 203,
                rows: 61,
            },
            HolderEvent::Hello {
                child_pid: None,
                cols: 0,
                rows: 0,
            },
            HolderEvent::Output(b"hello\r\n".to_vec()),
            HolderEvent::Exited(Some(0)),
            HolderEvent::Exited(Some(-9)),
            HolderEvent::Exited(None),
        ];
        let mut encoded = Vec::new();
        for event in &events {
            event
                .write_to(&mut encoded)
                .unwrap_or_else(|error| panic!("{error}"));
        }

        let mut reader = Cursor::new(encoded);
        let mut decoded = Vec::new();
        while let Some(event) =
            HolderEvent::read_from(&mut reader).unwrap_or_else(|error| panic!("{error}"))
        {
            decoded.push(event);
        }
        assert_eq!(decoded, events);
    }

    #[test]
    fn a_truncated_frame_is_an_error_and_a_clean_end_is_not() {
        let mut encoded = Vec::new();
        HolderEvent::Output(b"partial".to_vec())
            .write_to(&mut encoded)
            .unwrap_or_else(|error| panic!("{error}"));
        encoded.truncate(encoded.len() - 3);

        let mut reader = Cursor::new(encoded);
        assert!(matches!(
            HolderEvent::read_from(&mut reader),
            Err(ProtocolError::Io(_))
        ));
        assert!(
            HolderEvent::read_from(&mut Cursor::new(Vec::new()))
                .unwrap_or_else(|error| panic!("{error}"))
                .is_none()
        );
    }

    #[test]
    fn oversized_and_unknown_frames_are_rejected_without_allocating() {
        let mut header = vec![TAG_OUTPUT];
        header.extend_from_slice(
            &u32::try_from(MAX_FRAME_BYTES + 1)
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        assert!(matches!(
            HolderEvent::read_from(&mut Cursor::new(header)),
            Err(ProtocolError::Oversize { .. })
        ));

        let unknown = vec![0x7f, 0, 0, 0, 0];
        assert!(matches!(
            HolderEvent::read_from(&mut Cursor::new(unknown)),
            Err(ProtocolError::UnknownTag { tag: 0x7f })
        ));

        let mut short_resize = vec![TAG_RESIZE];
        short_resize.extend_from_slice(&2_u32.to_be_bytes());
        short_resize.extend_from_slice(&[0, 80]);
        assert!(matches!(
            HolderRequest::read_from(&mut Cursor::new(short_resize)),
            Err(ProtocolError::Malformed(_))
        ));
    }

    /// The two peers are different `fleetd` builds by design, so these bytes are a contract.
    ///
    /// A change here is a wire break: bump [`HOLDER_PROTOCOL_VERSION`] in the same edit, or an
    /// upgraded daemon will mis-parse a holder that predates it instead of refusing it.
    #[test]
    fn the_wire_bytes_of_every_frame_are_fixed() {
        let cases: Vec<(Vec<u8>, Vec<u8>)> = vec![
            (
                encoded_event(&HolderEvent::Hello {
                    child_pid: Some(0x0001_0932),
                    cols: 120,
                    rows: 36,
                }),
                vec![
                    0x81,
                    0,
                    0,
                    0,
                    9,
                    HOLDER_PROTOCOL_VERSION,
                    0x00,
                    0x01,
                    0x09,
                    0x32,
                    0x00,
                    0x78,
                    0x00,
                    0x24,
                ],
            ),
            (
                encoded_event(&HolderEvent::Output(b"hi".to_vec())),
                vec![0x82, 0, 0, 0, 2, b'h', b'i'],
            ),
            (
                encoded_event(&HolderEvent::Exited(Some(-1))),
                vec![0x83, 0, 0, 0, 4, 0xff, 0xff, 0xff, 0xff],
            ),
            (
                encoded_event(&HolderEvent::Exited(None)),
                vec![0x83, 0, 0, 0, 0],
            ),
            (
                encoded_request(&HolderRequest::Input(b"ls\r".to_vec())),
                vec![0x01, 0, 0, 0, 3, b'l', b's', b'\r'],
            ),
            (
                encoded_request(&HolderRequest::Resize {
                    cols: 120,
                    rows: 36,
                }),
                vec![0x02, 0, 0, 0, 4, 0x00, 0x78, 0x00, 0x24],
            ),
            (
                encoded_request(&HolderRequest::Kill),
                vec![0x03, 0, 0, 0, 0],
            ),
            (
                encoded_request(&HolderRequest::Detach),
                vec![0x04, 0, 0, 0, 0],
            ),
        ];
        for (actual, expected) in cases {
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn a_greeting_from_another_build_is_refused_by_version_rather_than_mis_parsed() {
        let mut foreign = encoded_event(&HolderEvent::Hello {
            child_pid: Some(7),
            cols: 80,
            rows: 24,
        });
        // The version byte is the first payload byte, and stays there in every future layout.
        foreign[5] = HOLDER_PROTOCOL_VERSION.wrapping_add(1);
        assert!(matches!(
            HolderEvent::read_from(&mut Cursor::new(foreign)),
            Err(ProtocolError::UnsupportedVersion { .. })
        ));

        // A greeting of the right version but the wrong length is malformed, not a version break.
        let mut truncated = vec![TAG_HELLO];
        truncated.extend_from_slice(&5_u32.to_be_bytes());
        truncated.extend_from_slice(&[HOLDER_PROTOCOL_VERSION, 0, 0, 0, 1]);
        assert!(matches!(
            HolderEvent::read_from(&mut Cursor::new(truncated)),
            Err(ProtocolError::Malformed(_))
        ));

        // An empty greeting carries no version at all.
        let empty = vec![TAG_HELLO, 0, 0, 0, 0];
        assert!(matches!(
            HolderEvent::read_from(&mut Cursor::new(empty)),
            Err(ProtocolError::Malformed(_))
        ));
    }

    fn encoded_event(event: &HolderEvent) -> Vec<u8> {
        let mut bytes = Vec::new();
        event
            .write_to(&mut bytes)
            .unwrap_or_else(|error| panic!("{error}"));
        bytes
    }

    fn encoded_request(request: &HolderRequest) -> Vec<u8> {
        let mut bytes = Vec::new();
        request
            .write_to(&mut bytes)
            .unwrap_or_else(|error| panic!("{error}"));
        bytes
    }

    #[test]
    fn writing_an_oversized_payload_is_refused() {
        let event = HolderEvent::Output(vec![0_u8; MAX_FRAME_BYTES + 1]);
        assert!(matches!(
            event.write_to(&mut Vec::new()),
            Err(ProtocolError::Oversize { .. })
        ));
    }
}
