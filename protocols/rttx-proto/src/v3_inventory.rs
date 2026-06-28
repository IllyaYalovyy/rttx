//! V3 workspace inventory: capability gating, builders, and field stripping.
//!
//! Implements RFC-021 Section 9 (`OPT_WORKSPACE_INVENTORY`).
//!
//! `ListWorkspaces` always returns core fields (id, name, policy, pane_count,
//! ownership, revision). When `OPT_WORKSPACE_INVENTORY` is negotiated, the
//! server additionally populates `active_pane_summary`, `takeover_eligible`,
//! `disabled_reason`, and `panes`.
//!
//! Without the capability, the client shows basic inventory (name, pane count,
//! attached status). With it, the client can display rich detail and disable
//! busy workspaces with an explanation.

use crate::v3;

/// Check whether `OPT_WORKSPACE_INVENTORY` is in the effective capability set.
#[must_use]
pub fn is_supported(effective_caps: &[i32]) -> bool {
    effective_caps.contains(&(v3::Capability::OptWorkspaceInventory as i32))
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
    pub no_persist: bool,
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
        no_persist: params.no_persist,
    }
}

/// Core fields for building a `WorkspaceInfo`.
pub struct WorkspaceInfoParams {
    pub id: uuid::Uuid,
    pub name: String,
    pub policy: v3::WorkspacePolicy,
    pub pane_count: u32,
    pub has_write_owner: bool,
    pub read_only_client_count: u32,
    pub current_client_role: v3::WorkspaceClientRole,
    pub workspace_revision: u64,
    pub reconstructed: bool,
}

/// Enriched fields for `WorkspaceInfo`.
pub struct WorkspaceInfoEnrichedFields {
    pub active_pane_summary: String,
    pub takeover_eligible: bool,
    pub disabled_reason: String,
    pub panes: Vec<v3::PaneInfo>,
}

/// Build a `WorkspaceInfo` with core fields only.
#[must_use]
pub fn build_workspace_info(params: WorkspaceInfoParams) -> v3::WorkspaceInfo {
    v3::WorkspaceInfo {
        id: crate::uuid_to_bytes(params.id),
        name: params.name,
        policy: params.policy as i32,
        pane_count: params.pane_count,
        has_write_owner: params.has_write_owner,
        read_only_client_count: params.read_only_client_count,
        current_client_role: params.current_client_role as i32,
        workspace_revision: params.workspace_revision,
        reconstructed: params.reconstructed,
        active_pane_summary: String::new(),
        takeover_eligible: false,
        disabled_reason: String::new(),
        panes: vec![],
    }
}

/// Build a `WorkspaceInfo` with enriched fields.
#[must_use]
pub fn build_workspace_info_enriched(
    params: WorkspaceInfoParams,
    enriched: WorkspaceInfoEnrichedFields,
) -> v3::WorkspaceInfo {
    v3::WorkspaceInfo {
        id: crate::uuid_to_bytes(params.id),
        name: params.name,
        policy: params.policy as i32,
        pane_count: params.pane_count,
        has_write_owner: params.has_write_owner,
        read_only_client_count: params.read_only_client_count,
        current_client_role: params.current_client_role as i32,
        workspace_revision: params.workspace_revision,
        reconstructed: params.reconstructed,
        active_pane_summary: enriched.active_pane_summary,
        takeover_eligible: enriched.takeover_eligible,
        disabled_reason: enriched.disabled_reason,
        panes: enriched.panes,
    }
}

/// Strip enriched fields from a `WorkspaceInfo`, leaving only core fields.
///
/// Used by the server when the client did not negotiate
/// `OPT_WORKSPACE_INVENTORY`.
pub fn strip_enriched_inventory_fields(info: &mut v3::WorkspaceInfo) {
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

/// Build a `WorkspaceList` response.
#[must_use]
pub fn build_workspace_list(workspaces: Vec<v3::WorkspaceInfo>) -> v3::WorkspaceList {
    v3::WorkspaceList { workspaces }
}

/// Build a `ServerEnvelope` response containing a `WorkspaceList`.
#[must_use]
pub fn build_workspace_list_response(
    request_id: u64,
    list: v3::WorkspaceList,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::WorkspaceList(list),
    )
}

/// Build a `ClientEnvelope` for a `ListWorkspaces` request.
#[must_use]
pub fn build_list_workspaces_envelope(
    id_gen: &crate::v3_envelope::RequestIdGenerator,
) -> v3::ClientEnvelope {
    crate::v3_envelope::build_client_envelope(
        id_gen,
        v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {}),
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
            no_persist: false,
        })
    }

    fn core_params(name: &str) -> WorkspaceInfoParams {
        WorkspaceInfoParams {
            id: rt(),
            name: name.into(),
            policy: v3::WorkspacePolicy::Persistent,
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            current_client_role: v3::WorkspaceClientRole::Unattached,
            workspace_revision: 0,
            reconstructed: false,
        }
    }

    // ── is_supported ──

    #[test]
    fn supported_when_capability_present() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::OptWorkspaceInventory as i32,
        ];
        assert!(is_supported(&caps));
    }

    #[test]
    fn not_supported_when_capability_absent() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
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
            no_persist: false,
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
            no_persist: false,
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
            no_persist: false,
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
            no_persist: false,
        });
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::PaneInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── build_workspace_info (core only) ──

    #[test]
    fn workspace_info_core_fields() {
        let r = rt();
        let info = build_workspace_info(WorkspaceInfoParams {
            id: r,
            name: "workspace-1".into(),
            policy: v3::WorkspacePolicy::Persistent,
            pane_count: 3,
            has_write_owner: true,
            read_only_client_count: 1,
            current_client_role: v3::WorkspaceClientRole::Writer,
            workspace_revision: 42,
            reconstructed: false,
        });
        assert_eq!(info.id, uuid_to_bytes(r));
        assert_eq!(info.name, "workspace-1");
        assert_eq!(info.policy, v3::WorkspacePolicy::Persistent as i32);
        assert_eq!(info.pane_count, 3);
        assert!(info.has_write_owner);
        assert_eq!(info.read_only_client_count, 1);
        assert_eq!(info.current_client_role, v3::WorkspaceClientRole::Writer as i32);
        assert_eq!(info.workspace_revision, 42);
        assert!(!info.reconstructed);
        // enriched fields are empty/default
        assert!(info.active_pane_summary.is_empty());
        assert!(!info.takeover_eligible);
        assert!(info.disabled_reason.is_empty());
        assert!(info.panes.is_empty());
    }

    #[test]
    fn workspace_info_core_wire_roundtrip() {
        let info = build_workspace_info(WorkspaceInfoParams {
            id: rt(),
            name: "test".into(),
            policy: v3::WorkspacePolicy::Ephemeral,
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            current_client_role: v3::WorkspaceClientRole::Unattached,
            workspace_revision: 0,
            reconstructed: true,
        });
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::WorkspaceInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── build_workspace_info_enriched ──

    #[test]
    fn workspace_info_enriched_populates_all_fields() {
        let r = rt();
        let pane = test_pane("bash", "/home", None);
        let info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: r,
                name: "dev".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Writer,
                workspace_revision: 10,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
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
    fn workspace_info_enriched_with_disabled_reason() {
        let info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "busy".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 2,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Unattached,
                workspace_revision: 5,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
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
    fn workspace_info_enriched_wire_roundtrip() {
        let pane = test_pane("htop", "/", None);
        let info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "monitor".into(),
                policy: v3::WorkspacePolicy::Ephemeral,
                pane_count: 1,
                has_write_owner: false,
                read_only_client_count: 2,
                current_client_role: v3::WorkspaceClientRole::Reader,
                workspace_revision: 99,
                reconstructed: true,
            },
            WorkspaceInfoEnrichedFields {
                active_pane_summary: "htop".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );
        let mut buf = BytesMut::new();
        encode_frame(&info, &mut buf).unwrap();
        let decoded: v3::WorkspaceInfo = decode_frame(&mut buf).unwrap();
        assert_eq!(info, decoded);
    }

    // ── strip_enriched_inventory_fields ──

    #[test]
    fn strip_clears_enriched_fields() {
        let pane = test_pane("bash", "/home", None);
        let mut info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "ws".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Writer,
                workspace_revision: 1,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
                active_pane_summary: "bash".into(),
                takeover_eligible: true,
                disabled_reason: "busy".into(),
                panes: vec![pane],
            },
        );
        strip_enriched_inventory_fields(&mut info);
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
        let mut info = build_workspace_info(core_params("empty"));
        strip_enriched_inventory_fields(&mut info);
        strip_enriched_inventory_fields(&mut info);
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
        let panes = vec![test_pane("bash", "/", None), test_pane("done", "/", Some(0))];
        assert_eq!(build_active_pane_summary(&panes), "bash");
    }

    #[test]
    fn summary_skips_empty_titles() {
        let panes = vec![test_pane("", "/", None), test_pane("bash", "/", None)];
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

    // ── build_workspace_list ──

    #[test]
    fn workspace_list_contains_workspaces() {
        let info = build_workspace_info(core_params("ws"));
        let list = build_workspace_list(vec![info]);
        assert_eq!(list.workspaces.len(), 1);
        assert_eq!(list.workspaces[0].name, "ws");
    }

    #[test]
    fn workspace_list_empty() {
        let list = build_workspace_list(vec![]);
        assert!(list.workspaces.is_empty());
    }

    #[test]
    fn workspace_list_wire_roundtrip() {
        let info = build_workspace_info(WorkspaceInfoParams {
            id: rt(),
            name: "test".into(),
            policy: v3::WorkspacePolicy::Ephemeral,
            pane_count: 2,
            has_write_owner: true,
            read_only_client_count: 1,
            current_client_role: v3::WorkspaceClientRole::Writer,
            workspace_revision: 5,
            reconstructed: false,
        });
        let list = build_workspace_list(vec![info]);
        let mut buf = BytesMut::new();
        encode_frame(&list, &mut buf).unwrap();
        let decoded: v3::WorkspaceList = decode_frame(&mut buf).unwrap();
        assert_eq!(list, decoded);
    }

    // ── build_workspace_list_response ──

    #[test]
    fn list_response_echoes_request_id() {
        let list = build_workspace_list(vec![]);
        let env = build_workspace_list_response(42, list);
        assert_eq!(env.request_id, 42);
    }

    #[test]
    fn list_response_contains_workspace_list_payload() {
        let info = build_workspace_info(core_params("ws"));
        let list = build_workspace_list(vec![info]);
        let env = build_workspace_list_response(7, list);
        match env.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(ref rl)) => {
                assert_eq!(rl.workspaces.len(), 1);
                assert_eq!(rl.workspaces[0].name, "ws");
            }
            _ => panic!("expected WorkspaceList payload"),
        }
    }

    #[test]
    fn list_response_is_not_push_event() {
        let list = build_workspace_list(vec![]);
        let env = build_workspace_list_response(1, list);
        assert!(!crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn list_response_wire_roundtrip() {
        let pane = test_pane("bash", "/home", None);
        let info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "full".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Writer,
                workspace_revision: 10,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
                active_pane_summary: "bash".into(),
                takeover_eligible: false,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );
        let list = build_workspace_list(vec![info]);
        let env = build_workspace_list_response(99, list);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_list_workspaces_envelope ──

    #[test]
    fn list_workspaces_envelope_has_nonzero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let env = build_list_workspaces_envelope(&id_gen);
        assert_ne!(env.request_id, 0);
    }

    #[test]
    fn list_workspaces_envelope_contains_correct_command() {
        let id_gen = RequestIdGenerator::new();
        let env = build_list_workspaces_envelope(&id_gen);
        match env.command {
            Some(v3::client_envelope::Command::ListWorkspaces(_)) => {}
            _ => panic!("expected ListWorkspaces command"),
        }
    }

    #[test]
    fn list_workspaces_envelope_wire_roundtrip() {
        let id_gen = RequestIdGenerator::new();
        let env = build_list_workspaces_envelope(&id_gen);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── Integration: capability gating strips enriched fields ──

    #[test]
    fn capability_gating_strips_enriched_when_absent() {
        let caps = vec![v3::Capability::CoreWorkspaceLifecycle as i32];
        assert!(!is_supported(&caps));

        let pane = test_pane("bash", "/home", None);
        let mut info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "ws".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Writer,
                workspace_revision: 1,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
                active_pane_summary: "bash".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );

        if !is_supported(&caps) {
            strip_enriched_inventory_fields(&mut info);
        }

        assert!(info.active_pane_summary.is_empty());
        assert!(!info.takeover_eligible);
        assert!(info.panes.is_empty());
        // Core fields preserved
        assert_eq!(info.name, "ws");
        assert_eq!(info.pane_count, 1);
    }

    #[test]
    fn capability_gating_preserves_enriched_when_present() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::OptWorkspaceInventory as i32,
        ];
        assert!(is_supported(&caps));

        let pane = test_pane("vim", "/src", None);
        let info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "dev".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 1,
                has_write_owner: false,
                read_only_client_count: 1,
                current_client_role: v3::WorkspaceClientRole::Reader,
                workspace_revision: 5,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
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
    fn unsupported_capability_error_for_inventory_enriched() {
        let err = crate::v3_error::build_error(
            v3::ErrorKind::UnsupportedCapability,
            "OPT_WORKSPACE_INVENTORY not negotiated",
            "ListWorkspaces",
        );
        let env = crate::v3_error::build_error_response(42, err);
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
                assert_eq!(e.operation, "ListWorkspaces");
            }
            _ => panic!("expected Error payload"),
        }
    }

    // ── Integration: full list flow with multiple workspaces ──

    #[test]
    fn full_list_flow_with_enriched_fields() {
        let id_gen = RequestIdGenerator::new();

        let req_env = build_list_workspaces_envelope(&id_gen);
        let saved_request_id = req_env.request_id;
        assert_ne!(saved_request_id, 0);

        let pane1 = test_pane("bash", "/home", None);
        let pane2 = test_pane("vim", "/src", None);
        let panes = vec![pane1, pane2];
        let summary = build_active_pane_summary(&panes);

        let info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "dev-workspace".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 2,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Writer,
                workspace_revision: 42,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
                active_pane_summary: summary,
                takeover_eligible: false,
                disabled_reason: String::new(),
                panes,
            },
        );

        let list = build_workspace_list(vec![info]);
        let resp_env = build_workspace_list_response(saved_request_id, list);
        assert_eq!(resp_env.request_id, saved_request_id);

        match resp_env.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(ref rl)) => {
                assert_eq!(rl.workspaces.len(), 1);
                assert_eq!(rl.workspaces[0].active_pane_summary, "bash, vim");
                assert_eq!(rl.workspaces[0].panes.len(), 2);
            }
            _ => panic!("expected WorkspaceList payload"),
        }
    }

    #[test]
    fn full_list_flow_without_enriched_fields() {
        let id_gen = RequestIdGenerator::new();
        let caps = vec![v3::Capability::CoreWorkspaceLifecycle as i32];

        let req_env = build_list_workspaces_envelope(&id_gen);
        let saved_request_id = req_env.request_id;

        let pane = test_pane("bash", "/home", None);
        let mut info = build_workspace_info_enriched(
            WorkspaceInfoParams {
                id: rt(),
                name: "ws".into(),
                policy: v3::WorkspacePolicy::Persistent,
                pane_count: 1,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Writer,
                workspace_revision: 1,
                reconstructed: false,
            },
            WorkspaceInfoEnrichedFields {
                active_pane_summary: "bash".into(),
                takeover_eligible: true,
                disabled_reason: String::new(),
                panes: vec![pane],
            },
        );

        if !is_supported(&caps) {
            strip_enriched_inventory_fields(&mut info);
        }

        let list = build_workspace_list(vec![info]);
        let resp_env = build_workspace_list_response(saved_request_id, list);

        match resp_env.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(ref rl)) => {
                assert_eq!(rl.workspaces.len(), 1);
                assert!(rl.workspaces[0].active_pane_summary.is_empty());
                assert!(rl.workspaces[0].panes.is_empty());
                assert_eq!(rl.workspaces[0].name, "ws");
                assert_eq!(rl.workspaces[0].pane_count, 1);
            }
            _ => panic!("expected WorkspaceList payload"),
        }
    }
}
