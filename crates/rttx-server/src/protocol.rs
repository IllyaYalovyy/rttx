//! Server-side protocol helpers.
//!
//! Convenience functions for constructing server response messages.

use rttx_proto::{proto, uuid_to_bytes};
use uuid::Uuid;

/// Build a `HelloAck` response.
#[must_use]
pub fn hello_ack(server_id: Uuid) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::HelloAck(proto::HelloAck {
            protocol_version: rttx_proto::PROTOCOL_VERSION,
            server_id: uuid_to_bytes(server_id),
        })),
    }
}

/// Build a `SessionList` response.
#[must_use]
pub const fn session_list(sessions: Vec<proto::SessionInfo>) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::SessionList(proto::SessionList { sessions })),
    }
}

/// Build a `SessionCreated` response.
#[must_use]
pub fn session_created(session_id: Uuid) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::SessionCreated(proto::SessionCreated {
            session_id: uuid_to_bytes(session_id),
        })),
    }
}

/// Build a `Snapshot` response.
#[must_use]
pub fn snapshot(session_id: Uuid, panes: Vec<proto::PaneSnapshot>) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::Snapshot(proto::Snapshot {
            session_id: uuid_to_bytes(session_id),
            panes,
        })),
    }
}

/// Build a `Delta` message.
#[must_use]
pub fn delta(session_id: Uuid, pane_id: Uuid, data: Vec<u8>) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::Delta(proto::Delta {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            data,
        })),
    }
}

/// Build a `PaneCreated` message.
#[must_use]
pub fn pane_created(session_id: Uuid, pane_id: Uuid) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneCreated(proto::PaneCreated {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
        })),
    }
}

/// Build a `PaneClosed` message.
#[must_use]
pub fn pane_closed(session_id: Uuid, pane_id: Uuid) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneClosed(proto::PaneClosed {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
        })),
    }
}

/// Build a `PaneExited` message.
#[must_use]
pub fn pane_exited(session_id: Uuid, pane_id: Uuid, status: i32) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneExited(proto::PaneExited {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            status,
        })),
    }
}

/// Build a `TitleChanged` message.
#[must_use]
pub fn title_changed(session_id: Uuid, pane_id: Uuid, title: String) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::TitleChanged(proto::TitleChanged {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            title,
        })),
    }
}

/// Build an `Error` response.
#[must_use]
pub const fn error(code: u32, message: String) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::Error(proto::Error { code, message })),
    }
}
