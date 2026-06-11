//! V3 diagnostics: capability gating, builders, and report construction.
//!
//! Implements RFC-021 Section 3 (`OPT_DIAGNOSTICS`).
//!
//! `GetDiagnostics` is a request/response command (uses `request_id`).
//! The server responds with a `DiagnosticsReport` containing per-workspace
//! and per-pane memory and state metrics. Without `OPT_DIAGNOSTICS`, the
//! client disables the diagnostics UI.

use crate::v3;

/// Check whether `OPT_DIAGNOSTICS` is in the effective capability set.
#[must_use]
pub fn is_supported(effective_caps: &[i32]) -> bool {
    effective_caps.contains(&(v3::Capability::OptDiagnostics as i32))
}

/// Build a `GetDiagnostics` request.
#[must_use]
pub fn build_get_diagnostics() -> v3::GetDiagnostics {
    v3::GetDiagnostics {}
}

/// Build a `ClientEnvelope` for a `GetDiagnostics` request.
#[must_use]
pub fn build_get_diagnostics_envelope(
    id_gen: &crate::v3_envelope::RequestIdGenerator,
) -> v3::ClientEnvelope {
    crate::v3_envelope::build_client_envelope(
        id_gen,
        v3::client_envelope::Command::GetDiagnostics(build_get_diagnostics()),
    )
}

/// Build a `PaneDiagnosticsInfo`.
#[must_use]
pub fn build_pane_diagnostics_info(
    pane_id: uuid::Uuid,
    raw_bytes_len: u64,
    pending_flush_len: u64,
    is_exited: bool,
) -> v3::PaneDiagnosticsInfo {
    v3::PaneDiagnosticsInfo {
        id: crate::uuid_to_bytes(pane_id),
        raw_bytes_len,
        pending_flush_len,
        is_exited,
    }
}

/// Build a `WorkspaceDiagnosticsInfo`.
#[must_use]
pub fn build_workspace_diagnostics_info(
    runtime_id: uuid::Uuid,
    name: String,
    active_pane_count: u32,
    exited_pane_count: u32,
    attached_client_count: u32,
    panes: Vec<v3::PaneDiagnosticsInfo>,
) -> v3::WorkspaceDiagnosticsInfo {
    v3::WorkspaceDiagnosticsInfo {
        id: crate::uuid_to_bytes(runtime_id),
        name,
        active_pane_count,
        exited_pane_count,
        command_history_len: 0,
        attached_client_count,
        panes,
    }
}

/// Build a `DiagnosticsReport`.
///
/// Accepts a `DiagnosticsReportArgs` struct to avoid excessive function parameters.
#[must_use]
pub fn build_diagnostics_report(args: DiagnosticsReportArgs) -> v3::DiagnosticsReport {
    v3::DiagnosticsReport {
        workspace_count: args.workspace_count,
        total_pane_count: args.total_pane_count,
        total_active_panes: args.total_active_panes,
        total_exited_panes: args.total_exited_panes,
        client_count: args.client_count,
        pty_writer_count: args.pty_writer_count,
        total_raw_bytes: args.total_raw_bytes,
        total_pending_flush: args.total_pending_flush,
        total_command_history: 0,
        workspaces: args.workspaces,
    }
}

/// Arguments for [`build_diagnostics_report`].
pub struct DiagnosticsReportArgs {
    pub workspace_count: u32,
    pub total_pane_count: u32,
    pub total_active_panes: u32,
    pub total_exited_panes: u32,
    pub client_count: u32,
    pub pty_writer_count: u32,
    pub total_raw_bytes: u64,
    pub total_pending_flush: u64,
    pub workspaces: Vec<v3::WorkspaceDiagnosticsInfo>,
}

/// Build a `ServerEnvelope` response containing a `DiagnosticsReport`.
#[must_use]
pub fn build_diagnostics_report_response(
    request_id: u64,
    report: v3::DiagnosticsReport,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::DiagnosticsReport(report),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame, uuid_to_bytes, v3_envelope::RequestIdGenerator};
    use bytes::BytesMut;

    fn rt() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn pn() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    // ── is_supported ──

    #[test]
    fn supported_when_capability_present() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::OptDiagnostics as i32,
        ];
        assert!(is_supported(&caps));
    }

    #[test]
    fn not_supported_when_capability_absent() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::OptChunkedScrollback as i32,
        ];
        assert!(!is_supported(&caps));
    }

    #[test]
    fn not_supported_with_empty_caps() {
        assert!(!is_supported(&[]));
    }

    // ── build_get_diagnostics ──

    #[test]
    fn get_diagnostics_wire_roundtrip() {
        let req = build_get_diagnostics();
        let mut buf = BytesMut::new();
        encode_frame(&req, &mut buf).unwrap();
        let decoded: v3::GetDiagnostics = decode_frame(&mut buf).unwrap();
        assert_eq!(req, decoded);
    }

    // ── build_get_diagnostics_envelope ──

    #[test]
    fn get_diagnostics_envelope_has_nonzero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let env = build_get_diagnostics_envelope(&id_gen);
        assert_ne!(env.request_id, 0);
    }

    #[test]
    fn get_diagnostics_envelope_contains_correct_command() {
        let id_gen = RequestIdGenerator::new();
        let env = build_get_diagnostics_envelope(&id_gen);
        assert!(matches!(env.command, Some(v3::client_envelope::Command::GetDiagnostics(_))));
    }

    #[test]
    fn get_diagnostics_envelope_wire_roundtrip() {
        let id_gen = RequestIdGenerator::new();
        let env = build_get_diagnostics_envelope(&id_gen);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_pane_diagnostics_info ──

    #[test]
    fn pane_diagnostics_info_populates_all_fields() {
        let p = pn();
        let info = build_pane_diagnostics_info(p, 4096, 128, false);
        assert_eq!(info.id, uuid_to_bytes(p));
        assert_eq!(info.raw_bytes_len, 4096);
        assert_eq!(info.pending_flush_len, 128);
        assert!(!info.is_exited);
    }

    #[test]
    fn pane_diagnostics_info_exited() {
        let info = build_pane_diagnostics_info(pn(), 0, 0, true);
        assert!(info.is_exited);
    }

    #[test]
    fn pane_diagnostics_info_wire_roundtrip() {
        let info = build_pane_diagnostics_info(pn(), 1_000_000, 512, false);
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::PaneDiagnosticsInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── build_workspace_diagnostics_info ──

    #[test]
    fn workspace_diagnostics_info_populates_all_fields() {
        let r = rt();
        let p = pn();
        let pane = build_pane_diagnostics_info(p, 2048, 64, false);
        let info = build_workspace_diagnostics_info(r, "dev".into(), 2, 1, 1, vec![pane.clone()]);
        assert_eq!(info.id, uuid_to_bytes(r));
        assert_eq!(info.name, "dev");
        assert_eq!(info.active_pane_count, 2);
        assert_eq!(info.exited_pane_count, 1);
        assert_eq!(info.attached_client_count, 1);
        assert_eq!(info.panes.len(), 1);
        assert_eq!(info.panes[0], pane);
    }

    #[test]
    fn workspace_diagnostics_info_empty_panes() {
        let info = build_workspace_diagnostics_info(rt(), "empty".into(), 0, 0, 0, vec![]);
        assert!(info.panes.is_empty());
    }

    #[test]
    fn workspace_diagnostics_info_wire_roundtrip() {
        let pane = build_pane_diagnostics_info(pn(), 8192, 256, true);
        let info = build_workspace_diagnostics_info(rt(), "test-rt".into(), 3, 2, 2, vec![pane]);
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::WorkspaceDiagnosticsInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── build_diagnostics_report ──

    #[test]
    fn diagnostics_report_populates_all_fields() {
        let pane = build_pane_diagnostics_info(pn(), 4096, 128, false);
        let workspace = build_workspace_diagnostics_info(rt(), "ws1".into(), 1, 0, 1, vec![pane]);
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 1,
            total_pane_count: 1,
            total_active_panes: 1,
            total_exited_panes: 0,
            client_count: 1,
            pty_writer_count: 1,
            total_raw_bytes: 4096,
            total_pending_flush: 128,
            workspaces: vec![workspace],
        });
        assert_eq!(report.workspace_count, 1);
        assert_eq!(report.total_pane_count, 1);
        assert_eq!(report.total_active_panes, 1);
        assert_eq!(report.total_exited_panes, 0);
        assert_eq!(report.client_count, 1);
        assert_eq!(report.pty_writer_count, 1);
        assert_eq!(report.total_raw_bytes, 4096);
        assert_eq!(report.total_pending_flush, 128);
        assert_eq!(report.workspaces.len(), 1);
    }

    #[test]
    fn diagnostics_report_empty_server() {
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 0,
            total_pane_count: 0,
            total_active_panes: 0,
            total_exited_panes: 0,
            client_count: 0,
            pty_writer_count: 0,
            total_raw_bytes: 0,
            total_pending_flush: 0,
            workspaces: vec![],
        });
        assert_eq!(report.workspace_count, 0);
        assert!(report.workspaces.is_empty());
    }

    #[test]
    fn diagnostics_report_wire_roundtrip() {
        let pane1 = build_pane_diagnostics_info(pn(), 1024, 0, false);
        let pane2 = build_pane_diagnostics_info(pn(), 2048, 512, true);
        let workspace = build_workspace_diagnostics_info(
            rt(),
            "multi-pane".into(),
            1,
            1,
            2,
            vec![pane1, pane2],
        );
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 1,
            total_pane_count: 2,
            total_active_panes: 1,
            total_exited_panes: 1,
            client_count: 2,
            pty_writer_count: 1,
            total_raw_bytes: 3072,
            total_pending_flush: 512,
            workspaces: vec![workspace],
        });
        let mut buf = BytesMut::new();
        encode_frame(&report, &mut buf).unwrap();
        let decoded: v3::DiagnosticsReport = decode_frame(&mut buf).unwrap();
        assert_eq!(report, decoded);
    }

    // ── build_diagnostics_report_response ──

    #[test]
    fn report_response_echoes_request_id() {
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 0,
            total_pane_count: 0,
            total_active_panes: 0,
            total_exited_panes: 0,
            client_count: 0,
            pty_writer_count: 0,
            total_raw_bytes: 0,
            total_pending_flush: 0,
            workspaces: vec![],
        });
        let env = build_diagnostics_report_response(42, report);
        assert_eq!(env.request_id, 42);
    }

    #[test]
    fn report_response_contains_correct_payload() {
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 2,
            total_pane_count: 5,
            total_active_panes: 3,
            total_exited_panes: 2,
            client_count: 1,
            pty_writer_count: 1,
            total_raw_bytes: 10000,
            total_pending_flush: 500,
            workspaces: vec![],
        });
        let env = build_diagnostics_report_response(7, report.clone());
        match env.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(ref r)) => {
                assert_eq!(r, &report);
            }
            _ => panic!("expected DiagnosticsReport payload"),
        }
    }

    #[test]
    fn report_response_is_not_push_event() {
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 0,
            total_pane_count: 0,
            total_active_panes: 0,
            total_exited_panes: 0,
            client_count: 0,
            pty_writer_count: 0,
            total_raw_bytes: 0,
            total_pending_flush: 0,
            workspaces: vec![],
        });
        let env = build_diagnostics_report_response(1, report);
        assert!(!crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn report_response_wire_roundtrip() {
        let pane = build_pane_diagnostics_info(pn(), 65536, 1024, false);
        let workspace =
            build_workspace_diagnostics_info(rt(), "roundtrip".into(), 1, 0, 1, vec![pane]);
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 1,
            total_pane_count: 1,
            total_active_panes: 1,
            total_exited_panes: 0,
            client_count: 1,
            pty_writer_count: 1,
            total_raw_bytes: 65536,
            total_pending_flush: 1024,
            workspaces: vec![workspace],
        });
        let env = build_diagnostics_report_response(99, report);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── Integration: unsupported capability error ──

    #[test]
    fn unsupported_capability_error_for_diagnostics() {
        let err = crate::v3_error::build_error(
            v3::ErrorKind::UnsupportedCapability,
            "OPT_DIAGNOSTICS not negotiated",
            "GetDiagnostics",
        );
        let env = crate::v3_error::build_error_response(42, err);
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
                assert_eq!(e.operation, "GetDiagnostics");
            }
            _ => panic!("expected Error payload"),
        }
    }

    // ── Integration: full diagnostics flow ──

    #[test]
    fn full_diagnostics_flow() {
        let id_gen = RequestIdGenerator::new();

        // 1. Client sends GetDiagnostics request
        let req_env = build_get_diagnostics_envelope(&id_gen);
        let saved_request_id = req_env.request_id;
        assert_ne!(saved_request_id, 0);

        // Verify it's a request/response command
        assert!(matches!(req_env.command, Some(v3::client_envelope::Command::GetDiagnostics(_))));

        // 2. Server builds a report with multiple workspaces and panes
        let p1 = build_pane_diagnostics_info(pn(), 4096, 128, false);
        let p2 = build_pane_diagnostics_info(pn(), 8192, 0, false);
        let p3 = build_pane_diagnostics_info(pn(), 0, 0, true);
        let rt1 = build_workspace_diagnostics_info(
            rt(),
            "workspace-1".into(),
            2,
            0,
            1,
            vec![p1.clone(), p2.clone()],
        );
        let rt2 =
            build_workspace_diagnostics_info(rt(), "workspace-2".into(), 0, 1, 0, vec![p3.clone()]);
        let report = build_diagnostics_report(DiagnosticsReportArgs {
            workspace_count: 2,
            total_pane_count: 3,
            total_active_panes: 2,
            total_exited_panes: 1,
            client_count: 1,
            pty_writer_count: 1,
            total_raw_bytes: p1.raw_bytes_len + p2.raw_bytes_len + p3.raw_bytes_len,
            total_pending_flush: p1.pending_flush_len + p2.pending_flush_len + p3.pending_flush_len,
            workspaces: vec![rt1, rt2],
        });

        // 3. Server responds with DiagnosticsReport
        let resp_env = build_diagnostics_report_response(saved_request_id, report);
        assert_eq!(resp_env.request_id, saved_request_id);
        assert!(!crate::v3_envelope::is_push_event(&resp_env));

        // 4. Verify report contents
        match resp_env.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(ref r)) => {
                assert_eq!(r.workspace_count, 2);
                assert_eq!(r.total_pane_count, 3);
                assert_eq!(r.total_active_panes, 2);
                assert_eq!(r.total_exited_panes, 1);
                assert_eq!(r.workspaces.len(), 2);
                assert_eq!(r.workspaces[0].panes.len(), 2);
                assert_eq!(r.workspaces[1].panes.len(), 1);
                assert!(r.workspaces[1].panes[0].is_exited);
            }
            _ => panic!("expected DiagnosticsReport"),
        }
    }

    // ── Integration: diagnostics disabled without capability ──

    #[test]
    fn diagnostics_disabled_without_capability() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::CorePaneLifecycle as i32,
            v3::Capability::CoreTerminalIo as i32,
        ];
        assert!(!is_supported(&caps));
    }

    // ── Integration: diagnostics enabled with capability ──

    #[test]
    fn diagnostics_enabled_with_full_capability_set() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::CorePaneLifecycle as i32,
            v3::Capability::CoreTerminalIo as i32,
            v3::Capability::CoreTerminalModes as i32,
            v3::Capability::CorePasteIntent as i32,
            v3::Capability::CoreFocusEvents as i32,
            v3::Capability::OptDiagnostics as i32,
        ];
        assert!(is_supported(&caps));
    }
}
