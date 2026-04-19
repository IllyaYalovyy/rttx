//! Integration tests for v3 `OPT_DIAGNOSTICS` protocol builders.
//!
//! Validates the end-to-end flow: build diagnostics request, receive report,
//! gate on capability, and verify wire roundtrip through envelopes.

use rttx_proto::v3;
use rttx_proto::v3_diagnostics::{self, DiagnosticsReportArgs};
use rttx_proto::v3_envelope::RequestIdGenerator;
use rttx_proto::{decode_frame, encode_frame, uuid_to_bytes};

fn rt() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

fn pn() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

#[test]
fn v3_diagnostics_full_flow_wire_roundtrip() {
    let id_gen = RequestIdGenerator::new();

    // 1. Client sends GetDiagnostics request
    let req_env = v3_diagnostics::build_get_diagnostics_envelope(&id_gen);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&req_env, &mut buf).unwrap();
    let decoded_req: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(req_env, decoded_req);
    assert_ne!(decoded_req.request_id, 0);

    // 2. Server builds a report with runtimes and panes
    let p1 = v3_diagnostics::build_pane_diagnostics_info(pn(), 4096, 128, false);
    let p2 = v3_diagnostics::build_pane_diagnostics_info(pn(), 0, 0, true);
    let rt1 = v3_diagnostics::build_runtime_diagnostics_info(
        rt(),
        "workspace-1".into(),
        1,
        1,
        5,
        1,
        vec![p1, p2],
    );
    let report = v3_diagnostics::build_diagnostics_report(DiagnosticsReportArgs {
        runtime_count: 1,
        total_pane_count: 2,
        total_active_panes: 1,
        total_exited_panes: 1,
        client_count: 1,
        pty_writer_count: 1,
        total_raw_bytes: 4096,
        total_pending_flush: 128,
        total_command_history: 5,
        runtimes: vec![rt1],
    });

    // 3. Server responds with DiagnosticsReport
    let resp_env =
        v3_diagnostics::build_diagnostics_report_response(decoded_req.request_id, report);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&resp_env, &mut buf).unwrap();
    let decoded_resp: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(resp_env, decoded_resp);
    assert_eq!(decoded_resp.request_id, decoded_req.request_id);

    // Verify payload contents
    let v3::server_envelope::Payload::DiagnosticsReport(ref r) = decoded_resp.payload.unwrap()
    else {
        panic!("expected DiagnosticsReport payload");
    };
    assert_eq!(r.runtime_count, 1);
    assert_eq!(r.total_pane_count, 2);
    assert_eq!(r.runtimes.len(), 1);
    assert_eq!(r.runtimes[0].panes.len(), 2);
    assert!(r.runtimes[0].panes[1].is_exited);
}

#[test]
fn v3_diagnostics_capability_gating_rejects_when_absent() {
    let caps_without_diagnostics =
        vec![v3::Capability::CoreRuntimeLifecycle as i32, v3::Capability::CorePaneLifecycle as i32];
    assert!(!v3_diagnostics::is_supported(&caps_without_diagnostics));

    // Server returns UnsupportedCapability error
    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::UnsupportedCapability,
        "OPT_DIAGNOSTICS not negotiated",
        "GetDiagnostics",
    );
    let env = rttx_proto::v3_error::build_error_response(1, err);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let v3::server_envelope::Payload::Error(ref e) = decoded.payload.unwrap() else {
        panic!("expected Error payload");
    };
    assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
    assert_eq!(e.operation, "GetDiagnostics");
}

#[test]
fn v3_diagnostics_empty_server_report() {
    let id_gen = RequestIdGenerator::new();

    let req_env = v3_diagnostics::build_get_diagnostics_envelope(&id_gen);
    let report = v3_diagnostics::build_diagnostics_report(DiagnosticsReportArgs {
        runtime_count: 0,
        total_pane_count: 0,
        total_active_panes: 0,
        total_exited_panes: 0,
        client_count: 0,
        pty_writer_count: 0,
        total_raw_bytes: 0,
        total_pending_flush: 0,
        total_command_history: 0,
        runtimes: vec![],
    });
    let resp_env = v3_diagnostics::build_diagnostics_report_response(req_env.request_id, report);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&resp_env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let v3::server_envelope::Payload::DiagnosticsReport(ref r) = decoded.payload.unwrap() else {
        panic!("expected DiagnosticsReport payload");
    };
    assert_eq!(r.runtime_count, 0);
    assert!(r.runtimes.is_empty());
}

#[test]
fn v3_diagnostics_pane_info_preserves_uuid() {
    let p = pn();
    let info = v3_diagnostics::build_pane_diagnostics_info(p, 1024, 64, false);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&info, &mut buf).unwrap();
    let decoded: v3::PaneDiagnosticsInfo = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.id, uuid_to_bytes(p));
}
