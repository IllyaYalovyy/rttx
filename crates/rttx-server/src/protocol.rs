//! Server-side protocol helpers.
//!
//! Convenience functions for constructing server response messages.

use crate::pane::Pane;
use crate::session::Session;
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

/// Build a deterministic runtime inventory payload for `ListSessions`.
#[must_use]
pub fn session_inventory<'a, I>(sessions: I) -> Vec<proto::SessionInfo>
where
    I: IntoIterator<Item = &'a Session>,
{
    let mut inventory: Vec<_> = sessions.into_iter().map(session_info).collect();
    inventory.sort_by(|left, right| left.id.cmp(&right.id));
    inventory
}

fn session_info(session: &Session) -> proto::SessionInfo {
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
        attached_client_count: u32::try_from(session.attached_clients.len()).unwrap_or(u32::MAX),
        reconstructed: session.reconstructed,
    }
}

fn pane_info(pane: &Pane) -> proto::PaneInfo {
    proto::PaneInfo {
        id: uuid_to_bytes(pane.id),
        title: pane.title.clone().unwrap_or_default(),
        cwd: pane.cwd.clone().unwrap_or_default(),
        cols: u32::from(pane.cols),
        rows: u32::from(pane.rows),
        exit_status: pane.exit_status,
        reconstructed: pane.reconstructed,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::RuntimePolicy;

    #[test]
    fn session_inventory_maps_runtime_metadata() {
        let mut session = Session::new("inventory".into());
        session.id = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
        session.policy = RuntimePolicy::Ephemeral;
        session.reconstructed = true;
        session.attached_clients = vec![Uuid::new_v4(), Uuid::new_v4()];

        let mut pane =
            Pane::new(Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(), 120, 40);
        pane.cwd = Some("/tmp/work".into());
        pane.title = Some("shell".into());
        pane.exit_status = Some(7);
        pane.reconstructed = true;
        let pane_id = pane.id;
        session.add_pane(pane);
        session.active_pane_id = Some(pane_id);

        let inventory = session_inventory([&session]);
        assert_eq!(inventory.len(), 1);

        let info = &inventory[0];
        assert_eq!(info.name, "inventory");
        assert_eq!(info.pane_count, 1);
        assert!(info.has_attached_client);
        assert_eq!(info.attached_client_count, 2);
        assert_eq!(info.active_pane_id.as_deref(), Some(pane_id.as_bytes().as_slice()));
        assert_eq!(info.policy, proto::RuntimePolicy::Ephemeral as i32);
        assert!(info.reconstructed);
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
