//! Server-side protocol helpers.
//!
//! Convenience functions for constructing server response messages.

use crate::runtime::{ClientRole, Runtime, TerminationReason};
use rttx_proto::{uuid_to_bytes, v3};
use uuid::Uuid;

// ── V3 protocol helpers ─────────────────────────────────────────

/// Build a v3 `OutputDelta` push envelope.
#[must_use]
pub fn v3_delta(
    runtime_id: Uuid,
    pane_id: Uuid,
    data: bytes::Bytes,
    pane_output_seq: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::OutputDelta(
        v3::OutputDelta {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            data,
            pane_output_seq,
        },
    ))
}

/// Build a v3 `CwdChanged` push envelope.
#[must_use]
pub fn v3_cwd_changed(
    runtime_id: Uuid,
    pane_id: Uuid,
    cwd: String,
    runtime_revision: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::CwdChanged(
        v3::CwdChanged {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            cwd,
            runtime_revision,
        },
    ))
}

/// Build a v3 `TitleChanged` push envelope.
#[must_use]
pub fn v3_title_changed(
    runtime_id: Uuid,
    pane_id: Uuid,
    title: String,
    runtime_revision: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::TitleChanged(
        v3::TitleChanged {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            title,
            runtime_revision,
        },
    ))
}

/// Build a v3 `PaneExited` push envelope.
#[must_use]
pub fn v3_pane_exited(
    runtime_id: Uuid,
    pane_id: Uuid,
    status: i32,
    runtime_revision: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::PaneExited(
        v3::PaneExited {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            status,
            runtime_revision,
        },
    ))
}

/// Build a v3 `RuntimeTerminated` push envelope.
#[must_use]
pub fn v3_runtime_terminated(
    runtime_id: Uuid,
    final_revision: u64,
    reason: TerminationReason,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::RuntimeTerminated(
        v3::RuntimeTerminated {
            runtime_id: uuid_to_bytes(runtime_id),
            final_revision,
            reason: reason.as_v3_proto() as i32,
        },
    ))
}

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

/// Build a v3 `RuntimeInfo` for a single runtime.
///
/// Public so callers with per-runtime locks can build inventory entries
/// one at a time.
#[must_use]
pub fn v3_runtime_info_for(
    client_id: Uuid,
    rt: &Runtime,
    has_inventory_v2: bool,
) -> v3::RuntimeInfo {
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
                        no_persist: pane.no_persist,
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
                command_history_len: 0,
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
        total_command_history: 0,
        runtimes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_cwd_changed_message_contains_correct_fields() {
        let sid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let env = v3_cwd_changed(sid, pid, "/home/user".into(), 42);
        let Some(v3::server_envelope::Payload::CwdChanged(inner)) = env.payload else {
            panic!("expected CwdChanged");
        };
        assert_eq!(inner.cwd, "/home/user");
        assert_eq!(inner.runtime_revision, 42);
        assert_eq!(inner.runtime_id, sid.as_bytes().to_vec());
        assert_eq!(inner.pane_id, pid.as_bytes().to_vec());
    }

    #[test]
    fn v3_delta_builds_output_delta_push_envelope() {
        let rid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let env = v3_delta(rid, pid, bytes::Bytes::from_static(b"payload"), 7);
        // Push events carry request_id 0.
        assert_eq!(env.request_id, 0);
        let Some(v3::server_envelope::Payload::OutputDelta(d)) = env.payload else {
            panic!("expected OutputDelta");
        };
        assert_eq!(d.runtime_id, rid.as_bytes().to_vec());
        assert_eq!(d.pane_id, pid.as_bytes().to_vec());
        assert_eq!(d.data.as_ref(), b"payload");
        assert_eq!(d.pane_output_seq, 7);
    }

    #[test]
    fn v3_pane_exited_carries_status_and_revision() {
        let env = v3_pane_exited(Uuid::new_v4(), Uuid::new_v4(), 137, 9);
        let Some(v3::server_envelope::Payload::PaneExited(p)) = env.payload else {
            panic!("expected PaneExited");
        };
        assert_eq!(p.status, 137);
        assert_eq!(p.runtime_revision, 9);
    }

    #[test]
    fn v3_runtime_terminated_maps_reason() {
        let env = v3_runtime_terminated(Uuid::new_v4(), 3, TerminationReason::EphemeralLastDetach);
        let Some(v3::server_envelope::Payload::RuntimeTerminated(t)) = env.payload else {
            panic!("expected RuntimeTerminated");
        };
        assert_eq!(t.final_revision, 3);
        assert_eq!(t.reason, v3::RuntimeTerminationReason::EphemeralDetach as i32);
    }
}
