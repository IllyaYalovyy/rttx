use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRunMode {
    Run,
    Insert,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedCommand {
    pub uuid: String,
    pub title: String,
    pub body: String,
    #[serde(default = "default_run_mode")]
    pub default_run_mode: CommandRunMode,
    /// Host keys this command is scoped to. Empty means global (visible everywhere).
    #[serde(default)]
    pub host_tags: Vec<String>,
}

const fn default_run_mode() -> CommandRunMode {
    CommandRunMode::Run
}

impl SavedCommand {
    #[must_use]
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            title: title.into(),
            body: body.into(),
            default_run_mode: CommandRunMode::Run,
            host_tags: Vec::new(),
        }
    }

    #[must_use]
    pub fn preview(&self) -> String {
        self.body.lines().next().unwrap_or_default().trim().to_string()
    }

    #[must_use]
    pub fn input_for(&self, run_mode: CommandRunMode) -> String {
        match run_mode {
            CommandRunMode::Run => format!("{}\n", self.body),
            CommandRunMode::Insert => self.body.clone(),
        }
    }
}

#[must_use]
pub fn matches_query(command: &SavedCommand, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }

    let query = query.to_ascii_lowercase();
    command.title.to_ascii_lowercase().contains(&query)
        || command.body.to_ascii_lowercase().contains(&query)
        || command.host_tags.iter().any(|tag| tag.to_ascii_lowercase().contains(&query))
}

/// Returns `true` if the command should be visible for the given host key.
///
/// Global commands (empty `host_tags`) are visible everywhere.
/// Tagged commands are visible only when `host_key` matches one of their tags.
#[must_use]
pub fn is_visible_on(command: &SavedCommand, host_key: &str) -> bool {
    command.host_tags.is_empty() || command.host_tags.iter().any(|tag| tag == host_key)
}

/// Migrate legacy commands that lack host tags by tagging them with `"local"`.
pub fn migrate_legacy(commands: &mut [SavedCommand]) {
    for command in commands.iter_mut() {
        if command.host_tags.is_empty() {
            command.host_tags.push(crate::host::LOCAL_KEY.into());
        }
    }
}

fn commands_path() -> PathBuf {
    let mut path = config::config_dir_path();
    path.push("commands.json");
    path
}

#[must_use]
pub fn load() -> Vec<SavedCommand> {
    load_from(&commands_path())
}

pub fn save(commands: &[SavedCommand]) -> Result<(), Box<dyn std::error::Error>> {
    save_to(commands, &commands_path())
}

#[must_use]
pub fn load_from(path: &Path) -> Vec<SavedCommand> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<Vec<SavedCommand>>(&data).ok())
        .unwrap_or_default()
}

/// Move the item with `source_uuid` to the position of `target_uuid`.
pub fn reorder(items: &mut Vec<SavedCommand>, source_uuid: &str, target_uuid: &str) {
    let Some(src) = items.iter().position(|c| c.uuid == source_uuid) else {
        return;
    };
    let Some(tgt) = items.iter().position(|c| c.uuid == target_uuid) else {
        return;
    };
    let item = items.remove(src);
    items.insert(tgt, item);
}

pub fn save_to(commands: &[SavedCommand], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(commands)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn multiline_command_roundtrips_without_loss() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("commands.json");
        let mut command =
            SavedCommand::new("Deploy", "cd /srv/app\ncargo build\nsystemctl restart app");
        command.default_run_mode = CommandRunMode::Insert;

        save_to(&[command.clone()], &path).unwrap();
        assert_eq!(load_from(&path), vec![command]);
    }

    #[test]
    fn matches_query_checks_title_and_body() {
        let command = SavedCommand::new("Tail logs", "journalctl -fu app.service");
        assert!(matches_query(&command, "tail"));
        assert!(matches_query(&command, "journalctl"));
        assert!(!matches_query(&command, "deploy"));
    }

    #[test]
    fn blank_query_matches_all_commands() {
        let command = SavedCommand::new("Anything", "echo hi");
        assert!(matches_query(&command, ""));
        assert!(matches_query(&command, "   "));
    }

    #[test]
    fn run_mode_appends_newline_but_insert_does_not() {
        let command = SavedCommand::new("Build", "cargo test\ncargo clippy");
        assert_eq!(command.input_for(CommandRunMode::Run), "cargo test\ncargo clippy\n");
        assert_eq!(command.input_for(CommandRunMode::Insert), "cargo test\ncargo clippy");
    }

    #[test]
    fn preview_uses_first_non_empty_line_verbatim() {
        let command = SavedCommand::new("Build", "cargo test\ncargo clippy");
        assert_eq!(command.preview(), "cargo test");
    }

    #[test]
    fn reorder_moves_item_to_target_position() {
        let mut items = vec![
            SavedCommand { uuid: "a".into(), ..SavedCommand::new("A", "echo a") },
            SavedCommand { uuid: "b".into(), ..SavedCommand::new("B", "echo b") },
            SavedCommand { uuid: "c".into(), ..SavedCommand::new("C", "echo c") },
        ];

        reorder(&mut items, "c", "a");
        let uuids: Vec<&str> = items.iter().map(|c| c.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_noop_for_unknown_uuid() {
        let mut items = vec![
            SavedCommand { uuid: "a".into(), ..SavedCommand::new("A", "echo a") },
            SavedCommand { uuid: "b".into(), ..SavedCommand::new("B", "echo b") },
        ];

        reorder(&mut items, "z", "a");
        let uuids: Vec<&str> = items.iter().map(|c| c.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["a", "b"]);
    }

    #[test]
    fn reorder_persists_through_save_and_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("commands.json");
        let mut items = vec![
            SavedCommand { uuid: "a".into(), ..SavedCommand::new("A", "echo a") },
            SavedCommand { uuid: "b".into(), ..SavedCommand::new("B", "echo b") },
            SavedCommand { uuid: "c".into(), ..SavedCommand::new("C", "echo c") },
        ];

        reorder(&mut items, "c", "b");
        save_to(&items, &path).unwrap();

        let loaded = load_from(&path);
        let uuids: Vec<&str> = loaded.iter().map(|c| c.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["a", "c", "b"]);
    }

    // ── Host tags ───────────────────────────────────────────────

    #[test]
    fn new_command_has_empty_host_tags() {
        let command = SavedCommand::new("Test", "echo test");
        assert!(command.host_tags.is_empty());
    }

    #[test]
    fn host_tags_roundtrip_via_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("commands.json");
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.host_tags = vec!["local".into(), "example.com".into()];

        save_to(&[command.clone()], &path).unwrap();
        assert_eq!(load_from(&path), vec![command]);
    }

    #[test]
    fn legacy_json_without_host_tags_deserializes_with_empty_vec() {
        let json = r#"[{
            "uuid": "abc",
            "title": "Old",
            "body": "echo old",
            "default_run_mode": "run"
        }]"#;
        let commands: Vec<SavedCommand> = serde_json::from_str(json).unwrap();
        assert_eq!(commands[0].host_tags, Vec::<String>::new());
    }

    #[test]
    fn matches_query_searches_host_tags() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.host_tags = vec!["example.com".into()];
        assert!(matches_query(&command, "example"));
        assert!(!matches_query(&command, "staging"));
    }

    #[test]
    fn is_visible_on_global_command_visible_everywhere() {
        let command = SavedCommand::new("Global", "echo hi");
        assert!(is_visible_on(&command, "local"));
        assert!(is_visible_on(&command, "example.com"));
    }

    #[test]
    fn is_visible_on_tagged_command_visible_only_on_matching_host() {
        let mut command = SavedCommand::new("Local only", "echo hi");
        command.host_tags = vec!["local".into()];
        assert!(is_visible_on(&command, "local"));
        assert!(!is_visible_on(&command, "example.com"));
    }

    #[test]
    fn is_visible_on_multi_tagged_command() {
        let mut command = SavedCommand::new("Multi", "echo hi");
        command.host_tags = vec!["local".into(), "example.com".into()];
        assert!(is_visible_on(&command, "local"));
        assert!(is_visible_on(&command, "example.com"));
        assert!(!is_visible_on(&command, "other.com"));
    }

    #[test]
    fn migrate_legacy_tags_untagged_commands_with_local() {
        let mut commands = vec![SavedCommand::new("A", "echo a"), SavedCommand::new("B", "echo b")];
        migrate_legacy(&mut commands);
        assert_eq!(commands[0].host_tags, vec!["local"]);
        assert_eq!(commands[1].host_tags, vec!["local"]);
    }

    #[test]
    fn migrate_legacy_preserves_existing_tags() {
        let mut command = SavedCommand::new("Tagged", "echo tagged");
        command.host_tags = vec!["example.com".into()];
        let mut commands = vec![command];
        migrate_legacy(&mut commands);
        assert_eq!(commands[0].host_tags, vec!["example.com"]);
    }
}
