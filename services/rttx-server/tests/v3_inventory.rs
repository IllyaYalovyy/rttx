//! Integration tests for v3 `OPT_RUNTIME_INVENTORY_V2` protocol builders.
//!
//! Validates the end-to-end flow: build inventory with V2 fields, gate on
//! capability, strip when absent, and verify wire roundtrip through envelopes.

use rttx_proto::v3;
use rttx_proto::v3_envelope::RequestIdGenerator;
use rttx_proto::v3_inventory::{self, PaneInfoParams, RuntimeInfoParams, RuntimeInfoV2Fields};
use rttx_proto::{decode_frame, encode_frame};

fn rt() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

fn pn() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

fn test_pane(title: &str, cwd: &str, exit_status: Option<i32>) -> v3::PaneInfo {
    v3_inventory::build_pane_info(PaneInfoParams {
        pane_id: pn(),
        title: title.into(),
        cwd: cwd.into(),
        cols: 80,
        rows: 24,
        exit_status,
        reconstructed: false,
    })
}

#[test]
fn v3_inventory_list_roundtrip_with_v2_fields() {
    let id_gen = RequestIdGenerator::new();

    // Client sends ListRuntimes request
    let req_env = v3_inventory::build_list_runtimes_envelope(&id_gen);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&req_env, &mut buf).unwrap();
    let decoded_req: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(req_env, decoded_req);

    // Server builds inventory with V2 fields
    let panes = vec![
        test_pane("bash", "/home", None),
        test_pane("vim", "/src", None),
        test_pane("exited", "/tmp", Some(0)),
    ];
    let summary = v3_inventory::build_active_pane_summary(&panes);
    assert_eq!(summary, "bash, vim");

    let info = v3_inventory::build_runtime_info_v2(
        RuntimeInfoParams {
            id: rt(),
            name: "integration-test".into(),
            policy: v3::RuntimePolicy::Persistent,
            pane_count: 3,
            has_write_owner: true,
            read_only_client_count: 1,
            current_client_role: v3::RuntimeClientRole::Writer,
            runtime_revision: 42,
            reconstructed: false,
        },
        RuntimeInfoV2Fields {
            active_pane_summary: summary,
            takeover_eligible: false,
            disabled_reason: String::new(),
            panes,
        },
    );

    let list = v3_inventory::build_runtime_list(vec![info]);
    let resp_env = v3_inventory::build_runtime_list_response(decoded_req.request_id, list);

    // Wire roundtrip
    let mut buf = bytes::BytesMut::new();
    encode_frame(&resp_env, &mut buf).unwrap();
    let decoded_resp: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(resp_env, decoded_resp);

    // Verify payload
    let v3::server_envelope::Payload::RuntimeList(ref rl) = decoded_resp.payload.unwrap() else {
        panic!("expected RuntimeList payload");
    };
    assert_eq!(rl.runtimes.len(), 1);
    assert_eq!(rl.runtimes[0].active_pane_summary, "bash, vim");
    assert_eq!(rl.runtimes[0].panes.len(), 3);
    assert!(!rl.runtimes[0].takeover_eligible);
}

#[test]
fn v3_inventory_capability_gating_strips_v2_fields_end_to_end() {
    let caps_without_v2 =
        vec![v3::Capability::CoreRuntimeLifecycle as i32, v3::Capability::CorePaneLifecycle as i32];
    assert!(!v3_inventory::is_supported(&caps_without_v2));

    let pane = test_pane("bash", "/home", None);
    let mut info = v3_inventory::build_runtime_info_v2(
        RuntimeInfoParams {
            id: rt(),
            name: "gated".into(),
            policy: v3::RuntimePolicy::Persistent,
            pane_count: 1,
            has_write_owner: true,
            read_only_client_count: 0,
            current_client_role: v3::RuntimeClientRole::Writer,
            runtime_revision: 5,
            reconstructed: false,
        },
        RuntimeInfoV2Fields {
            active_pane_summary: "bash".into(),
            takeover_eligible: true,
            disabled_reason: String::new(),
            panes: vec![pane],
        },
    );

    v3_inventory::strip_inventory_v2_fields(&mut info);

    // V2 fields cleared
    assert!(info.active_pane_summary.is_empty());
    assert!(!info.takeover_eligible);
    assert!(info.panes.is_empty());

    // Core fields preserved
    assert_eq!(info.name, "gated");
    assert_eq!(info.pane_count, 1);
    assert!(info.has_write_owner);

    // Wire roundtrip after stripping
    let list = v3_inventory::build_runtime_list(vec![info]);
    let env = v3_inventory::build_runtime_list_response(1, list);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(env, decoded);
}

#[test]
fn v3_inventory_disabled_runtime_visible_with_explanation() {
    let caps_with_v2 = vec![
        v3::Capability::CoreRuntimeLifecycle as i32,
        v3::Capability::OptRuntimeInventoryV2 as i32,
    ];
    assert!(v3_inventory::is_supported(&caps_with_v2));

    let info = v3_inventory::build_runtime_info_v2(
        RuntimeInfoParams {
            id: rt(),
            name: "busy-runtime".into(),
            policy: v3::RuntimePolicy::Persistent,
            pane_count: 2,
            has_write_owner: true,
            read_only_client_count: 0,
            current_client_role: v3::RuntimeClientRole::Unattached,
            runtime_revision: 10,
            reconstructed: false,
        },
        RuntimeInfoV2Fields {
            active_pane_summary: "vim".into(),
            takeover_eligible: false,
            disabled_reason: "owned by another client".into(),
            panes: vec![test_pane("vim", "/src", None)],
        },
    );

    assert_eq!(info.disabled_reason, "owned by another client");
    assert!(!info.takeover_eligible);
    assert_eq!(info.active_pane_summary, "vim");
    assert_eq!(info.panes.len(), 1);
}
