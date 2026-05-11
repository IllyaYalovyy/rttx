//! Integration tests for command CRUD workflows.
//!
//! Covers the full create → read → update → delete lifecycle through
//! `ClientStore`, including edge cases around UUID preservation on edit,
//! ordering stability, and concurrent place preservation.

use rttx::commands::{self, CommandParameter, CommandRunMode, SavedCommand};
use rttx::store::{ClientStore, StorePaths};
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

// ── Create ──────────────────────────────────────────────────────

#[test]
fn create_command_persists_all_fields() {
    let (_tmp, store) = test_store();

    let mut cmd = SavedCommand::new("Deploy", "cargo build --release");
    cmd.default_run_mode = CommandRunMode::RunInNewPane;
    cmd.host_tags = vec!["local".into(), "prod.example.com".into()];
    cmd.parameters = vec![CommandParameter {
        name: "ENV".into(),
        label: "Environment".into(),
        choices: vec!["prod".into(), "staging".into()],
        default: Some("staging".into()),
    }];
    cmd.description = "Build and deploy".into();
    cmd.labels = vec!["ops".into(), "deploy".into()];
    cmd.shortcut_keys = vec!["d".into(), "p".into()];

    store.save_commands(&[cmd.clone()]).unwrap();
    let loaded = store.load_commands();

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0], cmd);
}

#[test]
fn create_multiple_commands_preserves_order() {
    let (_tmp, store) = test_store();

    let c1 = SavedCommand::new("First", "echo 1");
    let c2 = SavedCommand::new("Second", "echo 2");
    let c3 = SavedCommand::new("Third", "echo 3");

    store.save_commands(&[c1, c2, c3]).unwrap();
    let loaded = store.load_commands();

    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].title, "First");
    assert_eq!(loaded[1].title, "Second");
    assert_eq!(loaded[2].title, "Third");
}

// ── Read (filtering) ────────────────────────────────────────────

#[test]
fn load_commands_from_empty_store_returns_empty() {
    let (_tmp, store) = test_store();
    assert!(store.load_commands().is_empty());
}

// ── Update (edit preserves UUID) ────────────────────────────────

#[test]
fn edit_command_preserves_uuid() {
    let (_tmp, store) = test_store();

    let original = SavedCommand::new("Deploy", "cargo build");
    let original_uuid = original.uuid.clone();
    store.save_commands(&[original]).unwrap();

    // Simulate edit: load, modify, save back with same UUID
    let mut items = store.load_commands();
    items[0].title = "Deploy v2".into();
    items[0].body = "cargo build --release\ncargo test".into();
    items[0].description = "Updated deploy".into();
    store.save_commands(&items).unwrap();

    let reloaded = store.load_commands();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].uuid, original_uuid);
    assert_eq!(reloaded[0].title, "Deploy v2");
    assert_eq!(reloaded[0].body, "cargo build --release\ncargo test");
    assert_eq!(reloaded[0].description, "Updated deploy");
}

#[test]
fn edit_command_does_not_affect_other_commands() {
    let (_tmp, store) = test_store();

    let c1 = SavedCommand::new("First", "echo 1");
    let c2 = SavedCommand::new("Second", "echo 2");
    let c1_uuid = c1.uuid.clone();
    let c2_uuid = c2.uuid.clone();
    store.save_commands(&[c1, c2]).unwrap();

    let mut items = store.load_commands();
    items[0].title = "First (edited)".into();
    store.save_commands(&items).unwrap();

    let reloaded = store.load_commands();
    assert_eq!(reloaded[0].uuid, c1_uuid);
    assert_eq!(reloaded[0].title, "First (edited)");
    assert_eq!(reloaded[1].uuid, c2_uuid);
    assert_eq!(reloaded[1].title, "Second");
}

// ── Delete ──────────────────────────────────────────────────────

#[test]
fn delete_command_removes_from_store() {
    let (_tmp, store) = test_store();

    let c1 = SavedCommand::new("Keep", "echo keep");
    let c2 = SavedCommand::new("Delete me", "echo bye");
    let delete_uuid = c2.uuid.clone();
    store.save_commands(&[c1, c2]).unwrap();

    // Simulate delete: retain all except the target UUID
    let mut items = store.load_commands();
    items.retain(|c| c.uuid != delete_uuid);
    store.save_commands(&items).unwrap();

    let reloaded = store.load_commands();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].title, "Keep");
}

#[test]
fn delete_last_command_leaves_empty_store() {
    let (_tmp, store) = test_store();

    let cmd = SavedCommand::new("Only one", "echo alone");
    let uuid = cmd.uuid.clone();
    store.save_commands(&[cmd]).unwrap();

    let mut items = store.load_commands();
    items.retain(|c| c.uuid != uuid);
    store.save_commands(&items).unwrap();

    assert!(store.load_commands().is_empty());
}

#[test]
fn delete_nonexistent_uuid_is_noop() {
    let (_tmp, store) = test_store();

    let cmd = SavedCommand::new("Survivor", "echo hi");
    store.save_commands(&[cmd]).unwrap();

    let mut items = store.load_commands();
    items.retain(|c| c.uuid != "nonexistent-uuid");
    store.save_commands(&items).unwrap();

    let reloaded = store.load_commands();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].title, "Survivor");
}

// ── Full CRUD roundtrip ─────────────────────────────────────────

#[test]
fn full_crud_lifecycle() {
    let (_tmp, store) = test_store();

    // Create
    let cmd = SavedCommand::new("Build", "cargo build");
    let uuid = cmd.uuid.clone();
    store.save_commands(&[cmd]).unwrap();
    assert_eq!(store.load_commands().len(), 1);

    // Read
    let loaded = store.load_commands();
    assert_eq!(loaded[0].title, "Build");
    assert_eq!(loaded[0].uuid, uuid);

    // Update
    let mut items = store.load_commands();
    items[0].title = "Build (updated)".into();
    items[0].default_run_mode = CommandRunMode::Insert;
    store.save_commands(&items).unwrap();

    let updated = store.load_commands();
    assert_eq!(updated[0].uuid, uuid);
    assert_eq!(updated[0].title, "Build (updated)");
    assert_eq!(updated[0].default_run_mode, CommandRunMode::Insert);

    // Delete
    let mut items = store.load_commands();
    items.retain(|c| c.uuid != uuid);
    store.save_commands(&items).unwrap();
    assert!(store.load_commands().is_empty());
}

// ── Commands do not clobber places ──────────────────────────────

#[test]
fn saving_commands_preserves_existing_places() {
    let (_tmp, store) = test_store();

    // Save a place first
    let place = rttx::places::Place::new("Work", "/home/user/work");
    store.save_places(std::slice::from_ref(&place)).unwrap();

    // Save commands
    let cmd = SavedCommand::new("Build", "cargo build");
    store.save_commands(&[cmd]).unwrap();

    // Places should still be there
    let places = store.load_places();
    assert_eq!(places.len(), 1);
    assert_eq!(places[0].name, "Work");
}

#[test]
fn deleting_all_commands_preserves_places() {
    let (_tmp, store) = test_store();

    let place = rttx::places::Place::new("Work", "/home/user/work");
    store.save_places(&[place]).unwrap();

    let cmd = SavedCommand::new("Build", "cargo build");
    store.save_commands(&[cmd]).unwrap();

    // Delete all commands
    store.save_commands(&[]).unwrap();

    assert!(store.load_commands().is_empty());
    assert_eq!(store.load_places().len(), 1);
}

// ── Reorder persistence ─────────────────────────────────────────

#[test]
fn reorder_persists_through_save_reload() {
    let (_tmp, store) = test_store();

    let mut items = vec![
        SavedCommand { uuid: "a".into(), ..SavedCommand::new("A", "echo a") },
        SavedCommand { uuid: "b".into(), ..SavedCommand::new("B", "echo b") },
        SavedCommand { uuid: "c".into(), ..SavedCommand::new("C", "echo c") },
    ];

    commands::reorder(&mut items, "c", "a");
    store.save_commands(&items).unwrap();

    let loaded = store.load_commands();
    let uuids: Vec<&str> = loaded.iter().map(|c| c.uuid.as_str()).collect();
    assert_eq!(uuids, vec!["c", "a", "b"]);
}

// ── Duplicate workflow ──────────────────────────────────────────

#[test]
fn duplicate_and_save_creates_independent_copy() {
    let (_tmp, store) = test_store();

    let mut original = SavedCommand::new("Deploy", "cargo build");
    original.labels = vec!["ops".into()];
    original.shortcut_keys = vec!["d".into()];
    store.save_commands(&[original.clone()]).unwrap();

    // Duplicate and append
    let copy = original.duplicate();
    let mut items = store.load_commands();
    items.push(copy);
    store.save_commands(&items).unwrap();

    let loaded = store.load_commands();
    assert_eq!(loaded.len(), 2);
    assert_ne!(loaded[0].uuid, loaded[1].uuid);
    assert_eq!(loaded[1].title, "Deploy (copy)");
    assert_eq!(loaded[1].labels, vec!["ops"]);
    assert_eq!(loaded[1].shortcut_keys, vec!["d"]);

    // Editing the copy does not affect the original
    let mut items = store.load_commands();
    items[1].title = "Deploy (staging)".into();
    items[1].labels = vec!["staging".into()];
    store.save_commands(&items).unwrap();

    let final_state = store.load_commands();
    assert_eq!(final_state[0].title, "Deploy");
    assert_eq!(final_state[0].labels, vec!["ops"]);
    assert_eq!(final_state[1].title, "Deploy (staging)");
    assert_eq!(final_state[1].labels, vec!["staging"]);
}
