//! Server-side protocol helpers.
//!
//! Convenience functions for constructing server response messages.

use crate::pane::Pane;
use crate::runtime::{ClientRole, Runtime, TerminationReason};
use rttx_proto::{proto, uuid_to_bytes, v3};
use uuid::Uuid;

/// Client sent a message with no inner payload.
pub const ERR_EMPTY_MESSAGE: u32 = 1;
/// Protocol version mismatch between client and server.
pub const ERR_VERSION_MISMATCH: u32 = 2;
/// A UUID or numeric parameter could not be parsed.
pub const ERR_INVALID_PARAMETER: u32 = 3;
/// The referenced runtime does not exist.
pub const ERR_RUNTIME_NOT_FOUND: u32 = 4;
/// Failed to spawn a PTY process for a new pane.
pub const ERR_SPAWN_FAILED: u32 = 5;
/// The referenced pane does not exist in the runtime.
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

/// Build a `RuntimeList` response.
#[must_use]
pub const fn runtime_list(runtimes: Vec<proto::RuntimeInfo>) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::RuntimeList(proto::RuntimeList { runtimes })),
    }
}

/// Build a deterministic runtime inventory payload for `ListRuntimes`.
#[must_use]
pub fn runtime_inventory<'a, I>(runtimes: I) -> Vec<proto::RuntimeInfo>
where
    I: IntoIterator<Item = &'a Runtime>,
{
    runtime_inventory_for(Uuid::nil(), runtimes)
}

/// Build a deterministic runtime inventory payload for `ListRuntimes` tailored to one client.
#[must_use]
pub fn runtime_inventory_for<'a, I>(client_id: Uuid, runtimes: I) -> Vec<proto::RuntimeInfo>
where
    I: IntoIterator<Item = &'a Runtime>,
{
    let mut inventory: Vec<_> =
        runtimes.into_iter().map(|rt| runtime_info_for(client_id, rt)).collect();
    inventory.sort_by(|left, right| left.id.cmp(&right.id));
    inventory
}

fn runtime_info_for(client_id: Uuid, rt: &Runtime) -> proto::RuntimeInfo {
    let mut panes: Vec<_> = rt.panes.values().map(pane_info).collect();
    panes.sort_by(|left, right| left.id.cmp(&right.id));

    proto::RuntimeInfo {
        id: uuid_to_bytes(rt.id),
        name: rt.name.clone(),
        pane_count: u32::try_from(rt.panes.len()).unwrap_or(u32::MAX),
        has_attached_client: rt.has_attached_clients(),
        active_pane_id: rt.active_pane_id.map(uuid_to_bytes),
        panes,
        policy: rt.policy.as_proto() as i32,
        attached_client_count: u32::try_from(rt.attached_client_count()).unwrap_or(u32::MAX),
        reconstructed: rt.reconstructed,
        revision: rt.revision(),
        current_client_role: rt
            .client_role(client_id)
            .map_or(proto::RuntimeClientRole::Unattached, ClientRole::as_proto)
            as i32,
        has_write_owner: rt.has_write_owner(),
        read_only_client_count: u32::try_from(rt.read_only_client_count()).unwrap_or(u32::MAX),
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

/// Build a `RuntimeCreated` response.
#[must_use]
pub fn runtime_created(runtime_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::RuntimeCreated(proto::RuntimeCreated {
            runtime_id: uuid_to_bytes(runtime_id),
            revision,
        })),
    }
}

/// Build a `RuntimeDetached` response.
#[must_use]
pub fn runtime_detached(runtime_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::RuntimeDetached(proto::RuntimeDetached {
            runtime_id: uuid_to_bytes(runtime_id),
            revision,
        })),
    }
}

/// Build a `RuntimeTerminated` response.
#[must_use]
pub fn runtime_terminated(
    runtime_id: Uuid,
    final_revision: u64,
    reason: TerminationReason,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::RuntimeTerminated(proto::RuntimeTerminated {
            runtime_id: uuid_to_bytes(runtime_id),
            final_revision,
            reason: reason.as_proto() as i32,
        })),
    }
}

/// Build a `Snapshot` response.
#[must_use]
pub fn snapshot(
    runtime_id: Uuid,
    panes: Vec<proto::PaneSnapshot>,
    revision: u64,
    current_client_role: ClientRole,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::Snapshot(proto::Snapshot {
            runtime_id: uuid_to_bytes(runtime_id),
            panes,
            revision,
            current_client_role: current_client_role.as_proto() as i32,
        })),
    }
}

/// Build a `Delta` message.
#[must_use]
pub fn delta(runtime_id: Uuid, pane_id: Uuid, data: bytes::Bytes) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::Delta(proto::Delta {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            data,
        })),
    }
}

/// Build a `PaneCreated` message.
#[must_use]
pub fn pane_created(runtime_id: Uuid, pane_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneCreated(proto::PaneCreated {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            revision,
        })),
    }
}

/// Build a `PaneClosed` message.
#[must_use]
pub fn pane_closed(runtime_id: Uuid, pane_id: Uuid, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneClosed(proto::PaneClosed {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            revision,
        })),
    }
}

/// Build a `PaneResized` acknowledgement.
#[must_use]
pub fn pane_resized(
    runtime_id: Uuid,
    pane_id: Uuid,
    cols: u16,
    rows: u16,
    revision: u64,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneResized(proto::PaneResized {
            runtime_id: uuid_to_bytes(runtime_id),
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
    runtime_id: Uuid,
    pane_id: Uuid,
    status: i32,
    revision: u64,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::PaneExited(proto::PaneExited {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            status,
            revision,
        })),
    }
}

/// Build a `TitleChanged` message.
#[must_use]
pub fn title_changed(
    runtime_id: Uuid,
    pane_id: Uuid,
    title: String,
    revision: u64,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::TitleChanged(proto::TitleChanged {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            title,
            revision,
        })),
    }
}

/// Build a `CwdChanged` message.
#[must_use]
pub fn cwd_changed(
    runtime_id: Uuid,
    pane_id: Uuid,
    cwd: String,
    revision: u64,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::CwdChanged(proto::CwdChanged {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            cwd,
            revision,
        })),
    }
}

/// Build an `AttachBlocked` response.
#[must_use]
pub fn attach_blocked(
    runtime_id: Uuid,
    current_client_role: Option<ClientRole>,
    attached_client_count: usize,
    read_only_client_count: usize,
) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::AttachBlocked(proto::AttachBlocked {
            runtime_id: uuid_to_bytes(runtime_id),
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

/// Build a `DiagnosticsReport` response from the current server state.
#[must_use]
pub fn diagnostics_report(server: &crate::server::Server) -> proto::ServerMessage {
    let report = server.diagnostics();
    let runtimes = report
        .runtimes
        .iter()
        .map(|s| {
            let panes = s
                .panes
                .iter()
                .map(|p| {
                    let id = uuid::Uuid::parse_str(&p.id).map(uuid_to_bytes).unwrap_or_default();
                    proto::PaneDiagnosticsInfo {
                        id,
                        raw_bytes_len: p.raw_bytes_len as u64,
                        pending_flush_len: p.pending_flush_len as u64,
                        is_exited: p.is_exited,
                    }
                })
                .collect();
            let id = uuid::Uuid::parse_str(&s.id).map(uuid_to_bytes).unwrap_or_default();
            proto::RuntimeDiagnosticsInfo {
                id,
                name: s.name.clone(),
                active_pane_count: s.active_pane_count as u32,
                exited_pane_count: s.exited_pane_count as u32,
                command_history_len: s.command_history_len as u32,
                attached_client_count: s.attached_client_count as u32,
                panes,
            }
        })
        .collect();
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::DiagnosticsReport(proto::DiagnosticsReport {
            runtime_count: report.runtime_count as u32,
            total_pane_count: report.total_pane_count as u32,
            total_active_panes: report.total_active_panes as u32,
            total_exited_panes: report.total_exited_panes as u32,
            client_count: report.client_count as u32,
            pty_writer_count: report.pty_writer_count as u32,
            total_raw_bytes: report.total_raw_bytes as u64,
            total_pending_flush: report.total_pending_flush as u64,
            total_command_history: report.total_command_history as u32,
            runtimes,
        })),
    }
}

/// Build a `RuntimeRenamed` response.
#[must_use]
pub fn runtime_renamed(runtime_id: Uuid, name: String, revision: u64) -> proto::ServerMessage {
    proto::ServerMessage {
        msg: Some(proto::server_message::Msg::RuntimeRenamed(proto::RuntimeRenamed {
            runtime_id: uuid_to_bytes(runtime_id),
            name,
            revision,
        })),
    }
}

// ── V3 protocol helpers ─────────────────────────────────────────

/// Build a v3 `RuntimeSnapshot` from a runtime's current state.
#[must_use]
pub fn build_v3_runtime_snapshot(
    rt: &Runtime,
    runtime_id: Uuid,
    client_role: v3::RuntimeClientRole,
) -> v3::RuntimeSnapshot {
    let panes: Vec<v3::PaneSnapshot> = rt
        .panes
        .values()
        .map(|pane| {
            let scrollback_data = crate::screen::strip_client_queries(
                pane.screen.snapshot_bytes(crate::pane::MAX_SNAPSHOT_BYTES),
            );
            let total_scrollback_bytes = pane.screen.raw_bytes().len() as u64;
            rttx_proto::v3_snapshot::build_pane_snapshot(
                rttx_proto::v3_snapshot::PaneSnapshotParams {
                    pane_id: pane.id,
                    pane_output_seq: pane.output_seq,
                    title: pane.title.clone().unwrap_or_default(),
                    cwd: pane.effective_cwd().unwrap_or_default(),
                    cols: u32::from(pane.cols),
                    rows: u32::from(pane.rows),
                    exit_status: pane.exit_status,
                    terminal_modes: pane.screen.terminal_mode_state(),
                    scrollback_tail: bytes::Bytes::from(scrollback_data),
                    total_scrollback_bytes,
                },
            )
        })
        .collect();
    rttx_proto::v3_snapshot::build_runtime_snapshot(runtime_id, rt.revision(), client_role, panes)
}

/// Build a v3 runtime inventory for `ListRuntimes`.
#[must_use]
pub fn v3_runtime_inventory_for<'a, I>(
    client_id: Uuid,
    runtimes: I,
    has_inventory_v2: bool,
) -> Vec<v3::RuntimeInfo>
where
    I: IntoIterator<Item = &'a Runtime>,
{
    let mut inventory: Vec<_> = runtimes
        .into_iter()
        .map(|rt| v3_runtime_info_for(client_id, rt, has_inventory_v2))
        .collect();
    inventory.sort_by(|left, right| left.id.cmp(&right.id));
    inventory
}

fn v3_runtime_info_for(client_id: Uuid, rt: &Runtime, has_inventory_v2: bool) -> v3::RuntimeInfo {
    let policy = match rt.policy {
        crate::runtime::RuntimePolicy::Persistent => v3::RuntimePolicy::Persistent,
        crate::runtime::RuntimePolicy::Ephemeral => v3::RuntimePolicy::Ephemeral,
    };
    let current_role = rt
        .client_role(client_id)
        .map_or(v3::RuntimeClientRole::Unattached, ClientRole::as_v3_proto);

    let params = rttx_proto::v3_inventory::RuntimeInfoParams {
        id: rt.id,
        name: rt.name.clone(),
        policy,
        pane_count: u32::try_from(rt.panes.len()).unwrap_or(u32::MAX),
        has_write_owner: rt.has_write_owner(),
        read_only_client_count: u32::try_from(rt.read_only_client_count()).unwrap_or(u32::MAX),
        current_client_role: current_role,
        runtime_revision: rt.revision(),
        reconstructed: rt.reconstructed,
    };

    if has_inventory_v2 {
        let mut panes: Vec<_> = rt
            .panes
            .values()
            .map(|pane| {
                rttx_proto::v3_inventory::build_pane_info(
                    rttx_proto::v3_inventory::PaneInfoParams {
                        pane_id: pane.id,
                        title: pane.title.clone().unwrap_or_default(),
                        cwd: pane.effective_cwd().unwrap_or_default(),
                        cols: u32::from(pane.cols),
                        rows: u32::from(pane.rows),
                        exit_status: pane.exit_status,
                        reconstructed: pane.reconstructed,
                    },
                )
            })
            .collect();
        panes.sort_by(|left, right| left.id.cmp(&right.id));
        rttx_proto::v3_inventory::build_runtime_info_v2(
            params,
            rttx_proto::v3_inventory::RuntimeInfoV2Fields {
                active_pane_summary: String::new(),
                takeover_eligible: !rt.has_write_owner(),
                disabled_reason: String::new(),
                panes,
            },
        )
    } else {
        rttx_proto::v3_inventory::build_runtime_info(params)
    }
}

/// Build a v3 `DiagnosticsReport` from the current server state.
#[must_use]
pub fn v3_diagnostics_report(server: &crate::server::Server) -> v3::DiagnosticsReport {
    let report = server.diagnostics();
    let runtimes = report
        .runtimes
        .iter()
        .map(|s| {
            let panes = s
                .panes
                .iter()
                .map(|p| {
                    let id = uuid::Uuid::parse_str(&p.id).map(uuid_to_bytes).unwrap_or_default();
                    v3::PaneDiagnosticsInfo {
                        id,
                        raw_bytes_len: p.raw_bytes_len as u64,
                        pending_flush_len: p.pending_flush_len as u64,
                        is_exited: p.is_exited,
                    }
                })
                .collect();
            let id = uuid::Uuid::parse_str(&s.id).map(uuid_to_bytes).unwrap_or_default();
            v3::RuntimeDiagnosticsInfo {
                id,
                name: s.name.clone(),
                active_pane_count: s.active_pane_count as u32,
                exited_pane_count: s.exited_pane_count as u32,
                command_history_len: s.command_history_len as u32,
                attached_client_count: s.attached_client_count as u32,
                panes,
            }
        })
        .collect();
    v3::DiagnosticsReport {
        runtime_count: report.runtime_count as u32,
        total_pane_count: report.total_pane_count as u32,
        total_active_panes: report.total_active_panes as u32,
        total_exited_panes: report.total_exited_panes as u32,
        client_count: report.client_count as u32,
        pty_writer_count: report.pty_writer_count as u32,
        total_raw_bytes: report.total_raw_bytes as u64,
        total_pending_flush: report.total_pending_flush as u64,
        total_command_history: report.total_command_history as u32,
        runtimes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimePolicy;

    #[test]
    fn error_codes_are_distinct_and_nonzero() {
        let codes = [
            ERR_EMPTY_MESSAGE,
            ERR_VERSION_MISMATCH,
            ERR_INVALID_PARAMETER,
            ERR_RUNTIME_NOT_FOUND,
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
    fn runtime_inventory_maps_runtime_metadata() {
        let mut rt = Runtime::new("inventory".into());
        rt.id = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
        rt.policy = RuntimePolicy::Ephemeral;
        rt.reconstructed = true;
        let writer = Uuid::new_v4();
        let reader = Uuid::new_v4();
        let _ = rt.attach_client(writer, crate::runtime::AttachMode::ReadWrite);
        let _ = rt.attach_client(reader, crate::runtime::AttachMode::ReadOnly);

        let mut pane =
            Pane::new(Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(), 120, 40);
        pane.cwd = Some("/tmp/work".into());
        pane.title = Some("shell".into());
        pane.exit_status = Some(7);
        pane.reconstructed = true;
        let pane_id = pane.id;
        rt.add_pane(pane);
        rt.active_pane_id = Some(pane_id);

        let inventory = runtime_inventory_for(writer, [&rt]);
        assert_eq!(inventory.len(), 1);

        let info = &inventory[0];
        assert_eq!(info.name, "inventory");
        assert_eq!(info.pane_count, 1);
        assert!(info.has_attached_client);
        assert_eq!(info.attached_client_count, 2);
        assert_eq!(info.active_pane_id.as_deref(), Some(pane_id.as_bytes().as_slice()));
        assert_eq!(info.policy, proto::RuntimePolicy::Ephemeral as i32);
        assert!(info.reconstructed);
        assert_eq!(info.revision, rt.revision());
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
    fn runtime_inventory_is_sorted_by_runtime_and_pane_id() {
        let mut later = Runtime::new("later".into());
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

        let mut earlier = Runtime::new("earlier".into());
        earlier.id = Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap();
        earlier.add_pane(Pane::new(
            Uuid::parse_str("99999999-9999-9999-9999-999999999999").unwrap(),
            80,
            24,
        ));

        let inventory = runtime_inventory([&later, &earlier]);
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

    #[test]
    fn cwd_changed_message_contains_correct_fields() {
        let sid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let msg = cwd_changed(sid, pid, "/home/user".into(), 42);
        let proto::server_message::Msg::CwdChanged(inner) = msg.msg.unwrap() else {
            panic!("expected CwdChanged");
        };
        assert_eq!(inner.cwd, "/home/user");
        assert_eq!(inner.revision, 42);
        assert_eq!(inner.runtime_id, sid.as_bytes().to_vec());
        assert_eq!(inner.pane_id, pid.as_bytes().to_vec());
    }
}
