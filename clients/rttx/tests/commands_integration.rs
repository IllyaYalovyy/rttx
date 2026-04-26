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
