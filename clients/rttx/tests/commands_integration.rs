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
