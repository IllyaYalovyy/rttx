//! Shared protocol types for rttx-server and rttx GUI communication.
//!
//! All wire protocol messages are defined in `proto/rttx.proto` and generated
//! by `prost-build`. This crate also provides helper functions for UUID
//! conversion and length-prefixed message framing.

use bytes::{Buf, BufMut, BytesMut};
use prost::Message;

/// Protocol version. Bumped on incompatible wire format changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum message size (16 MB). Prevents unbounded allocations.
pub const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

/// Generated protobuf types.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rttx.rs"));
}

/// Convert a `uuid::Uuid` to protobuf bytes.
#[must_use]
pub fn uuid_to_bytes(id: uuid::Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

/// Convert protobuf bytes back to a `uuid::Uuid`.
///
/// # Errors
///
/// Returns an error if the byte slice is not exactly 16 bytes.
pub fn bytes_to_uuid(bytes: &[u8]) -> Result<uuid::Uuid, FrameError> {
    let arr: [u8; 16] = bytes.try_into().map_err(|_| FrameError::InvalidUuid(bytes.len()))?;
    Ok(uuid::Uuid::from_bytes(arr))
}

/// Errors that can occur during message framing.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The message exceeds the maximum allowed size.
    #[error("message too large: {0} bytes (max {MAX_MESSAGE_SIZE})")]
    TooLarge(u32),

    /// Not enough data to decode a complete frame.
    #[error("incomplete frame")]
    Incomplete,

    /// Protobuf decode error.
    #[error("decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    /// Invalid UUID byte length.
    #[error("invalid UUID: expected 16 bytes, got {0}")]
    InvalidUuid(usize),
}

/// Encode a protobuf message with a 4-byte little-endian length prefix.
///
/// # Errors
///
/// Returns an error if the encoded message exceeds `MAX_MESSAGE_SIZE`.
pub fn encode_frame<M: Message>(msg: &M, buf: &mut BytesMut) -> Result<(), FrameError> {
    let len = msg.encoded_len();
    let len_u32 = u32::try_from(len).map_err(|_| FrameError::TooLarge(u32::MAX))?;
    if len_u32 > MAX_MESSAGE_SIZE {
        return Err(FrameError::TooLarge(len_u32));
    }
    buf.reserve(4 + len);
    buf.put_u32_le(len_u32);
    msg.encode(buf).map_err(|e| FrameError::Decode(prost::DecodeError::new(e.to_string())))?;
    Ok(())
}

/// Try to decode a length-prefixed frame from the buffer.
///
/// On success, advances the buffer past the consumed frame and returns the
/// decoded message. Returns `Err(FrameError::Incomplete)` if more data is
/// needed.
///
/// # Errors
///
/// Returns an error on size violations, incomplete data, or decode failures.
pub fn decode_frame<M: Message + Default>(buf: &mut BytesMut) -> Result<M, FrameError> {
    if buf.len() < 4 {
        return Err(FrameError::Incomplete);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if len > MAX_MESSAGE_SIZE {
        return Err(FrameError::TooLarge(len));
    }
    let total = 4 + len as usize;
    if buf.len() < total {
        return Err(FrameError::Incomplete);
    }
    buf.advance(4);
    let msg = M::decode(buf.split_to(len as usize))?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_roundtrip() {
        let id = uuid::Uuid::new_v4();
        let bytes = uuid_to_bytes(id);
        let recovered = bytes_to_uuid(&bytes).unwrap();
        assert_eq!(id, recovered);
    }

    #[test]
    fn uuid_invalid_length() {
        assert!(bytes_to_uuid(&[0; 5]).is_err());
    }

    #[test]
    fn frame_roundtrip_hello() {
        let msg = proto::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        };
        let mut buf = BytesMut::new();
        encode_frame(&msg, &mut buf).unwrap();

        let decoded: proto::Hello = decode_frame(&mut buf).unwrap();
        assert_eq!(msg.protocol_version, decoded.protocol_version);
        assert_eq!(msg.client_id, decoded.client_id);
    }

    #[test]
    fn frame_roundtrip_all_client_messages() {
        let session_id = uuid_to_bytes(uuid::Uuid::new_v4());
        let pane_id = uuid_to_bytes(uuid::Uuid::new_v4());

        let messages: Vec<proto::ClientMessage> = vec![
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Hello(proto::Hello {
                    protocol_version: 1,
                    client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                    name: "test".into(),
                    policy: proto::RuntimePolicy::Persistent as i32,
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                    session_id: session_id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                    session_id: session_id.clone(),
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
                    session_id: session_id.clone(),
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    session_id: session_id.clone(),
                    cwd: None,
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    data: b"hello".to_vec(),
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    cols: 80,
                    rows: 24,
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    title: "pane-title".into(),
                })),
            },
            proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
            },
        ];

        for msg in &messages {
            let mut buf = BytesMut::new();
            encode_frame(msg, &mut buf).unwrap();
            let decoded: proto::ClientMessage = decode_frame(&mut buf).unwrap();
            assert_eq!(msg.msg.is_some(), decoded.msg.is_some());
        }
    }

    #[test]
    fn frame_roundtrip_server_messages() {
        let session_id = uuid_to_bytes(uuid::Uuid::new_v4());
        let pane_id = uuid_to_bytes(uuid::Uuid::new_v4());

        let messages: Vec<proto::ServerMessage> = vec![
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::HelloAck(proto::HelloAck {
                    protocol_version: 1,
                    server_id: uuid_to_bytes(uuid::Uuid::new_v4()),
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::SessionList(proto::SessionList {
                    sessions: vec![proto::SessionInfo {
                        id: session_id.clone(),
                        name: "inventory-test".into(),
                        pane_count: 1,
                        has_attached_client: true,
                        active_pane_id: Some(pane_id.clone()),
                        panes: vec![proto::PaneInfo {
                            id: pane_id.clone(),
                            title: "pane-title".into(),
                            cwd: "/tmp/project".into(),
                            cols: 120,
                            rows: 40,
                            exit_status: None,
                            reconstructed: true,
                        }],
                        policy: proto::RuntimePolicy::Persistent as i32,
                        attached_client_count: 1,
                        reconstructed: true,
                        revision: 7,
                        current_client_role: proto::RuntimeClientRole::Writer as i32,
                        has_write_owner: true,
                        read_only_client_count: 0,
                    }],
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::SessionCreated(proto::SessionCreated {
                    session_id: session_id.clone(),
                    revision: 1,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::SessionDetached(proto::SessionDetached {
                    session_id: session_id.clone(),
                    revision: 8,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::SessionTerminated(
                    proto::SessionTerminated {
                        session_id: session_id.clone(),
                        final_revision: 9,
                        reason: proto::RuntimeTerminationReason::Explicit as i32,
                    },
                )),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::Snapshot(proto::Snapshot {
                    session_id: session_id.clone(),
                    panes: vec![proto::PaneSnapshot {
                        pane_id: pane_id.clone(),
                        title: "pane-title".into(),
                        cwd: "/tmp/project".into(),
                        cols: 120,
                        rows: 40,
                        scrollback: b"hello".to_vec(),
                        exit_status: None,
                    }],
                    revision: 10,
                    current_client_role: proto::RuntimeClientRole::Writer as i32,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::Delta(proto::Delta {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    data: b"output".to_vec(),
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::PaneCreated(proto::PaneCreated {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    revision: 11,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::PaneClosed(proto::PaneClosed {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    revision: 12,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::PaneResized(proto::PaneResized {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    cols: 100,
                    rows: 30,
                    revision: 13,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::PaneExited(proto::PaneExited {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    status: 0,
                    revision: 14,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::TitleChanged(proto::TitleChanged {
                    session_id,
                    pane_id,
                    title: "pane-title".into(),
                    revision: 15,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::AttachBlocked(proto::AttachBlocked {
                    session_id: uuid_to_bytes(uuid::Uuid::new_v4()),
                    current_client_role: proto::RuntimeClientRole::Unattached as i32,
                    attached_client_count: 2,
                    read_only_client_count: 1,
                })),
            },
            proto::ServerMessage {
                msg: Some(proto::server_message::Msg::Error(proto::Error {
                    code: 1,
                    message: "not found".into(),
                })),
            },
        ];

        for msg in &messages {
            let mut buf = BytesMut::new();
            encode_frame(msg, &mut buf).unwrap();
            let decoded: proto::ServerMessage = decode_frame(&mut buf).unwrap();
            assert_eq!(msg.msg.is_some(), decoded.msg.is_some());
        }
    }

    #[test]
    fn decode_incomplete_returns_error() {
        let mut buf = BytesMut::from(&[0u8, 0, 0][..]);
        assert!(matches!(decode_frame::<proto::Hello>(&mut buf), Err(FrameError::Incomplete)));
    }

    #[test]
    fn decode_too_large_returns_error() {
        let mut buf = BytesMut::new();
        buf.put_u32_le(MAX_MESSAGE_SIZE + 1);
        assert!(matches!(decode_frame::<proto::Hello>(&mut buf), Err(FrameError::TooLarge(_))));
    }
}
