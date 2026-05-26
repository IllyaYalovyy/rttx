//! Export bundle and envelope for single-file configuration backup (RFC-029 §2, §6).
//!
//! Import validation lives here alongside the export types so that
//! `parse_export_file` can validate against the canonical schema and version
//! constants without circular dependencies.

use serde::{Deserialize, Serialize};

use super::hosts::HostCatalog;
use super::library::Library;
use super::preferences::PreferencesV1;
use super::workspaces::WorkspaceStore;
use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Export;
pub const CURRENT_VERSION: u32 = 1;

/// Errors that can occur when parsing an export file for import.
///
/// Messages are user-facing and will be shown in dialogs.
#[derive(Debug)]
pub enum ImportError {
    /// The input is not valid JSON.
    InvalidJson(serde_json::Error),
    /// The JSON is valid but the `schema` field does not match `rttx.client.export`.
    WrongSchema,
    /// The file version is newer than this build supports.
    UnsupportedVersion { found: u32, max: u32 },
    /// An I/O error occurred while reading the file.
    IoError(std::io::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(e) => write!(f, "The file is not valid JSON: {e}"),
            Self::WrongSchema => {
                write!(f, "The file is not an rttx configuration export")
            }
            Self::UnsupportedVersion { found, max } => write!(
                f,
                "The file was created by a newer version of rttx (format version {found}, \
                 this build supports up to {max})"
            ),
            Self::IoError(e) => write!(f, "Could not read the file: {e}"),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJson(e) => Some(e),
            Self::IoError(e) => Some(e),
            Self::WrongSchema | Self::UnsupportedVersion { .. } => None,
        }
    }
}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

/// Parse and validate an export file's JSON content before importing.
///
/// Checks:
/// 1. Valid JSON
/// 2. Correct schema (`rttx.client.export`)
/// 3. Version <= `CURRENT_VERSION`
///
/// Missing sub-documents (`null` or absent fields) are accepted for partial import.
///
/// # Errors
///
/// Returns `ImportError` if validation fails.
pub fn parse_export_file(json: &str) -> Result<ExportBundle, ImportError> {
    let envelope: ExportEnvelope =
        serde_json::from_str(json).map_err(|e| match schema_probe(json) {
            SchemaProbe::NotJson(e) => ImportError::InvalidJson(e),
            SchemaProbe::WrongSchema => ImportError::WrongSchema,
            SchemaProbe::CorrectSchema => ImportError::InvalidJson(e),
        })?;

    if envelope.schema != SCHEMA {
        return Err(ImportError::WrongSchema);
    }

    if envelope.version > CURRENT_VERSION {
        return Err(ImportError::UnsupportedVersion {
            found: envelope.version,
            max: CURRENT_VERSION,
        });
    }

    Ok(envelope.data)
}

/// Probe the JSON to distinguish "not JSON at all" from "wrong schema" when
/// full deserialization fails.
enum SchemaProbe {
    NotJson(serde_json::Error),
    WrongSchema,
    CorrectSchema,
}

fn schema_probe(json: &str) -> SchemaProbe {
    #[derive(Deserialize)]
    struct Peek {
        schema: Option<serde_json::Value>,
    }

    match serde_json::from_str::<Peek>(json) {
        Err(e) => SchemaProbe::NotJson(e),
        Ok(peek) => {
            let is_export = peek
                .schema
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| s == "rttx.client.export");
            if is_export { SchemaProbe::CorrectSchema } else { SchemaProbe::WrongSchema }
        }
    }
}

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
                    daemon_binary_path: None,
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

    // ── parse_export_file tests ─────────────────────────────

    #[test]
    fn parse_export_file_accepts_valid_envelope() {
        let envelope = ExportEnvelope::new(ExportBundle {
            preferences: Some(PreferencesV1::default()),
            library: None,
            hosts: None,
            workspaces: None,
        });
        let json = serde_json::to_string_pretty(&envelope).unwrap();
        let bundle = parse_export_file(&json).unwrap();
        assert!(bundle.preferences.is_some());
        assert!(bundle.library.is_none());
    }

    #[test]
    fn parse_export_file_accepts_partial_data() {
        let json = r#"{
            "schema": "rttx.client.export",
            "version": 1,
            "app_version": "0.4.0",
            "exported_at": "2026-01-01T00:00:00Z",
            "data": {}
        }"#;
        let bundle = parse_export_file(json).unwrap();
        assert!(bundle.preferences.is_none());
        assert!(bundle.library.is_none());
        assert!(bundle.hosts.is_none());
        assert!(bundle.workspaces.is_none());
    }

    #[test]
    fn parse_export_file_accepts_null_sub_documents() {
        let json = r#"{
            "schema": "rttx.client.export",
            "version": 1,
            "app_version": "0.4.0",
            "exported_at": "2026-01-01T00:00:00Z",
            "data": {
                "preferences": null,
                "library": null,
                "hosts": null,
                "workspaces": null
            }
        }"#;
        let bundle = parse_export_file(json).unwrap();
        assert!(bundle.preferences.is_none());
        assert!(bundle.library.is_none());
        assert!(bundle.hosts.is_none());
        assert!(bundle.workspaces.is_none());
    }

    #[test]
    fn parse_export_file_rejects_invalid_json() {
        let err = parse_export_file("not json at all").unwrap_err();
        assert!(matches!(err, ImportError::InvalidJson(_)));
        assert!(err.to_string().contains("not valid JSON"));
    }

    #[test]
    fn parse_export_file_rejects_wrong_schema() {
        let json = r#"{
            "schema": "rttx.client.preferences",
            "version": 1,
            "app_version": "0.4.0",
            "exported_at": "2026-01-01T00:00:00Z",
            "data": {}
        }"#;
        let err = parse_export_file(json).unwrap_err();
        assert!(matches!(err, ImportError::WrongSchema));
        assert!(err.to_string().contains("not an rttx configuration export"));
    }

    #[test]
    fn parse_export_file_rejects_unknown_schema_string() {
        let json = r#"{
            "schema": "some.other.app",
            "version": 1,
            "app_version": "1.0.0",
            "exported_at": "2026-01-01T00:00:00Z",
            "data": {}
        }"#;
        let err = parse_export_file(json).unwrap_err();
        assert!(matches!(err, ImportError::WrongSchema));
    }

    #[test]
    fn parse_export_file_rejects_unsupported_version() {
        let json = r#"{
            "schema": "rttx.client.export",
            "version": 99,
            "app_version": "9.0.0",
            "exported_at": "2026-01-01T00:00:00Z",
            "data": {}
        }"#;
        let err = parse_export_file(json).unwrap_err();
        assert!(matches!(err, ImportError::UnsupportedVersion { found: 99, max: 1 }));
        assert!(err.to_string().contains("newer version"));
        assert!(err.to_string().contains("99"));
    }

    #[test]
    fn parse_export_file_rejects_missing_schema_field() {
        let json = r#"{
            "version": 1,
            "app_version": "0.4.0",
            "exported_at": "2026-01-01T00:00:00Z",
            "data": {}
        }"#;
        let err = parse_export_file(json).unwrap_err();
        assert!(matches!(err, ImportError::WrongSchema));
    }

    #[test]
    fn parse_export_file_rejects_empty_object() {
        let err = parse_export_file("{}").unwrap_err();
        assert!(matches!(err, ImportError::WrongSchema));
    }

    #[test]
    fn import_error_display_messages_are_user_facing() {
        let json_err = parse_export_file("{{bad").unwrap_err();
        let msg = json_err.to_string();
        assert!(msg.starts_with("The file is not valid JSON"));

        let schema_err = ImportError::WrongSchema;
        assert_eq!(schema_err.to_string(), "The file is not an rttx configuration export");

        let version_err = ImportError::UnsupportedVersion { found: 5, max: 1 };
        let msg = version_err.to_string();
        assert!(msg.contains("newer version of rttx"));
        assert!(msg.contains("format version 5"));
        assert!(msg.contains("up to 1"));

        let io_err = ImportError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(io_err.to_string().contains("Could not read the file"));
    }
}
