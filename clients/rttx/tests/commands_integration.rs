use rttx::commands::{self, CommandRunMode, SavedCommand};
use tempfile::TempDir;

#[test]
fn commands_roundtrip_multiline_and_mode() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("commands.json");

    let mut command = SavedCommand::new("Deploy", "cargo build\ncargo test\nsystemctl restart app");
    command.default_run_mode = CommandRunMode::Insert;

    commands::save_to(&[command.clone()], &path).unwrap();
    assert_eq!(commands::load_from(&path), vec![command]);
}

#[test]
fn invalid_command_json_returns_empty_list() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("commands.json");

    std::fs::write(&path, "{not-json").unwrap();
    assert!(commands::load_from(&path).is_empty());
}

#[test]
fn commands_with_host_tags_roundtrip() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("commands.json");

    let mut command = SavedCommand::new("Remote deploy", "cargo build");
    command.host_tags = vec!["local".into(), "example.com".into()];

    commands::save_to(&[command.clone()], &path).unwrap();
    let loaded = commands::load_from(&path);
    assert_eq!(loaded, vec![command]);
}

#[test]
fn legacy_commands_without_host_tags_load_and_migrate() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("commands.json");

    let json = r#"[{
        "uuid": "abc-123",
        "title": "Old command",
        "body": "echo hello",
        "default_run_mode": "run"
    }]"#;
    std::fs::write(&path, json).unwrap();

    let mut loaded = commands::load_from(&path);
    assert!(loaded[0].host_tags.is_empty(), "legacy commands load with empty tags");

    commands::migrate_legacy(&mut loaded);
    assert_eq!(loaded[0].host_tags, vec!["local"], "migration tags with local");

    commands::save_to(&loaded, &path).unwrap();
    let reloaded = commands::load_from(&path);
    assert_eq!(reloaded[0].host_tags, vec!["local"], "migrated tags persist");
}
