//! Server-side protocol helpers.
//!
//! Convenience functions for constructing server response messages.

use crate::pane::Pane;
use crate::session::{ClientRole, Session, TerminationReason};
use rttx_proto::{proto, uuid_to_bytes};
use uuid::Uuid;

/// Client sent a message with no inner payload.
pub const ERR_EMPTY_MESSAGE: u32 = 1;
/// Protocol version mismatch between client and server.
pub const ERR_VERSION_MISMATCH: u32 = 2;
/// A UUID or numeric parameter could not be parsed.
pub const ERR_INVALID_PARAMETER: u32 = 3;
/// The referenced session/runtime does not exist.
pub const ERR_SESSION_NOT_FOUND: u32 = 4;
/// Failed to spawn a PTY process for a new pane.
pub const ERR_SPAWN_FAILED: u32 = 5;
/// The referenced pane does not exist in the session.
pub const ERR_PANE_NOT_FOUND: u32 = 6;
/// The pane exists but has no running PTY.
pub const ERR_PANE_NOT_RUNNING: u32 = 7;
/// The requested operation is not supported yet.
pub const ERR_UNSUPPORTED: u32 = 8;
/// The runtime is owned by another client.
pub const ERR_OWNERSHIP_CONFLICT: u32 = 9;

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

/// Build a `Pong` response.
#[must_use]
pub const fn pong(nonce: u64) -> proto::ServerMessage {
    proto::ServerMessage { msg: Some(proto::server_message::Msg::Pong(proto::Pong { nonce })) }
}

/// Build a `SessionList` response.
#[must_use]
pub const fn session_list(sessions: Vec<proto::SessionInfo>) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::SessionList(proto::SessionList { sessions })),
    }
}

/// Build a deterministic runtime inventory payload for `ListSessions`.
#[must_use]
pub fn session_inventory<'a, I>(sessions: I) -> Vec<proto::SessionInfo>
where
    I: IntoIterator<Item = &'a Session>,
{
    session_inventory_for(Uuid::nil(), sessions)
}

/// Build a deterministic runtime inventory payload for `ListSessions` tailored to one client.
#[must_use]
pub fn session_inventory_for<'a, I>(client_id: Uuid, sessions: I) -> Vec<proto::SessionInfo>
where
    I: IntoIterator<Item = &'a Session>,
{
    let mut inventory: Vec<_> =
        sessions.into_iter().map(|session| session_info(client_id, session)).collect();
    inventory.sort_by(|left, right| left.id.cmp(&right.id));
    inventory
}

fn session_info(client_id: Uuid, session: &Session) -> proto::SessionInfo {
    let mut panes: Vec<_> = session.panes.values().map(pane_info).collect();
    panes.sort_by(|left, right| left.id.cmp(&right.id));

    proto::SessionInfo {
        id: uuid_to_bytes(session.id),
        name: session.name.clone(),
        pane_count: u32::try_from(session.panes.len()).unwrap_or(u32::MAX),
        has_attached_client: session.has_attached_clients(),
        active_pane_id: session.active_pane_id.map(uuid_to_bytes),
        panes,
        policy: session.policy.as_proto() as i32,
        attached_client_count: u32::try_from(session.attached_client_count()).unwrap_or(u32::MAX),
        reconstructed: session.reconstructed,
        revision: session.revision(),
        current_client_role: session
            .client_role(client_id)
            .map_or(proto::RuntimeClientRole::Unattached, ClientRole::as_proto)
            as i32,
        has_write_owner: session.has_write_owner(),
        read_only_client_count: u32::try_from(session.read_only_client_count()).unwrap_or(u32::MAX),
    }
}

fn pane_info(pane: &Pane) -> proto::PaneInfo {
    proto::PaneInfo {
        id: uuid_to_bytes(pane.id),
        title: pane.title.clone().unwrap_or_default(),
        cwd: pane.effective_cwd().unwrap_or_default(),
        cols: u32::from(pane.cols),
        rows: u32::from(pane.rows),
        exit_status: pane.exit_status,
        reconstructed: pane.reconstructed,
    }
}

/// Build a `SessionCreated` response.
#[must_use]
pub fn session_created(session_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::SessionCreated(proto::SessionCreated {
            session_id: uuid_to_bytes(session_id),
            revision,
        })),
    }
}

/// Build a `SessionDetached` response.
#[must_use]
pub fn session_detached(session_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::SessionDetached(proto::SessionDetached {
            session_id: uuid_to_bytes(session_id),
            revision,
        })),
    }
}

/// Build a `SessionTerminated` response.
#[must_use]
pub fn session_terminated(
    session_id: Uuid,
    final_revision: u64,
    reason: TerminationReason,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::SessionTerminated(proto::SessionTerminated {
            session_id: uuid_to_bytes(session_id),
            final_revision,
            reason: reason.as_proto() as i32,
        })),
    }
}

/// Build a `Snapshot` response.
#[must_use]
pub fn snapshot(
    session_id: Uuid,
    panes: Vec<proto::PaneSnapshot>,
    revision: u64,
    current_client_role: ClientRole,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::Snapshot(proto::Snapshot {
            session_id: uuid_to_bytes(session_id),
            panes,
            revision,
            current_client_role: current_client_role.as_proto() as i32,
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
pub fn pane_created(session_id: Uuid, pane_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneCreated(proto::PaneCreated {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            revision,
        })),
    }
}

/// Build a `PaneClosed` message.
#[must_use]
pub fn pane_closed(session_id: Uuid, pane_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneClosed(proto::PaneClosed {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            revision,
        })),
    }
}

/// Build a `PaneResized` acknowledgement.
#[must_use]
pub fn pane_resized(
    session_id: Uuid,
    pane_id: Uuid,
    cols: u16,
    rows: u16,
    revision: u64,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneResized(proto::PaneResized {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            cols: u32::from(cols),
            rows: u32::from(rows),
            revision,
        })),
    }
}

/// Build a `PaneExited` message.
#[must_use]
pub fn pane_exited(
    session_id: Uuid,
    pane_id: Uuid,
    status: i32,
    revision: u64,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneExited(proto::PaneExited {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            status,
            revision,
        })),
    }
}

/// Build a `TitleChanged` message.
#[must_use]
pub fn title_changed(
    session_id: Uuid,
    pane_id: Uuid,
    title: String,
    revision: u64,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::TitleChanged(proto::TitleChanged {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            title,
            revision,
        })),
    }
}

/// Build an `AttachBlocked` response.
#[must_use]
pub fn attach_blocked(
    session_id: Uuid,
    current_client_role: Option<ClientRole>,
    attached_client_count: usize,
    read_only_client_count: usize,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::AttachBlocked(proto::AttachBlocked {
            session_id: uuid_to_bytes(session_id),
            current_client_role: current_client_role
                .map_or(proto::RuntimeClientRole::Unattached, ClientRole::as_proto)
                as i32,
            attached_client_count: u32::try_from(attached_client_count).unwrap_or(u32::MAX),
            read_only_client_count: u32::try_from(read_only_client_count).unwrap_or(u32::MAX),
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

/// Build a `SessionRenamed` response.
#[must_use]
pub fn session_renamed(session_id: Uuid, name: String, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::SessionRenamed(proto::SessionRenamed {
            session_id: uuid_to_bytes(session_id),
            name,
            revision,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::RuntimePolicy;

    #[test]
    fn error_codes_are_distinct_and_nonzero() {
        let codes = [
            ERR_EMPTY_MESSAGE,
            ERR_VERSION_MISMATCH,
            ERR_INVALID_PARAMETER,
            ERR_SESSION_NOT_FOUND,
            ERR_SPAWN_FAILED,
            ERR_PANE_NOT_FOUND,
            ERR_PANE_NOT_RUNNING,
            ERR_UNSUPPORTED,
            ERR_OWNERSHIP_CONFLICT,
        ];
        for &code in &codes {
            assert_ne!(code, 0, "error codes must be nonzero");
        }
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "error codes must be distinct");
    }

    #[test]
    fn session_inventory_maps_runtime_metadata() {
        let mut session = Session::new("inventory".into());
        session.id = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
        session.policy = RuntimePolicy::Ephemeral;
        session.reconstructed = true;
        let writer = Uuid::new_v4();
        let reader = Uuid::new_v4();
        let _ = session.attach_client(writer, crate::session::AttachMode::ReadWrite);
        let _ = session.attach_client(reader, crate::session::AttachMode::ReadOnly);

        let mut pane =
            Pane::new(Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(), 120, 40);
        pane.cwd = Some("/tmp/work".into());
        pane.title = Some("shell".into());
        pane.exit_status = Some(7);
        pane.reconstructed = true;
        let pane_id = pane.id;
        session.add_pane(pane);
        session.active_pane_id = Some(pane_id);

        let inventory = session_inventory_for(writer, [&session]);
        assert_eq!(inventory.len(), 1);

        let info = &inventory[0];
        assert_eq!(info.name, "inventory");
        assert_eq!(info.pane_count, 1);
        assert!(info.has_attached_client);
        assert_eq!(info.attached_client_count, 2);
        assert_eq!(info.active_pane_id.as_deref(), Some(pane_id.as_bytes().as_slice()));
        assert_eq!(info.policy, proto::RuntimePolicy::Ephemeral as i32);
        assert!(info.reconstructed);
        assert_eq!(info.revision, session.revision());
        assert_eq!(info.current_client_role, proto::RuntimeClientRole::Writer as i32);
        assert!(info.has_write_owner);
        assert_eq!(info.read_only_client_count, 1);
        assert_eq!(info.panes.len(), 1);
        assert_eq!(info.panes[0].id, pane_id.as_bytes());
        assert_eq!(info.panes[0].title, "shell");
        assert_eq!(info.panes[0].cwd, "/tmp/work");
        assert_eq!(info.panes[0].cols, 120);
        assert_eq!(info.panes[0].rows, 40);
        assert_eq!(info.panes[0].exit_status, Some(7));
        assert!(info.panes[0].reconstructed);
    }

    #[test]
    fn session_inventory_is_sorted_by_session_and_pane_id() {
        let mut later = Session::new("later".into());
        later.id = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();
        later.add_pane(Pane::new(
            Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap(),
            80,
            24,
        ));
        later.add_pane(Pane::new(
            Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            80,
            24,
        ));

        let mut earlier = Session::new("earlier".into());
        earlier.id = Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap();
        earlier.add_pane(Pane::new(
            Uuid::parse_str("99999999-9999-9999-9999-999999999999").unwrap(),
            80,
            24,
        ));

        let inventory = session_inventory([&later, &earlier]);
        assert_eq!(inventory[0].name, "earlier");
        assert_eq!(inventory[1].name, "later");
        assert_eq!(
            inventory[1].panes.iter().map(|pane| pane.id.clone()).collect::<Vec<_>>(),
            vec![
                Uuid::parse_str("11111111-1111-1111-1111-111111111111")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ]
        );
    }
}
