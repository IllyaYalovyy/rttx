use rttx::commands::{self, CommandRunMode, SavedCommand};
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

#[test]
fn commands_roundtrip_multiline_and_mode() {
    let (_tmp, store) = test_store();

    let mut command = SavedCommand::new("Deploy", "cargo build\ncargo test\nsystemctl restart app");
    command.default_run_mode = CommandRunMode::Insert;

    store.save_commands(&[command.clone()]).unwrap();
    let loaded = store.load_commands();
    assert_eq!(loaded, vec![command]);
}

#[test]
fn commands_with_host_tags_roundtrip() {
    let (_tmp, store) = test_store();

    let mut command = SavedCommand::new("Remote deploy", "cargo build");
    command.host_tags = vec!["local".into(), "example.com".into()];

    store.save_commands(&[command.clone()]).unwrap();
    let loaded = store.load_commands();
    assert_eq!(loaded, vec![command]);
}

#[test]
fn legacy_commands_without_host_tags_deserialize_and_migrate() {
    let json = r#"[{
        "uuid": "abc-123",
        "title": "Old command",
        "body": "echo hello",
        "default_run_mode": "run"
    }]"#;

    let mut loaded: Vec<SavedCommand> = serde_json::from_str(json).unwrap();
    assert!(loaded[0].host_tags.is_empty(), "legacy commands load with empty tags");

    commands::migrate_legacy(&mut loaded);
    assert_eq!(loaded[0].host_tags, vec!["local"], "migration tags with local");

    let json2 = serde_json::to_string(&loaded).unwrap();
    let reloaded: Vec<SavedCommand> = serde_json::from_str(&json2).unwrap();
    assert_eq!(reloaded[0].host_tags, vec!["local"], "migrated tags persist");
}

#[test]
fn commands_with_parameters_roundtrip() {
    let (_tmp, store) = test_store();

    let mut command = SavedCommand::new("Restart service", "systemctl restart $SERVICE");
    command.parameters = vec![rttx::commands::CommandParameter {
        name: "SERVICE".into(),
        label: "Service name".into(),
        choices: vec!["api".into(), "web".into(), "worker".into()],
        default: Some("api".into()),
    }];
    command.description = "Restart a systemd service".into();
    command.labels = vec!["ops".into()];

    store.save_commands(&[command.clone()]).unwrap();
    let loaded = store.load_commands();
    assert_eq!(loaded, vec![command]);
}

#[test]
fn duplicate_command_persists_with_new_uuid() {
    let (_tmp, store) = test_store();

    let mut command = SavedCommand::new("Deploy", "cargo build");
    command.parameters = vec![rttx::commands::CommandParameter {
        name: "ENV".into(),
        label: "Environment".into(),
        choices: vec!["prod".into(), "dev".into()],
        default: Some("dev".into()),
    }];

    store.save_commands(&[command.clone()]).unwrap();
    let copy = command.duplicate();
    store.save_commands(&[command.clone(), copy]).unwrap();

    let loaded = store.load_commands();
    assert_eq!(loaded.len(), 2);
    assert_ne!(loaded[0].uuid, loaded[1].uuid);
    assert_eq!(loaded[1].title, "Deploy (copy)");
    assert_eq!(loaded[1].parameters, command.parameters);
}

#[test]
fn command_description_roundtrip() {
    let (_tmp, store) = test_store();

    let mut command = SavedCommand::new("Deploy", "cargo build");
    command.description = "Builds and deploys the production service".into();

    store.save_commands(&[command]).unwrap();
    let loaded = store.load_commands();
    assert_eq!(loaded[0].description, "Builds and deploys the production service");
}

#[test]
fn empty_description_not_serialized_in_json() {
    let command = SavedCommand::new("Plain", "echo hi");
    let json = serde_json::to_string(&command).unwrap();
    assert!(!json.contains("description"));
}

#[test]
fn commands_with_labels_roundtrip() {
    let (_tmp, store) = test_store();

    let mut command = SavedCommand::new("Deploy", "cargo build");
    command.labels = vec!["ops".into(), "deploy".into()];

    store.save_commands(&[command.clone()]).unwrap();
    let loaded = store.load_commands();
    assert_eq!(loaded, vec![command]);
}

#[test]
fn label_filtering_composes_with_text_search() {
    let mut c1 = SavedCommand::new("Deploy prod", "cargo build --release");
    c1.labels = vec!["ops".into(), "deploy".into()];
    let mut c2 = SavedCommand::new("Deploy dev", "cargo build");
    c2.labels = vec!["ops".into(), "dev".into()];
    let c3 = SavedCommand::new("Tail logs", "journalctl -fu app");

    let all = [c1, c2, c3];

    // Label filter alone
    let active = vec!["ops".into()];
    assert_eq!(all.iter().filter(|c| commands::matches_labels(c, &active)).count(), 2);

    // Label + text search compose with AND
    let filtered: Vec<_> = all
        .iter()
        .filter(|c| commands::matches_labels(c, &active))
        .filter(|c| commands::matches_query(c, "prod"))
        .collect();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].title, "Deploy prod");
}

#[test]
fn collect_labels_from_stored_commands() {
    let (_tmp, store) = test_store();

    let mut c1 = SavedCommand::new("A", "echo a");
    c1.labels = vec!["deploy".into(), "ops".into()];
    let mut c2 = SavedCommand::new("B", "echo b");
    c2.labels = vec!["ops".into(), "diag".into()];

    store.save_commands(&[c1, c2]).unwrap();
    let loaded = store.load_commands();
    let labels = commands::collect_labels(&loaded);
    assert_eq!(labels, vec!["deploy", "diag", "ops"]);
}

#[test]
fn run_in_new_pane_mode_roundtrip() {
    let (_tmp, store) = test_store();

    let mut command = SavedCommand::new("Build", "cargo build");
    command.default_run_mode = CommandRunMode::RunInNewPane;

    store.save_commands(&[command]).unwrap();
    let loaded = store.load_commands();
    assert_eq!(loaded[0].default_run_mode, CommandRunMode::RunInNewPane);
}
