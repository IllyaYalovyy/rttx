//! Integration tests for import file validation (issue #854).

use rttx::store::models::export::{ExportBundle, ExportEnvelope, ImportError, parse_export_file};

#[test]
fn parse_export_file_round_trips_through_serialized_envelope() {
    let envelope = ExportEnvelope::new(ExportBundle::default());
    let json = serde_json::to_string_pretty(&envelope).unwrap();
    let bundle = parse_export_file(&json).unwrap();
    assert!(bundle.preferences.is_none());
    assert!(bundle.library.is_none());
    assert!(bundle.hosts.is_none());
    assert!(bundle.workspaces.is_none());
}

#[test]
fn parse_export_file_rejects_non_export_schema() {
    let json = r#"{
        "schema": "rttx.client.preferences",
        "version": 1,
        "app_version": "0.4.0",
        "written_at": "2026-01-01T00:00:00Z",
        "data": {}
    }"#;
    let err = parse_export_file(json).unwrap_err();
    assert!(matches!(err, ImportError::WrongSchema));
}

#[test]
fn parse_export_file_rejects_future_version() {
    let json = r#"{
        "schema": "rttx.client.export",
        "version": 999,
        "app_version": "99.0.0",
        "exported_at": "2099-01-01T00:00:00Z",
        "data": {}
    }"#;
    let err = parse_export_file(json).unwrap_err();
    assert!(matches!(err, ImportError::UnsupportedVersion { found: 999, max: 1 }));
}
