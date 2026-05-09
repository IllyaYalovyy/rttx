//! Integration tests for the export configuration action (issue #856).
//!
//! These tests verify the export-to-file flow: serialization, file writing,
//! and round-trip validation through `parse_export_file`.

use rttx::store::models::export::{ExportBundle, ExportEnvelope, parse_export_file};
use rttx::store::models::preferences::PreferencesV1;

#[test]
fn export_envelope_serializes_to_valid_json_file() {
    let bundle = ExportBundle {
        preferences: Some(PreferencesV1::default()),
        library: None,
        hosts: None,
        workspaces: None,
    };
    let envelope = ExportEnvelope::new(bundle);
    let json = serde_json::to_string_pretty(&envelope).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rttx-config-2026-05-08.json");
    std::fs::write(&path, &json).unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed = parse_export_file(&contents).unwrap();
    assert!(parsed.preferences.is_some());
}

#[test]
fn export_file_contains_schema_and_version_fields() {
    let envelope = ExportEnvelope::new(ExportBundle::default());
    let json = serde_json::to_string_pretty(&envelope).unwrap();

    assert!(json.contains("\"schema\": \"rttx.client.export\""));
    assert!(json.contains("\"version\": 1"));
    assert!(json.contains("\"app_version\""));
    assert!(json.contains("\"exported_at\""));
}

#[test]
fn export_default_filename_contains_date() {
    let filename = format!("rttx-config-{}.json", "2026-05-08");
    assert!(filename.starts_with("rttx-config-"));
    assert!(std::path::Path::new(&filename)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json")));
    assert_eq!(filename.len(), "rttx-config-YYYY-MM-DD.json".len());
}

#[test]
fn export_write_failure_on_readonly_path() {
    let bundle = ExportBundle::default();
    let envelope = ExportEnvelope::new(bundle);
    let json = serde_json::to_string_pretty(&envelope).unwrap();

    let result = std::fs::write("/proc/nonexistent/export.json", &json);
    assert!(result.is_err());
}

#[test]
fn export_full_bundle_round_trips_through_file() {
    use rttx::store::models::hosts::{HostCatalog, HostKind, HostRecord};
    use rttx::store::models::library::{CommandRecord, Library, PlaceRecord};
    use rttx::store::models::commands::RunMode;

    let bundle = ExportBundle {
        preferences: Some(PreferencesV1::default()),
        library: Some(Library {
            places: vec![PlaceRecord {
                id: "p1".into(),
                name: "Projects".into(),
                path: "~/projects".into(),
                host_tags: vec![],
            }],
            commands: vec![CommandRecord {
                id: "c1".into(),
                title: "Test".into(),
                body: "cargo test".into(),
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
                key: "dev-server".into(),
                name: "Dev Server".into(),
                kind: HostKind::default(),
                ssh_target: Some("user@dev.example.com".into()),
                labels: vec![],
            }],
        }),
        workspaces: None,
    };

    let envelope = ExportEnvelope::new(bundle.clone());
    let json = serde_json::to_string_pretty(&envelope).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("full-export.json");
    std::fs::write(&path, &json).unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed = parse_export_file(&contents).unwrap();

    assert_eq!(parsed.preferences, bundle.preferences);
    assert_eq!(parsed.library, bundle.library);
    assert_eq!(parsed.hosts, bundle.hosts);
    assert_eq!(parsed.workspaces, bundle.workspaces);
}
