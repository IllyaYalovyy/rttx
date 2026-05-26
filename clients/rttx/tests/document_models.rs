//! Fixture-driven tests for canonical document models (RFC-023 Step 2).
//!
//! Covers:
//! - Round-trip serialization for every document model
//! - Loading from JSON fixture files
//! - Rejection of unsupported future versions
//! - `runtime-cache.json` deletion does not break startup
//! - Default fallback for missing fields

use rttx::store::models::hosts::{self, HostCatalog, HostKind, HostRecord};
use rttx::store::models::library::{self, CommandRecord, Library, PlaceRecord};
use rttx::store::models::preferences::{self, PreferencesV1};
use rttx::store::models::runtime_cache::{self, RuntimeCache};
use rttx::store::models::ui::{self, UiState};
use rttx::store::models::workspaces::{self, WorkspaceStore};
use rttx::store::{DocumentEnvelope, LoadOutcome, Schema, atomic_load, atomic_save};
use tempfile::TempDir;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/store");

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(FIXTURES).join(name)
}

fn load_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"))
}

// ── Preferences ──────────────────────────────────────────────────────

#[test]
fn preferences_fixture_round_trips() {
    let json = load_fixture("preferences_v1.json");
    let envelope: DocumentEnvelope<PreferencesV1> = serde_json::from_str(&json).unwrap();

    assert_eq!(envelope.schema, Schema::Preferences);
    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.data.font, "JetBrains Mono 14");
    assert_eq!(envelope.data.scrollback_lines, 20_000);
    assert!(envelope.data.smart_clipboard);
    assert_eq!(envelope.data.reconnect_delay_secs, 5);
    assert_eq!(envelope.data.paste_guard_threshold, 300);

    let reserialized = serde_json::to_string_pretty(&envelope).unwrap();
    let reloaded: DocumentEnvelope<PreferencesV1> = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(envelope.data, reloaded.data);
}

#[test]
fn preferences_default_fills_all_fields() {
    let prefs = PreferencesV1::default();
    assert_eq!(prefs.font, "Monospace 12");
    assert_eq!(prefs.scrollback_lines, 10_000);
    assert!(prefs.show_headerbar);
    assert!(prefs.auto_start_daemon);
    assert_eq!(prefs.reconnect_delay_secs, 3);
    assert_eq!(prefs.paste_guard_threshold, 200);
}

#[test]
fn preferences_partial_json_fills_defaults() {
    let json = r#"{
        "schema": "rttx.client.preferences",
        "version": 1,
        "app_version": "0.4.4",
        "written_at": "2026-01-01T00:00:00Z",
        "data": { "font": "Hack 11" }
    }"#;
    let envelope: DocumentEnvelope<PreferencesV1> = serde_json::from_str(json).unwrap();
    assert_eq!(envelope.data.font, "Hack 11");
    assert_eq!(envelope.data.scrollback_lines, 10_000);
    assert!(envelope.data.auto_start_daemon);
}

#[test]
fn preferences_rejects_future_version() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("preferences.json");
    let backups = tmp.path().join("backups");

    let env = DocumentEnvelope {
        schema: Schema::Preferences,
        version: 99,
        app_version: "9.0.0".into(),
        written_at: "2030-01-01T00:00:00Z".into(),
        data: PreferencesV1::default(),
    };
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<PreferencesV1> =
        atomic_load(&path, preferences::SCHEMA, preferences::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::UnsupportedVersion { found: 99, max_supported: 1 }));
    assert!(path.exists(), "file must not be deleted");
}

#[test]
fn preferences_atomic_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("preferences.json");
    let backups = tmp.path().join("backups");

    let prefs = PreferencesV1 { font: "Fira Code 13".into(), ..PreferencesV1::default() };
    let env =
        DocumentEnvelope::new(preferences::SCHEMA, preferences::CURRENT_VERSION, prefs.clone());
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<PreferencesV1> =
        atomic_load(&path, preferences::SCHEMA, preferences::CURRENT_VERSION, &backups);
    assert_eq!(outcome.into_value().unwrap(), prefs);
}

// ── Hosts ────────────────────────────────────────────────────────────

#[test]
fn hosts_fixture_round_trips() {
    let json = load_fixture("hosts_v1.json");
    let envelope: DocumentEnvelope<HostCatalog> = serde_json::from_str(&json).unwrap();

    assert_eq!(envelope.schema, Schema::Hosts);
    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.data.hosts.len(), 2);
    assert_eq!(envelope.data.hosts[0].key, "devbox.example.com");
    assert_eq!(envelope.data.hosts[0].kind, HostKind::Remote);
    assert_eq!(envelope.data.hosts[0].labels, vec!["work", "dev"]);
    assert_eq!(envelope.data.hosts[1].labels, Vec::<String>::new());

    let reserialized = serde_json::to_string_pretty(&envelope).unwrap();
    let reloaded: DocumentEnvelope<HostCatalog> = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(envelope.data, reloaded.data);
}

#[test]
fn hosts_empty_catalog_round_trips() {
    let catalog = HostCatalog::default();
    let env = DocumentEnvelope::new(hosts::SCHEMA, hosts::CURRENT_VERSION, catalog.clone());
    let json = serde_json::to_string(&env).unwrap();
    let reloaded: DocumentEnvelope<HostCatalog> = serde_json::from_str(&json).unwrap();
    assert_eq!(reloaded.data, catalog);
}

#[test]
fn hosts_record_without_optional_fields_deserializes() {
    let json = r#"{
        "schema": "rttx.client.hosts",
        "version": 1,
        "app_version": "0.4.4",
        "written_at": "2026-01-01T00:00:00Z",
        "data": {
            "hosts": [{
                "key": "box",
                "name": "Box",
                "kind": "remote"
            }]
        }
    }"#;
    let envelope: DocumentEnvelope<HostCatalog> = serde_json::from_str(json).unwrap();
    assert!(envelope.data.hosts[0].ssh_target.is_none());
    assert!(envelope.data.hosts[0].labels.is_empty());
}

#[test]
fn hosts_rejects_future_version() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("hosts.json");
    let backups = tmp.path().join("backups");

    let env = DocumentEnvelope {
        schema: Schema::Hosts,
        version: 42,
        app_version: "9.0.0".into(),
        written_at: "2030-01-01T00:00:00Z".into(),
        data: HostCatalog::default(),
    };
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<HostCatalog> =
        atomic_load(&path, hosts::SCHEMA, hosts::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::UnsupportedVersion { found: 42, max_supported: 1 }));
}

#[test]
fn hosts_atomic_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("hosts.json");
    let backups = tmp.path().join("backups");

    let catalog = HostCatalog {
        hosts: vec![HostRecord {
            key: "test.host".into(),
            name: "Test".into(),
            kind: HostKind::Remote,
            ssh_target: Some("user@test.host".into()),
            daemon_binary_path: None,
            labels: vec!["test".into()],
        }],
    };
    let env = DocumentEnvelope::new(hosts::SCHEMA, hosts::CURRENT_VERSION, catalog.clone());
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<HostCatalog> =
        atomic_load(&path, hosts::SCHEMA, hosts::CURRENT_VERSION, &backups);
    assert_eq!(outcome.into_value().unwrap(), catalog);
}

// ── Library ──────────────────────────────────────────────────────────

#[test]
fn library_fixture_round_trips() {
    let json = load_fixture("library_v1.json");
    let envelope: DocumentEnvelope<Library> = serde_json::from_str(&json).unwrap();

    assert_eq!(envelope.schema, Schema::Library);
    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.data.places.len(), 2);
    assert_eq!(envelope.data.commands.len(), 3);

    assert_eq!(envelope.data.places[0].name, "Projects");
    assert!(envelope.data.places[0].host_tags.is_empty());
    assert_eq!(envelope.data.places[1].host_tags, vec!["devbox.example.com"]);

    assert_eq!(envelope.data.commands[0].title, "Git Status");
    assert_eq!(
        envelope.data.commands[2].default_run_mode,
        rttx::store::models::commands::RunMode::Insert
    );

    let reserialized = serde_json::to_string_pretty(&envelope).unwrap();
    let reloaded: DocumentEnvelope<Library> = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(envelope.data, reloaded.data);
}

#[test]
fn library_empty_round_trips() {
    let lib = Library::default();
    let env = DocumentEnvelope::new(library::SCHEMA, library::CURRENT_VERSION, lib.clone());
    let json = serde_json::to_string(&env).unwrap();
    let reloaded: DocumentEnvelope<Library> = serde_json::from_str(&json).unwrap();
    assert_eq!(reloaded.data, lib);
}

#[test]
fn library_command_without_run_mode_defaults_to_run() {
    let json = r#"{
        "schema": "rttx.client.library",
        "version": 1,
        "app_version": "0.4.4",
        "written_at": "2026-01-01T00:00:00Z",
        "data": {
            "places": [],
            "commands": [{
                "id": "c1",
                "title": "ls",
                "body": "ls -la"
            }]
        }
    }"#;
    let envelope: DocumentEnvelope<Library> = serde_json::from_str(json).unwrap();
    assert_eq!(
        envelope.data.commands[0].default_run_mode,
        rttx::store::models::commands::RunMode::Run
    );
}

#[test]
fn library_rejects_future_version() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("library.json");
    let backups = tmp.path().join("backups");

    let env = DocumentEnvelope {
        schema: Schema::Library,
        version: 50,
        app_version: "9.0.0".into(),
        written_at: "2030-01-01T00:00:00Z".into(),
        data: Library::default(),
    };
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<Library> =
        atomic_load(&path, library::SCHEMA, library::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::UnsupportedVersion { found: 50, max_supported: 1 }));
}

#[test]
fn library_preserves_orphaned_host_tags() {
    let lib = Library {
        places: vec![PlaceRecord {
            id: "p1".into(),
            name: "Orphaned".into(),
            path: "/tmp".into(),
            host_tags: vec!["deleted-host-key".into()],
        }],
        commands: vec![CommandRecord {
            id: "c1".into(),
            title: "Orphaned cmd".into(),
            body: "echo hi".into(),
            default_run_mode: rttx::store::models::commands::RunMode::Run,
            host_tags: vec!["deleted-host-key".into()],
            parameters: vec![],
            description: String::new(),
            labels: vec![],
            shortcut_keys: vec![],
        }],
    };
    let env = DocumentEnvelope::new(library::SCHEMA, library::CURRENT_VERSION, lib);
    let json = serde_json::to_string(&env).unwrap();
    let reloaded: DocumentEnvelope<Library> = serde_json::from_str(&json).unwrap();
    assert_eq!(reloaded.data.places[0].host_tags, vec!["deleted-host-key"]);
    assert_eq!(reloaded.data.commands[0].host_tags, vec!["deleted-host-key"]);
}

// ── Workspaces ───────────────────────────────────────────────────────

#[test]
fn workspaces_fixture_round_trips() {
    let json = load_fixture("workspaces_v1.json");
    let envelope: DocumentEnvelope<WorkspaceStore> = serde_json::from_str(&json).unwrap();

    assert_eq!(envelope.schema, Schema::Workspaces);
    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.data.active_workspace_id.as_deref(), Some("ws-001"));
    assert_eq!(envelope.data.workspaces.len(), 2);

    let ws1 = &envelope.data.workspaces[0];
    assert_eq!(ws1.name, "Development");
    assert!(ws1.user_renamed);
    assert_eq!(ws1.endpoint_key, "local");
    assert!(ws1.runtime_ref.is_some());
    assert_eq!(ws1.runtime_ref.as_ref().unwrap().runtime_id, "rt-abc-123");
    assert_eq!(ws1.pane_recovery.len(), 2);

    let ws2 = &envelope.data.workspaces[1];
    assert_eq!(ws2.endpoint_key, "devbox.example.com");
    assert!(ws2.runtime_ref.is_none());

    let reserialized = serde_json::to_string_pretty(&envelope).unwrap();
    let reloaded: DocumentEnvelope<WorkspaceStore> = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(envelope.data, reloaded.data);
}

#[test]
fn workspaces_empty_store_round_trips() {
    let store = WorkspaceStore::default();
    let env = DocumentEnvelope::new(workspaces::SCHEMA, workspaces::CURRENT_VERSION, store.clone());
    let json = serde_json::to_string(&env).unwrap();
    let reloaded: DocumentEnvelope<WorkspaceStore> = serde_json::from_str(&json).unwrap();
    assert_eq!(reloaded.data, store);
}

#[test]
fn workspaces_missing_optional_fields_deserialize() {
    let json = r#"{
        "schema": "rttx.client.workspaces",
        "version": 1,
        "app_version": "0.4.4",
        "written_at": "2026-01-01T00:00:00Z",
        "data": {
            "workspaces": [{
                "id": "ws-min",
                "name": "Minimal",
                "layout": { "terminal": { "uuid": "p1" } }
            }]
        }
    }"#;
    let envelope: DocumentEnvelope<WorkspaceStore> = serde_json::from_str(json).unwrap();
    let ws = &envelope.data.workspaces[0];
    assert!(!ws.user_renamed);
    assert_eq!(ws.endpoint_key, "local");
    assert!(ws.runtime_ref.is_none());
    assert!(ws.active_pane_id.is_none());
    assert!(ws.zoomed_pane_id.is_none());
    assert!(ws.pane_recovery.is_empty());
}

#[test]
fn workspaces_rejects_future_version() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("workspaces.json");
    let backups = tmp.path().join("backups");

    let env = DocumentEnvelope {
        schema: Schema::Workspaces,
        version: 77,
        app_version: "9.0.0".into(),
        written_at: "2030-01-01T00:00:00Z".into(),
        data: WorkspaceStore::default(),
    };
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<WorkspaceStore> =
        atomic_load(&path, workspaces::SCHEMA, workspaces::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::UnsupportedVersion { found: 77, max_supported: 1 }));
}

// ── UI State ─────────────────────────────────────────────────────────

#[test]
fn ui_fixture_round_trips() {
    let json = load_fixture("ui_v1.json");
    let envelope: DocumentEnvelope<UiState> = serde_json::from_str(&json).unwrap();

    assert_eq!(envelope.schema, Schema::Ui);
    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.data.window_width, 1280);
    assert_eq!(envelope.data.window_height, 720);
    assert!(!envelope.data.is_maximized);
    assert_eq!(envelope.data.left_sidebar_width, 250);
    assert!(envelope.data.left_sidebar_visible);
    assert!(!envelope.data.right_sidebar_visible);
    assert_eq!(envelope.data.selected_right_tool.as_deref(), Some("commands"));

    let reserialized = serde_json::to_string_pretty(&envelope).unwrap();
    let reloaded: DocumentEnvelope<UiState> = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(envelope.data, reloaded.data);
}

#[test]
fn ui_default_has_sensible_dimensions() {
    let ui = UiState::default();
    assert_eq!(ui.window_width, 900);
    assert_eq!(ui.window_height, 600);
    assert_eq!(ui.left_sidebar_width, 220);
    assert_eq!(ui.right_sidebar_width, 320);
    assert!(!ui.is_maximized);
}

#[test]
fn ui_partial_json_fills_defaults() {
    let json = r#"{
        "schema": "rttx.client.ui",
        "version": 1,
        "app_version": "0.4.4",
        "written_at": "2026-01-01T00:00:00Z",
        "data": { "window_width": 1920 }
    }"#;
    let envelope: DocumentEnvelope<UiState> = serde_json::from_str(json).unwrap();
    assert_eq!(envelope.data.window_width, 1920);
    assert_eq!(envelope.data.window_height, 600);
    assert_eq!(envelope.data.left_sidebar_width, 220);
}

#[test]
fn ui_rejects_future_version() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("ui.json");
    let backups = tmp.path().join("backups");

    let env = DocumentEnvelope {
        schema: Schema::Ui,
        version: 10,
        app_version: "9.0.0".into(),
        written_at: "2030-01-01T00:00:00Z".into(),
        data: UiState::default(),
    };
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<UiState> =
        atomic_load(&path, ui::SCHEMA, ui::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::UnsupportedVersion { found: 10, max_supported: 1 }));
}

#[test]
fn ui_atomic_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("ui.json");
    let backups = tmp.path().join("backups");

    let state = UiState { window_width: 1600, window_height: 900, ..UiState::default() };
    let env = DocumentEnvelope::new(ui::SCHEMA, ui::CURRENT_VERSION, state.clone());
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<UiState> =
        atomic_load(&path, ui::SCHEMA, ui::CURRENT_VERSION, &backups);
    assert_eq!(outcome.into_value().unwrap(), state);
}

// ── Runtime Cache ────────────────────────────────────────────────────

#[test]
fn runtime_cache_fixture_round_trips() {
    let json = load_fixture("runtime_cache_v1.json");
    let envelope: DocumentEnvelope<RuntimeCache> = serde_json::from_str(&json).unwrap();

    assert_eq!(envelope.schema, Schema::RuntimeCache);
    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.data.dismissed_runtime_ids.len(), 2);
    assert!(envelope.data.dismissed_runtime_ids.contains("rt-old-001"));

    let reserialized = serde_json::to_string_pretty(&envelope).unwrap();
    let reloaded: DocumentEnvelope<RuntimeCache> = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(envelope.data, reloaded.data);
}

#[test]
fn runtime_cache_rejects_future_version() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime-cache.json");
    let backups = tmp.path().join("backups");

    let env = DocumentEnvelope {
        schema: Schema::RuntimeCache,
        version: 5,
        app_version: "9.0.0".into(),
        written_at: "2030-01-01T00:00:00Z".into(),
        data: RuntimeCache::default(),
    };
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<RuntimeCache> =
        atomic_load(&path, runtime_cache::SCHEMA, runtime_cache::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::UnsupportedVersion { found: 5, max_supported: 1 }));
}

#[test]
fn runtime_cache_deletion_returns_empty_default() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime-cache.json");
    let backups = tmp.path().join("backups");

    // File does not exist — simulates deletion
    assert!(!path.exists());

    let outcome: LoadOutcome<RuntimeCache> =
        atomic_load(&path, runtime_cache::SCHEMA, runtime_cache::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::Default(_)));
    let cache = outcome.into_value().unwrap();
    assert!(cache.dismissed_runtime_ids.is_empty());
}

#[test]
fn runtime_cache_deletion_after_save_returns_default() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime-cache.json");
    let backups = tmp.path().join("backups");

    // Save, then delete
    let cache = RuntimeCache { dismissed_runtime_ids: std::iter::once("rt-1".into()).collect() };
    let env = DocumentEnvelope::new(runtime_cache::SCHEMA, runtime_cache::CURRENT_VERSION, cache);
    atomic_save(&path, &env).unwrap();
    assert!(path.exists());

    std::fs::remove_file(&path).unwrap();
    // Also remove .bak to fully simulate cache cleanup
    let _ = std::fs::remove_file(path.with_extension("bak"));

    let outcome: LoadOutcome<RuntimeCache> =
        atomic_load(&path, runtime_cache::SCHEMA, runtime_cache::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::Default(_)));
    assert!(outcome.into_value().unwrap().dismissed_runtime_ids.is_empty());
}

#[test]
fn runtime_cache_atomic_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime-cache.json");
    let backups = tmp.path().join("backups");

    let cache = RuntimeCache {
        dismissed_runtime_ids: ["rt-a".into(), "rt-b".into()].into_iter().collect(),
    };
    let env =
        DocumentEnvelope::new(runtime_cache::SCHEMA, runtime_cache::CURRENT_VERSION, cache.clone());
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<RuntimeCache> =
        atomic_load(&path, runtime_cache::SCHEMA, runtime_cache::CURRENT_VERSION, &backups);
    assert_eq!(outcome.into_value().unwrap(), cache);
}

// ── Cross-domain: schema mismatch ────────────────────────────────────

#[test]
fn loading_hosts_file_as_preferences_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("wrong.json");
    let backups = tmp.path().join("backups");

    let env = DocumentEnvelope::new(hosts::SCHEMA, hosts::CURRENT_VERSION, HostCatalog::default());
    atomic_save(&path, &env).unwrap();

    let outcome: LoadOutcome<PreferencesV1> =
        atomic_load(&path, preferences::SCHEMA, preferences::CURRENT_VERSION, &backups);
    assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
}
