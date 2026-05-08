//! Export bundle and envelope for single-file configuration backup (RFC-029 §2, §6).

use serde::{Deserialize, Serialize};

use super::hosts::HostCatalog;
use super::library::Library;
use super::preferences::PreferencesV1;
use super::workspaces::WorkspaceStore;
use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Export;
pub const CURRENT_VERSION: u32 = 1;

/// All exportable client configuration domains in a single bundle.
///
/// Each field is optional so partial exports are possible.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ExportBundle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferences: Option<PreferencesV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<Library>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<HostCatalog>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<WorkspaceStore>,
}

/// Self-describing envelope for a configuration export file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportEnvelope {
    pub schema: Schema,
    pub version: u32,
    pub app_version: String,
    pub exported_at: String,
    pub data: ExportBundle,
}

impl ExportEnvelope {
    /// Create a new export envelope stamped with the current app version and time.
    #[must_use]
    pub fn new(data: ExportBundle) -> Self {
        Self {
            schema: SCHEMA,
            version: CURRENT_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            exported_at: crate::store::envelope::now_iso8601(),
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::commands::RunMode;
    use crate::store::models::hosts::{HostKind, HostRecord};
    use crate::store::models::library::{CommandRecord, PlaceRecord};
    use crate::store::models::workspaces::{
        InputSyncState, LayoutNode, WorkspaceColor, WorkspacePolicy, WorkspaceRecord,
    };

    #[test]
    fn export_bundle_round_trips_populated() {
        let bundle = ExportBundle {
            preferences: Some(PreferencesV1::default()),
            library: Some(Library {
                places: vec![PlaceRecord {
                    id: "p1".into(),
                    name: "Home".into(),
                    path: "~".into(),
                    host_tags: vec![],
                }],
                commands: vec![CommandRecord {
                    id: "c1".into(),
                    title: "Build".into(),
                    body: "cargo build".into(),
                    default_run_mode: RunMode::Run,
                    host_tags: vec![],
                    parameters: vec![],
                    description: String::new(),
                    labels: vec![],
                    shortcut_keys: vec![],
                }],
            }),
            hosts: Some(HostCatalog {
                hosts: vec![HostRecord {
                    key: "example.com".into(),
                    name: "Example".into(),
                    kind: HostKind::default(),
                    ssh_target: Some("user@example.com".into()),
                    labels: vec![],
                }],
            }),
            workspaces: Some(WorkspaceStore {
                active_workspace_id: Some("ws-1".into()),
                workspaces: vec![WorkspaceRecord {
                    id: "ws-1".into(),
                    name: "Dev".into(),
                    user_renamed: false,
                    endpoint_key: "local".into(),
                    policy: WorkspacePolicy::Ephemeral,
                    runtime_ref: None,
                    layout: LayoutNode::Terminal {
                        uuid: "t-1".into(),
                        profile: None,
                        cwd: Some("/home/user".into()),
                        custom_title: None,
                    },
                    active_pane_id: Some("t-1".into()),
                    zoomed_pane_id: None,
                    input_sync: InputSyncState::Off,
                    color: WorkspaceColor::Blue,
                    pane_recovery: std::collections::BTreeMap::new(),
                }],
            }),
        };

        let envelope = ExportEnvelope::new(bundle.clone());
        assert_eq!(envelope.schema, Schema::Export);
        assert_eq!(envelope.version, 1);
        assert_eq!(envelope.app_version, env!("CARGO_PKG_VERSION"));

        let json = serde_json::to_string_pretty(&envelope).unwrap();
        let loaded: ExportEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.schema, Schema::Export);
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.data, bundle);
    }

    #[test]
    fn export_bundle_round_trips_empty() {
        let bundle = ExportBundle::default();
        let envelope = ExportEnvelope::new(bundle);
        let json = serde_json::to_string(&envelope).unwrap();
        let loaded: ExportEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.data.preferences, None);
        assert_eq!(loaded.data.library, None);
        assert_eq!(loaded.data.hosts, None);
        assert_eq!(loaded.data.workspaces, None);
    }

    #[test]
    fn export_bundle_partial_fields_deserialize() {
        let json = r#"{
            "schema": "rttx.client.export",
            "version": 1,
            "app_version": "0.4.0",
            "exported_at": "2026-01-01T00:00:00Z",
            "data": {
                "preferences": null,
                "hosts": { "hosts": [] }
            }
        }"#;
        let loaded: ExportEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(loaded.data.preferences, None);
        assert!(loaded.data.hosts.is_some());
        assert_eq!(loaded.data.library, None);
        assert_eq!(loaded.data.workspaces, None);
    }

    #[test]
    fn export_envelope_schema_serializes_correctly() {
        let envelope = ExportEnvelope::new(ExportBundle::default());
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains(r#""schema":"rttx.client.export""#));
    }
}
