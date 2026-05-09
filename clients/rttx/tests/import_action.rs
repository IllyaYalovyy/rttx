//! Integration tests for the import configuration action (issue #857).
//!
//! These tests verify the import flow: file reading, validation, store write,
//! and error handling. The GTK dialog interactions (confirmation, file picker)
//! cannot be tested without a display, but the underlying logic is exercised.

use rttx::store::models::export::{ExportBundle, ExportEnvelope, parse_export_file};
use rttx::store::models::preferences::PreferencesV1;
use rttx::store::{ClientStore, StorePaths};

fn test_store() -> (tempfile::TempDir, ClientStore) {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    let state_dir = dir.path().join("state");
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    let paths = StorePaths::new(config_dir, state_dir, cache_dir);
    let store = ClientStore::new(paths);
    (dir, store)
}

#[test]
fn import_flow_reads_file_validates_and_writes_to_store() {
    let (dir, store) = test_store();

    let bundle = ExportBundle {
        preferences: Some(PreferencesV1 { font: "Monospace 14".into(), ..Default::default() }),
        library: None,
        hosts: None,
        workspaces: None,
    };
    let envelope = ExportEnvelope::new(bundle);
    let json = serde_json::to_string_pretty(&envelope).unwrap();

    let file_path = dir.path().join("import.json");
    std::fs::write(&file_path, &json).unwrap();

    let contents = std::fs::read_to_string(&file_path).unwrap();
    let parsed = parse_export_file(&contents).unwrap();
    store.import_bundle(&parsed).unwrap();

    let loaded = store.load_preferences().into_value().unwrap_or_default();
    assert_eq!(loaded.font, "Monospace 14");
}

#[test]
fn import_flow_rejects_invalid_file_content() {
    let result = parse_export_file("this is not json");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("not valid JSON"));
}

#[test]
fn import_flow_rejects_wrong_schema_file() {
    let json = r#"{"schema": "something.else", "version": 1, "data": {}}"#;
    let result = parse_export_file(json);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("not an rttx configuration export"));
}

#[test]
fn import_flow_rejects_unsupported_version() {
    let json = r#"{
        "schema": "rttx.client.export",
        "version": 99,
        "app_version": "9.0.0",
        "exported_at": "2026-01-01T00:00:00Z",
        "data": {}
    }"#;
    let result = parse_export_file(json);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("newer version"));
}

#[test]
fn import_flow_handles_io_error_on_missing_file() {
    let result = std::fs::read_to_string("/nonexistent/path/config.json");
    assert!(result.is_err());
}

#[test]
fn import_flow_overwrites_existing_preferences() {
    let (_dir, store) = test_store();

    let initial_bundle = ExportBundle {
        preferences: Some(PreferencesV1 { font: "Initial Font 12".into(), ..Default::default() }),
        library: None,
        hosts: None,
        workspaces: None,
    };
    store.import_bundle(&initial_bundle).unwrap();

    let loaded = store.load_preferences().into_value().unwrap_or_default();
    assert_eq!(loaded.font, "Initial Font 12");

    let bundle = ExportBundle {
        preferences: Some(PreferencesV1 { font: "Imported Font 16".into(), ..Default::default() }),
        library: None,
        hosts: None,
        workspaces: None,
    };
    let envelope = ExportEnvelope::new(bundle);
    let json = serde_json::to_string_pretty(&envelope).unwrap();

    let parsed = parse_export_file(&json).unwrap();
    store.import_bundle(&parsed).unwrap();

    let loaded = store.load_preferences().into_value().unwrap_or_default();
    assert_eq!(loaded.font, "Imported Font 16");
}
