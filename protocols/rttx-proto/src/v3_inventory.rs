//! V3 runtime inventory: capability gating, builders, and field stripping.
//!
//! Implements RFC-021 Section 9 (`OPT_RUNTIME_INVENTORY_V2`).
//!
//! `ListRuntimes` always returns core fields (id, name, policy, pane_count,
//! ownership, revision). When `OPT_RUNTIME_INVENTORY_V2` is negotiated, the
//! server additionally populates `active_pane_summary`, `takeover_eligible`,
//! `disabled_reason`, and `panes`.
//!
//! Without the capability, the client shows basic inventory (name, pane count,
//! attached status). With it, the client can display rich detail and disable
//! busy runtimes with an explanation.

use crate::v3;

/// Check whether `OPT_RUNTIME_INVENTORY_V2` is in the effective capability set.
#[must_use]
pub fn is_supported(effective_caps: &[i32]) -> bool {
    effective_caps.contains(&(v3::Capability::OptRuntimeInventoryV2 as i32))
}

/// Parameters for building a `PaneInfo`.
pub struct PaneInfoParams {
    pub pane_id: uuid::Uuid,
    pub title: String,
    pub cwd: String,
    pub cols: u32,
    pub rows: u32,
    pub exit_status: Option<i32>,
    pub reconstructed: bool,
}

/// Build a `PaneInfo` message.
#[must_use]
pub fn build_pane_info(params: PaneInfoParams) -> v3::PaneInfo {
    v3::PaneInfo {
        id: crate::uuid_to_bytes(params.pane_id),
        title: params.title,
        cwd: params.cwd,
        cols: params.cols,
        rows: params.rows,
        exit_status: params.exit_status,
        reconstructed: params.reconstructed,
    }
}

/// Core fields for building a `RuntimeInfo`.
pub struct RuntimeInfoParams {
    pub id: uuid::Uuid,
    pub name: String,
    pub policy: v3::RuntimePolicy,
    pub pane_count: u32,
    pub has_write_owner: bool,
    pub read_only_client_count: u32,
    pub current_client_role: v3::RuntimeClientRole,
    pub runtime_revision: u64,
    pub reconstructed: bool,
}

/// V2 enrichment fields for `RuntimeInfo`.
pub struct RuntimeInfoV2Fields {
    pub active_pane_summary: String,
    pub takeover_eligible: bool,
    pub disabled_reason: String,
    pub panes: Vec<v3::PaneInfo>,
}

/// Build a `RuntimeInfo` with core fields only.
#[must_use]
pub fn build_runtime_info(params: RuntimeInfoParams) -> v3::RuntimeInfo {
    v3::RuntimeInfo {
        id: crate::uuid_to_bytes(params.id),
        name: params.name,
        policy: params.policy as i32,
        pane_count: params.pane_count,
        has_write_owner: params.has_write_owner,
        read_only_client_count: params.read_only_client_count,
        current_client_role: params.current_client_role as i32,
        runtime_revision: params.runtime_revision,
        reconstructed: params.reconstructed,
        active_pane_summary: String::new(),
        takeover_eligible: false,
        disabled_reason: String::new(),
        panes: vec![],
    }
}

/// Build a `RuntimeInfo` with V2 enriched fields.
#[must_use]
pub fn build_runtime_info_v2(
    params: RuntimeInfoParams,
    v2: RuntimeInfoV2Fields,
) -> v3::RuntimeInfo {
    v3::RuntimeInfo {
        id: crate::uuid_to_bytes(params.id),
        name: params.name,
        policy: params.policy as i32,
        pane_count: params.pane_count,
        has_write_owner: params.has_write_owner,
        read_only_client_count: params.read_only_client_count,
        current_client_role: params.current_client_role as i32,
        runtime_revision: params.runtime_revision,
        reconstructed: params.reconstructed,
        active_pane_summary: v2.active_pane_summary,
        takeover_eligible: v2.takeover_eligible,
        disabled_reason: v2.disabled_reason,
        panes: v2.panes,
    }
}

/// Strip V2 fields from a `RuntimeInfo`, leaving only core fields.
///
/// Used by the server when the client did not negotiate
/// `OPT_RUNTIME_INVENTORY_V2`.
pub fn strip_inventory_v2_fields(info: &mut v3::RuntimeInfo) {
    info.active_pane_summary.clear();
    info.takeover_eligible = false;
    info.disabled_reason.clear();
    info.panes.clear();
}

/// Build a summary string from pane titles (e.g. "bash, vim, htop").
///
/// Deduplicates titles and joins with ", ". Empty titles are skipped.
#[must_use]
pub fn build_active_pane_summary(panes: &[v3::PaneInfo]) -> String {
    let mut seen = Vec::new();
    for pane in panes {
        if pane.exit_status.is_some() || pane.title.is_empty() {
            continue;
        }
        if !seen.contains(&pane.title) {
            seen.push(pane.title.clone());
        }
    }
    seen.join(", ")
}

/// Build a `RuntimeList` response.
#[must_use]
pub fn build_runtime_list(runtimes: Vec<v3::RuntimeInfo>) -> v3::RuntimeList {
    v3::RuntimeList { runtimes }
}

/// Build a `ServerEnvelope` response containing a `RuntimeList`.
#[must_use]
pub fn build_runtime_list_response(
    request_id: u64,
    list: v3::RuntimeList,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::RuntimeList(list),
    )
}

/// Build a `ClientEnvelope` for a `ListRuntimes` request.
#[must_use]
pub fn build_list_runtimes_envelope(
    id_gen: &crate::v3_envelope::RequestIdGenerator,
) -> v3::ClientEnvelope {
    crate::v3_envelope::build_client_envelope(
        id_gen,
        v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {}),
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

    fn test_pane(title: &str, cwd: &str, exit_status: Option<i32>) -> v3::PaneInfo {
        build_pane_info(PaneInfoParams {
            pane_id: pn(),
            title: title.into(),
            cwd: cwd.into(),
            cols: 80,
            rows: 24,
            exit_status,
            reconstructed: false,
        })
    }

    fn core_params(name: &str) -> RuntimeInfoParams {
        RuntimeInfoParams {
            id: rt(),
            name: name.into(),
            policy: v3::RuntimePolicy::Persistent,
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            current_client_role: v3::RuntimeClientRole::Unattached,
            runtime_revision: 0,
            reconstructed: false,
        }
    }

    // ── is_supported ──

    #[test]
    fn supported_when_capability_present() {
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptRuntimeInventoryV2 as i32,
        ];
        assert!(is_supported(&caps));
    }

    #[test]
    fn not_supported_when_capability_absent() {
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptDiagnostics as i32,
        ];
        assert!(!is_supported(&caps));
    }

    #[test]
    fn not_supported_with_empty_caps() {
        assert!(!is_supported(&[]));
    }

    // ── build_pane_info ──

    #[test]
    fn pane_info_populates_all_fields() {
        let p = pn();
        let info = build_pane_info(PaneInfoParams {
            pane_id: p,
            title: "bash".into(),
            cwd: "/home/user".into(),
            cols: 80,
            rows: 24,
            exit_status: None,
            reconstructed: false,
        });
        assert_eq!(info.id, uuid_to_bytes(p));
        assert_eq!(info.title, "bash");
        assert_eq!(info.cwd, "/home/user");
        assert_eq!(info.cols, 80);
        assert_eq!(info.rows, 24);
        assert_eq!(info.exit_status, None);
        assert!(!info.reconstructed);
    }

    #[test]
    fn pane_info_with_exit_status() {
        let info = build_pane_info(PaneInfoParams {
            pane_id: pn(),
            title: "done".into(),
            cwd: "/tmp".into(),
            cols: 120,
            rows: 40,
            exit_status: Some(0),
            reconstructed: false,
        });
        assert_eq!(info.exit_status, Some(0));
    }

    #[test]
    fn pane_info_reconstructed() {
        let info = build_pane_info(PaneInfoParams {
            pane_id: pn(),
            title: "shell".into(),
            cwd: "/".into(),
            cols: 80,
            rows: 24,
            exit_status: None,
            reconstructed: true,
        });
        assert!(info.reconstructed);
    }

    #[test]
    fn pane_info_wire_roundtrip() {
        let info = build_pane_info(PaneInfoParams {
            pane_id: pn(),
            title: "vim".into(),
            cwd: "/home/user/src".into(),
            cols: 120,
            rows: 40,
            exit_status: Some(1),
            reconstructed: true,
        });
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::PaneInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── build_runtime_info (core only) ──

    #[test]
    fn runtime_info_core_fields() {
        let r = rt();
        let info = build_runtime_info(RuntimeInfoParams {
            id: r,
            name: "workspace-1".into(),
            policy: v3::RuntimePolicy::Persistent,
            pane_count: 3,
            has_write_owner: true,
            read_only_client_count: 1,
            current_client_role: v3::RuntimeClientRole::Writer,
            runtime_revision: 42,
            reconstructed: false,
        });
        assert_eq!(info.id, uuid_to_bytes(r));
        assert_eq!(info.name, "workspace-1");
        assert_eq!(info.policy, v3::RuntimePolicy::Persistent as i32);
        assert_eq!(info.pane_count, 3);
        assert!(info.has_write_owner);
        assert_eq!(info.read_only_client_count, 1);
        assert_eq!(info.current_client_role, v3::RuntimeClientRole::Writer as i32);
        assert_eq!(info.runtime_revision, 42);
        assert!(!info.reconstructed);
        // V2 fields are empty/default
        assert!(info.active_pane_summary.is_empty());
        assert!(!info.takeover_eligible);
        assert!(info.disabled_reason.is_empty());
        assert!(info.panes.is_empty());
    }

    #[test]
    fn runtime_info_core_wire_roundtrip() {
        let info = build_runtime_info(RuntimeInfoParams {
            id: rt(),
            name: "test".into(),
            policy: v3::RuntimePolicy::Ephemeral,
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            current_client_role: v3::RuntimeClientRole::Unattached,
            runtime_revision: 0,
            reconstructed: true,
        });
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::RuntimeInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── build_runtime_info_v2 ──

    #[test]
    fn runtime_info_v2_populates_all_fields() {
        let r = rt();
        let pane = test_pane("bash", "/home", None);
        let info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: r,
                name: "dev".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::RuntimeClientRole::Writer,
                runtime_revision: 10,
                reconstructed: false,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "bash".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );
        assert_eq!(info.active_pane_summary, "bash");
        assert!(info.takeover_eligible);
        assert!(info.disabled_reason.is_empty());
        assert_eq!(info.panes.len(), 1);
        assert_eq!(info.panes[0].title, "bash");
    }

    #[test]
    fn runtime_info_v2_with_disabled_reason() {
        let info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "busy".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 2,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::RuntimeClientRole::Unattached,
                runtime_revision: 5,
                reconstructed: false,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "vim".into(),
                takeover_eligible: false,
                disabled_reason: "owned by another client".into(),
                panes: vec![],
            },
        );
        assert_eq!(info.disabled_reason, "owned by another client");
        assert!(!info.takeover_eligible);
    }

    #[test]
    fn runtime_info_v2_wire_roundtrip() {
        let pane = test_pane("htop", "/", None);
        let info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "monitor".into(),
                policy: v3::RuntimePolicy::Ephemeral,
                pane_count: 1,
                has_write_owner: false,
                read_only_client_count: 2,
                current_client_role: v3::RuntimeClientRole::Reader,
                runtime_revision: 99,
                reconstructed: true,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "htop".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::RuntimeInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── strip_inventory_v2_fields ──

    #[test]
    fn strip_clears_v2_fields() {
        let pane = test_pane("bash", "/home", None);
        let mut info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "ws".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::RuntimeClientRole::Writer,
                runtime_revision: 1,
                reconstructed: false,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "bash".into(),
                takeover_eligible: true,
                disabled_reason: "busy".into(),
                panes: vec![pane],
            },
        );
        strip_inventory_v2_fields(&mut info);
        assert!(info.active_pane_summary.is_empty());
        assert!(!info.takeover_eligible);
        assert!(info.disabled_reason.is_empty());
        assert!(info.panes.is_empty());
        // Core fields preserved
        assert_eq!(info.name, "ws");
        assert_eq!(info.pane_count, 1);
        assert!(info.has_write_owner);
    }

    #[test]
    fn strip_is_idempotent() {
        let mut info = build_runtime_info(core_params("empty"));
        strip_inventory_v2_fields(&mut info);
        strip_inventory_v2_fields(&mut info);
        assert!(info.active_pane_summary.is_empty());
        assert!(info.panes.is_empty());
    }

    // ── build_active_pane_summary ──

    #[test]
    fn summary_from_multiple_panes() {
        let panes = vec![
            test_pane("bash", "/", None),
            test_pane("vim", "/src", None),
            test_pane("htop", "/", None),
        ];
        assert_eq!(build_active_pane_summary(&panes), "bash, vim, htop");
    }

    #[test]
    fn summary_deduplicates_titles() {
        let panes = vec![
            test_pane("bash", "/", None),
            test_pane("bash", "/home", None),
            test_pane("vim", "/src", None),
        ];
        assert_eq!(build_active_pane_summary(&panes), "bash, vim");
    }

    #[test]
    fn summary_skips_exited_panes() {
        let panes = vec![
            test_pane("bash", "/", None),
            test_pane("done", "/", Some(0)),
        ];
        assert_eq!(build_active_pane_summary(&panes), "bash");
    }

    #[test]
    fn summary_skips_empty_titles() {
        let panes = vec![
            test_pane("", "/", None),
            test_pane("bash", "/", None),
        ];
        assert_eq!(build_active_pane_summary(&panes), "bash");
    }

    #[test]
    fn summary_empty_when_no_active_panes() {
        let panes = vec![test_pane("done", "/", Some(1))];
        assert_eq!(build_active_pane_summary(&panes), "");
    }

    #[test]
    fn summary_empty_when_no_panes() {
        assert_eq!(build_active_pane_summary(&[]), "");
    }

    // ── build_runtime_list ──

    #[test]
    fn runtime_list_contains_runtimes() {
        let info = build_runtime_info(core_params("ws"));
        let list = build_runtime_list(vec![info]);
        assert_eq!(list.runtimes.len(), 1);
        assert_eq!(list.runtimes[0].name, "ws");
    }

    #[test]
    fn runtime_list_empty() {
        let list = build_runtime_list(vec![]);
        assert!(list.runtimes.is_empty());
    }

    #[test]
    fn runtime_list_wire_roundtrip() {
        let info = build_runtime_info(RuntimeInfoParams {
            id: rt(),
            name: "test".into(),
            policy: v3::RuntimePolicy::Ephemeral,
            pane_count: 2,
            has_write_owner: true,
            read_only_client_count: 1,
            current_client_role: v3::RuntimeClientRole::Writer,
            runtime_revision: 5,
            reconstructed: false,
        });
        let list = build_runtime_list(vec![info]);
        let mut buf = BytesMut::new();
        encode_frame(&list, &mut buf).unwrap();
        let decoded: v3::RuntimeList = decode_frame(&mut buf).unwrap();
        assert_eq!(list, decoded);
    }

    // ── build_runtime_list_response ──

    #[test]
    fn list_response_echoes_request_id() {
        let list = build_runtime_list(vec![]);
        let env = build_runtime_list_response(42, list);
        assert_eq!(env.request_id, 42);
    }

    #[test]
    fn list_response_contains_runtime_list_payload() {
        let info = build_runtime_info(core_params("ws"));
        let list = build_runtime_list(vec![info]);
        let env = build_runtime_list_response(7, list);
        match env.payload {
            Some(v3::server_envelope::Payload::RuntimeList(ref rl)) => {
                assert_eq!(rl.runtimes.len(), 1);
                assert_eq!(rl.runtimes[0].name, "ws");
            }
            _ => panic!("expected RuntimeList payload"),
        }
    }

    #[test]
    fn list_response_is_not_push_event() {
        let list = build_runtime_list(vec![]);
        let env = build_runtime_list_response(1, list);
        assert!(!crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn list_response_wire_roundtrip() {
        let pane = test_pane("bash", "/home", None);
        let info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "full".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::RuntimeClientRole::Writer,
                runtime_revision: 10,
                reconstructed: false,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "bash".into(),
                takeover_eligible: false,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );
        let list = build_runtime_list(vec![info]);
        let env = build_runtime_list_response(99, list);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_list_runtimes_envelope ──

    #[test]
    fn list_runtimes_envelope_has_nonzero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let env = build_list_runtimes_envelope(&id_gen);
        assert_ne!(env.request_id, 0);
    }

    #[test]
    fn list_runtimes_envelope_contains_correct_command() {
        let id_gen = RequestIdGenerator::new();
        let env = build_list_runtimes_envelope(&id_gen);
        match env.command {
            Some(v3::client_envelope::Command::ListRuntimes(_)) => {}
            _ => panic!("expected ListRuntimes command"),
        }
    }

    #[test]
    fn list_runtimes_envelope_wire_roundtrip() {
        let id_gen = RequestIdGenerator::new();
        let env = build_list_runtimes_envelope(&id_gen);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── Integration: capability gating strips V2 fields ──

    #[test]
    fn capability_gating_strips_v2_when_absent() {
        let caps = vec![v3::Capability::CoreRuntimeLifecycle as i32];
        assert!(!is_supported(&caps));

        let pane = test_pane("bash", "/home", None);
        let mut info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "ws".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::RuntimeClientRole::Writer,
                runtime_revision: 1,
                reconstructed: false,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "bash".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );

        if !is_supported(&caps) {
            strip_inventory_v2_fields(&mut info);
        }

        assert!(info.active_pane_summary.is_empty());
        assert!(!info.takeover_eligible);
        assert!(info.panes.is_empty());
        // Core fields preserved
        assert_eq!(info.name, "ws");
        assert_eq!(info.pane_count, 1);
    }

    #[test]
    fn capability_gating_preserves_v2_when_present() {
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptRuntimeInventoryV2 as i32,
        ];
        assert!(is_supported(&caps));

        let pane = test_pane("vim", "/src", None);
        let info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "dev".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 1,
                has_write_owner: false,
                read_only_client_count: 1,
                current_client_role: v3::RuntimeClientRole::Reader,
                runtime_revision: 5,
                reconstructed: false,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "vim".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );

        assert_eq!(info.active_pane_summary, "vim");
        assert!(info.takeover_eligible);
        assert_eq!(info.panes.len(), 1);
    }

    // ── Integration: unsupported capability error ──

    #[test]
    fn unsupported_capability_error_for_inventory_v2() {
        let err = crate::v3_error::build_error(
            v3::ErrorKind::UnsupportedCapability,
            "OPT_RUNTIME_INVENTORY_V2 not negotiated",
            "ListRuntimes",
        );
        let env = crate::v3_error::build_error_response(42, err);
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
                assert_eq!(e.operation, "ListRuntimes");
            }
            _ => panic!("expected Error payload"),
        }
    }

    // ── Integration: full list flow with multiple runtimes ──

    #[test]
    fn full_list_flow_with_v2_fields() {
        let id_gen = RequestIdGenerator::new();

        let req_env = build_list_runtimes_envelope(&id_gen);
        let saved_request_id = req_env.request_id;
        assert_ne!(saved_request_id, 0);

        let pane1 = test_pane("bash", "/home", None);
        let pane2 = test_pane("vim", "/src", None);
        let panes = vec![pane1, pane2];
        let summary = build_active_pane_summary(&panes);

        let info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "dev-workspace".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 2,
                has_write_owner: true,
                read_only_client_count: 0,
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

        let list = build_runtime_list(vec![info]);
        let resp_env = build_runtime_list_response(saved_request_id, list);
        assert_eq!(resp_env.request_id, saved_request_id);

        match resp_env.payload {
            Some(v3::server_envelope::Payload::RuntimeList(ref rl)) => {
                assert_eq!(rl.runtimes.len(), 1);
                assert_eq!(rl.runtimes[0].active_pane_summary, "bash, vim");
                assert_eq!(rl.runtimes[0].panes.len(), 2);
            }
            _ => panic!("expected RuntimeList payload"),
        }
    }

    #[test]
    fn full_list_flow_without_v2_fields() {
        let id_gen = RequestIdGenerator::new();
        let caps = vec![v3::Capability::CoreRuntimeLifecycle as i32];

        let req_env = build_list_runtimes_envelope(&id_gen);
        let saved_request_id = req_env.request_id;

        let pane = test_pane("bash", "/home", None);
        let mut info = build_runtime_info_v2(
            RuntimeInfoParams {
                id: rt(),
                name: "ws".into(),
                policy: v3::RuntimePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::RuntimeClientRole::Writer,
                runtime_revision: 1,
                reconstructed: false,
            },
            RuntimeInfoV2Fields {
                active_pane_summary: "bash".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );

        if !is_supported(&caps) {
            strip_inventory_v2_fields(&mut info);
        }

        let list = build_runtime_list(vec![info]);
        let resp_env = build_runtime_list_response(saved_request_id, list);

        match resp_env.payload {
            Some(v3::server_envelope::Payload::RuntimeList(ref rl)) => {
                assert_eq!(rl.runtimes.len(), 1);
                assert!(rl.runtimes[0].active_pane_summary.is_empty());
                assert!(rl.runtimes[0].panes.is_empty());
                assert_eq!(rl.runtimes[0].name, "ws");
                assert_eq!(rl.runtimes[0].pane_count, 1);
            }
            _ => panic!("expected RuntimeList payload"),
        }
    }
}
