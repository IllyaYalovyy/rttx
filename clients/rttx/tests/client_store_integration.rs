//! Integration tests for `ClientStore` API (RFC-023 Step 4).
//!
//! Verifies end-to-end round-trip through the store for preferences, hosts,
//! and the merged library document, including malformed-file recovery.

use rttx::commands::{CommandRunMode, SavedCommand};
use rttx::host::Host;
use rttx::places::Place;
use rttx::preferences::Preferences;
use rttx::store::{ClientStore, LoadOutcome, StorePaths};
use tempfile::TempDir;

fn test_store() -> (TempDir, ClientStore) {
    let tmp = TempDir::new().unwrap();
    let paths = StorePaths::new(
        tmp.path().join("config"),
        tmp.path().join("state"),
        tmp.path().join("cache"),
    );
    (tmp, ClientStore::new(paths))
}

// ── Preferences ──────────────────────────────────────────────────────

#[test]
fn client_store_preferences_full_round_trip() {
    let (_tmp, store) = test_store();
    let prefs = Preferences {
        font: "Fira Code 13".into(),
        scrollback_lines: 20_000,
        smart_clipboard: true,
        paste_guard_threshold: 512,
        ..Default::default()
    };
    store.save_preferences(&prefs).unwrap();
    let loaded = store.load_preferences().into_value().unwrap();
    assert_eq!(loaded.font, "Fira Code 13");
    assert_eq!(loaded.scrollback_lines, 20_000);
    assert!(loaded.smart_clipboard);
    assert_eq!(loaded.paste_guard_threshold, 512);
}

#[test]
fn client_store_preferences_malformed_recovery() {
    let (_tmp, store) = test_store();
    // Save a good version, then save again to create .bak
    let good = Preferences { font: "Good Font 12".into(), ..Default::default() };
    store.save_preferences(&good).unwrap();
    let updated = Preferences { font: "Updated Font 12".into(), ..Default::default() };
    store.save_preferences(&updated).unwrap();
    // Corrupt the primary
    let path = store.paths().config().join("preferences.json");
    std::fs::write(&path, "corrupted data").unwrap();
    let outcome = store.load_preferences();
    assert!(matches!(outcome, LoadOutcome::Recovered(_)));
    assert_eq!(outcome.into_value().unwrap().font, "Good Font 12");
}

// ── Hosts ────────────────────────────────────────────────────────────

#[test]
fn client_store_hosts_full_round_trip() {
    let (_tmp, store) = test_store();
    let hosts = vec![Host::remote("deploy@example.com"), Host::remote("admin@staging.example.com")];
    store.save_hosts(&hosts).unwrap();
    let loaded = store.load_hosts().into_value().unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].key, "example.com");
    assert_eq!(loaded[1].key, "staging.example.com");
}

// ── Library (places + commands merged) ───────────────────────────────

#[test]
fn client_store_library_merges_places_and_commands() {
    let (_tmp, store) = test_store();
    let mut place = Place::new("rttx", "~/pro/rttx");
    place.host_tags = vec!["local".into()];
    let mut cmd = SavedCommand::new("Build", "cargo build");
    cmd.host_tags = vec!["local".into(), "example.com".into()];

    store.save_library(&[place], &[cmd]).unwrap();

    // Verify both are in the same file
    let lib_path = store.paths().config().join("library.json");
    let content = std::fs::read_to_string(&lib_path).unwrap();
    assert!(content.contains("rttx.client.library"));
    assert!(content.contains("rttx"));
    assert!(content.contains("Build"));

    let (places, commands) = store.load_library().into_value().unwrap();
    assert_eq!(places.len(), 1);
    assert_eq!(commands.len(), 1);
    assert_eq!(places[0].name, "rttx");
    assert_eq!(commands[0].title, "Build");
}

#[test]
fn client_store_save_places_preserves_commands() {
    let (_tmp, store) = test_store();
    let cmd = SavedCommand::new("Deploy", "cargo deploy");
    store.save_commands(&[cmd]).unwrap();

    let place = Place::new("Home", "~");
    store.save_places(&[place]).unwrap();

    let (places, commands) = store.load_library().into_value().unwrap();
    assert_eq!(places.len(), 1);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].title, "Deploy");
}

#[test]
fn client_store_save_commands_preserves_places() {
    let (_tmp, store) = test_store();
    let place = Place::new("Projects", "~/projects");
    store.save_places(&[place]).unwrap();

    let cmd = SavedCommand::new("Test", "cargo test");
    store.save_commands(&[cmd]).unwrap();

    let (places, commands) = store.load_library().into_value().unwrap();
    assert_eq!(places.len(), 1);
    assert_eq!(commands.len(), 1);
    assert_eq!(places[0].name, "Projects");
}

#[test]
fn client_store_library_preserves_command_run_mode() {
    let (_tmp, store) = test_store();
    let mut cmd = SavedCommand::new("Insert", "echo hi");
    cmd.default_run_mode = CommandRunMode::Insert;
    store.save_commands(&[cmd]).unwrap();

    let commands = store.load_commands();
    assert_eq!(commands[0].default_run_mode, CommandRunMode::Insert);
}

#[test]
fn client_store_library_malformed_recovery() {
    let (_tmp, store) = test_store();
    let path = store.paths().config().join("library.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not json").unwrap();
    let outcome = store.load_library();
    assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    let (places, commands) = outcome.into_value().unwrap();
    assert!(places.is_empty());
    assert!(commands.is_empty());
}
