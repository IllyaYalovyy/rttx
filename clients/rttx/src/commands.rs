use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRunMode {
    Run,
    Insert,
    RunInNewPane,
}

/// A fixed-choice parameter declared on a saved command.
///
/// The command body references the parameter via `$NAME` or `${NAME}`.
/// At runtime rttx prompts for all declared parameters and injects them
/// as `export` statements.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandParameter {
    pub name: String,
    pub label: String,
    pub choices: Vec<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<CommandParameter>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Key sequence after the leader key (e.g. `["d", "k"]`).
    /// Duplicates across commands are intentional for host-scoped resolution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortcut_keys: Vec<String>,
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
            parameters: Vec::new(),
            description: String::new(),
            labels: Vec::new(),
            shortcut_keys: Vec::new(),
        }
    }

    /// Create a duplicate with a new UUID and a "(copy)" title suffix.
    #[must_use]
    pub fn duplicate(&self) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            title: format!("{} (copy)", self.title),
            ..self.clone()
        }
    }

    /// Whether this command has declared parameters.
    #[must_use]
    pub const fn has_parameters(&self) -> bool {
        !self.parameters.is_empty()
    }

    #[must_use]
    pub fn preview(&self) -> String {
        self.body.lines().next().unwrap_or_default().trim().to_string()
    }

    #[must_use]
    pub fn input_for(&self, run_mode: CommandRunMode) -> String {
        match run_mode {
            CommandRunMode::Run | CommandRunMode::RunInNewPane => format!("{}\n", self.body),
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
        || command.description.to_ascii_lowercase().contains(&query)
        || command.host_tags.iter().any(|tag| tag.to_ascii_lowercase().contains(&query))
        || command.labels.iter().any(|l| l.to_ascii_lowercase().contains(&query))
}

/// Returns `true` if the command has at least one label in `active_labels`.
///
/// When `active_labels` is empty, all commands match (no filter active).
#[must_use]
pub fn matches_labels(command: &SavedCommand, active_labels: &[String]) -> bool {
    if active_labels.is_empty() {
        return true;
    }
    command.labels.iter().any(|l| active_labels.contains(l))
}

/// Collect all distinct labels from a set of commands, sorted alphabetically.
#[must_use]
pub fn collect_labels(commands: &[SavedCommand]) -> Vec<String> {
    let mut labels: Vec<String> = commands.iter().flat_map(|c| c.labels.iter().cloned()).collect();
    labels.sort();
    labels.dedup();
    labels
}

/// Returns `true` if the command should be visible for the given host key.
///
/// Global commands (empty `host_tags`) are visible everywhere.
/// Tagged commands are visible only when `host_key` matches one of their tags.
#[must_use]
pub fn is_visible_on(command: &SavedCommand, host_key: &str) -> bool {
    command.host_tags.is_empty() || command.host_tags.iter().any(|tag| tag == host_key)
}

/// Collect commands visible on `host_key`: matching saved commands.
#[must_use]
pub fn visible_for_host(saved: &[SavedCommand], host_key: &str) -> Vec<SavedCommand> {
    saved.iter().filter(|c| is_visible_on(c, host_key)).cloned().collect()
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

/// Shell-escape a value for safe inclusion in a single-quoted string.
///
/// Wraps the value in single quotes, escaping any embedded single quotes.
#[must_use]
pub fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Resolve the effective default value for a parameter.
///
/// Returns `default` if present in `choices`, otherwise the first choice,
/// otherwise the empty string.
#[must_use]
pub fn resolve_default(param: &CommandParameter) -> &str {
    if let Some(ref d) = param.default
        && param.choices.contains(d)
    {
        return d;
    }
    param.choices.first().map_or("", String::as_str)
}

/// Render the env-var injection block for a parameterized command.
///
/// Wraps the body in a subshell with `export` statements for each parameter
/// in declaration order.
#[must_use]
pub fn render_env_block(body: &str, values: &[(String, String)]) -> String {
    use std::fmt::Write;
    let mut result = String::from("(\n");
    for (name, value) in values {
        let _ = writeln!(result, "export {}={}", name, shell_escape(value));
    }
    result.push_str(body);
    if !body.ends_with('\n') {
        result.push('\n');
    }
    result.push(')');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn multiline_command_serde_roundtrip() {
        let mut command =
            SavedCommand::new("Deploy", "cd /srv/app\ncargo build\nsystemctl restart app");
        command.default_run_mode = CommandRunMode::Insert;

        let json = serde_json::to_string(&[&command]).unwrap();
        let loaded: Vec<SavedCommand> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, vec![command]);
    }

    #[test]
    fn matches_query_checks_title_and_body() {
        let command = SavedCommand::new("Tail logs", "journalctl -fu app.service");
        assert!(matches_query(&command, "tail"));
        assert!(matches_query(&command, "journalctl"));
        assert!(!matches_query(&command, "deploy"));
    }

    #[test]
    fn matches_query_searches_description() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.description = "Builds and deploys the production service".into();
        assert!(matches_query(&command, "production"));
        assert!(!matches_query(&command, "staging"));
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
    fn run_in_new_pane_appends_newline_like_run() {
        let command = SavedCommand::new("Build", "cargo test\ncargo clippy");
        assert_eq!(command.input_for(CommandRunMode::RunInNewPane), "cargo test\ncargo clippy\n");
    }

    #[test]
    fn run_in_new_pane_serde_roundtrip() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.default_run_mode = CommandRunMode::RunInNewPane;

        let json = serde_json::to_string(&[&command]).unwrap();
        let loaded: Vec<SavedCommand> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded[0].default_run_mode, CommandRunMode::RunInNewPane);
    }

    #[test]
    fn run_in_new_pane_serializes_as_kebab_case() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.default_run_mode = CommandRunMode::RunInNewPane;

        let json = serde_json::to_string(&command).unwrap();
        assert!(json.contains("\"run-in-new-pane\""));
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
    fn reorder_preserves_order_through_serde() {
        let mut items = vec![
            SavedCommand { uuid: "a".into(), ..SavedCommand::new("A", "echo a") },
            SavedCommand { uuid: "b".into(), ..SavedCommand::new("B", "echo b") },
            SavedCommand { uuid: "c".into(), ..SavedCommand::new("C", "echo c") },
        ];

        reorder(&mut items, "c", "b");
        let json = serde_json::to_string(&items).unwrap();
        let loaded: Vec<SavedCommand> = serde_json::from_str(&json).unwrap();
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
    fn host_tags_serde_roundtrip() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.host_tags = vec!["local".into(), "example.com".into()];

        let json = serde_json::to_string(&[&command]).unwrap();
        let loaded: Vec<SavedCommand> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, vec![command]);
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
    fn visible_for_host_returns_matching_commands() {
        let mut local_cmd = SavedCommand::new("Local", "echo local");
        local_cmd.host_tags = vec!["local".into()];
        let mut remote_cmd = SavedCommand::new("Remote", "echo remote");
        remote_cmd.host_tags = vec!["example.com".into()];
        let global_cmd = SavedCommand::new("Global", "echo global");

        let saved = vec![local_cmd, remote_cmd, global_cmd];
        let visible = visible_for_host(&saved, "local");
        let names: Vec<&str> = visible.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(names, vec!["Local", "Global"]);
    }

    #[test]
    fn visible_for_host_with_no_commands_returns_empty() {
        let visible = visible_for_host(&[], "local");
        assert!(visible.is_empty());
    }

    // ── Parameters ──────────────────────────────────────────────

    #[test]
    fn shell_escape_plain_value() {
        assert_eq!(shell_escape("prod"), "'prod'");
    }

    #[test]
    fn shell_escape_value_with_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn shell_escape_value_with_single_quotes() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_value_with_dollar_sign() {
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
    }

    #[test]
    fn shell_escape_value_with_semicolons_and_newlines() {
        assert_eq!(shell_escape("a;b\nc"), "'a;b\nc'");
    }

    #[test]
    fn shell_escape_empty_string() {
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn render_env_block_single_parameter() {
        let result =
            render_env_block("systemctl restart $SERVICE", &[("SERVICE".into(), "api".into())]);
        assert_eq!(result, "(\nexport SERVICE='api'\nsystemctl restart $SERVICE\n)");
    }

    #[test]
    fn render_env_block_multiple_parameters_preserves_order() {
        let result = render_env_block(
            "kubectl logs -n $NS $POD",
            &[("NS".into(), "prod".into()), ("POD".into(), "web-1".into())],
        );
        assert_eq!(result, "(\nexport NS='prod'\nexport POD='web-1'\nkubectl logs -n $NS $POD\n)");
    }

    #[test]
    fn render_env_block_body_with_trailing_newline() {
        let result = render_env_block("echo done\n", &[("X".into(), "1".into())]);
        assert_eq!(result, "(\nexport X='1'\necho done\n)");
    }

    #[test]
    fn render_env_block_escapes_values() {
        let result = render_env_block("echo $MSG", &[("MSG".into(), "it's a test".into())]);
        assert_eq!(result, "(\nexport MSG='it'\\''s a test'\necho $MSG\n)");
    }

    #[test]
    fn resolve_default_uses_declared_default_when_in_choices() {
        let param = CommandParameter {
            name: "ENV".into(),
            label: "Environment".into(),
            choices: vec!["dev".into(), "staging".into(), "prod".into()],
            default: Some("staging".into()),
            description: String::new(),
        };
        assert_eq!(resolve_default(&param), "staging");
    }

    #[test]
    fn resolve_default_falls_back_to_first_choice_when_default_not_in_choices() {
        let param = CommandParameter {
            name: "ENV".into(),
            label: "Environment".into(),
            choices: vec!["dev".into(), "prod".into()],
            default: Some("staging".into()),
            description: String::new(),
        };
        assert_eq!(resolve_default(&param), "dev");
    }

    #[test]
    fn resolve_default_falls_back_to_first_choice_when_no_default() {
        let param = CommandParameter {
            name: "ENV".into(),
            label: "Environment".into(),
            choices: vec!["dev".into(), "prod".into()],
            default: None,
            description: String::new(),
        };
        assert_eq!(resolve_default(&param), "dev");
    }

    #[test]
    fn resolve_default_returns_empty_when_no_choices() {
        let param = CommandParameter {
            name: "ENV".into(),
            label: "Environment".into(),
            choices: vec![],
            default: None,
            description: String::new(),
        };
        assert_eq!(resolve_default(&param), "");
    }

    #[test]
    fn duplicate_creates_new_uuid_and_copy_title() {
        let mut original = SavedCommand::new("Deploy", "cargo build");
        original.parameters = vec![CommandParameter {
            name: "ENV".into(),
            label: "Environment".into(),
            choices: vec!["prod".into()],
            default: None,
            description: String::new(),
        }];
        original.description = "Deploys the app".into();
        original.labels = vec!["deploy".into()];

        let copy = original.duplicate();
        assert_ne!(copy.uuid, original.uuid);
        assert_eq!(copy.title, "Deploy (copy)");
        assert_eq!(copy.body, original.body);
        assert_eq!(copy.parameters, original.parameters);
        assert_eq!(copy.description, original.description);
        assert_eq!(copy.labels, original.labels);
        assert_eq!(copy.host_tags, original.host_tags);
    }

    #[test]
    fn has_parameters_returns_false_for_plain_command() {
        let command = SavedCommand::new("Test", "echo hi");
        assert!(!command.has_parameters());
    }

    #[test]
    fn has_parameters_returns_true_when_parameters_present() {
        let mut command = SavedCommand::new("Test", "echo $X");
        command.parameters = vec![CommandParameter {
            name: "X".into(),
            label: "X".into(),
            choices: vec!["1".into()],
            default: None,
            description: String::new(),
        }];
        assert!(command.has_parameters());
    }

    #[test]
    fn serde_roundtrip_with_parameters() {
        let mut command = SavedCommand::new("Parameterized", "systemctl restart $SERVICE");
        command.parameters = vec![CommandParameter {
            name: "SERVICE".into(),
            label: "Service name".into(),
            choices: vec!["api".into(), "web".into(), "worker".into()],
            default: Some("api".into()),
            description: String::new(),
        }];
        command.description = "Restart a service".into();
        command.labels = vec!["ops".into(), "restart".into()];

        let json = serde_json::to_string(&[&command]).unwrap();
        let loaded: Vec<SavedCommand> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, vec![command]);
    }

    #[test]
    fn legacy_json_without_new_fields_deserializes_with_defaults() {
        let json = r#"[{
            "uuid": "abc",
            "title": "Old",
            "body": "echo old",
            "default_run_mode": "run"
        }]"#;
        let commands: Vec<SavedCommand> = serde_json::from_str(json).unwrap();
        assert!(commands[0].parameters.is_empty());
        assert!(commands[0].description.is_empty());
        assert!(commands[0].labels.is_empty());
    }

    #[test]
    fn empty_parameters_not_serialized() {
        let command = SavedCommand::new("Plain", "echo hi");
        let json = serde_json::to_string(&command).unwrap();
        assert!(!json.contains("parameters"));
        assert!(!json.contains("description"));
        assert!(!json.contains("labels"));
    }

    // ── Label filtering ─────────────────────────────────────────

    #[test]
    fn matches_query_searches_labels() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.labels = vec!["ops".into(), "deploy".into()];
        assert!(matches_query(&command, "ops"));
        assert!(matches_query(&command, "deploy"));
        assert!(!matches_query(&command, "diag"));
    }

    #[test]
    fn matches_labels_empty_filter_matches_all() {
        let command = SavedCommand::new("Test", "echo hi");
        assert!(matches_labels(&command, &[]));
    }

    #[test]
    fn matches_labels_returns_true_when_command_has_matching_label() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.labels = vec!["ops".into(), "deploy".into()];
        assert!(matches_labels(&command, &["ops".into()]));
        assert!(matches_labels(&command, &["deploy".into()]));
    }

    #[test]
    fn matches_labels_returns_false_when_no_match() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.labels = vec!["ops".into()];
        assert!(!matches_labels(&command, &["diag".into()]));
    }

    #[test]
    fn matches_labels_unlabeled_command_excluded_when_filter_active() {
        let command = SavedCommand::new("Plain", "echo hi");
        assert!(!matches_labels(&command, &["ops".into()]));
    }

    #[test]
    fn collect_labels_returns_sorted_unique_labels() {
        let mut c1 = SavedCommand::new("A", "echo a");
        c1.labels = vec!["deploy".into(), "ops".into()];
        let mut c2 = SavedCommand::new("B", "echo b");
        c2.labels = vec!["ops".into(), "diag".into()];
        let c3 = SavedCommand::new("C", "echo c");

        let labels = collect_labels(&[c1, c2, c3]);
        assert_eq!(labels, vec!["deploy", "diag", "ops"]);
    }

    #[test]
    fn collect_labels_empty_commands_returns_empty() {
        let labels = collect_labels(&[]);
        assert!(labels.is_empty());
    }

    #[test]
    fn shortcut_keys_serde_roundtrip() {
        let mut command = SavedCommand::new("Deploy", "cargo build");
        command.shortcut_keys = vec!["d".into(), "k".into()];

        let json = serde_json::to_string(&[&command]).unwrap();
        let loaded: Vec<SavedCommand> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded[0].shortcut_keys, vec!["d", "k"]);
    }

    #[test]
    fn shortcut_keys_empty_not_serialized() {
        let command = SavedCommand::new("Plain", "echo hi");
        let json = serde_json::to_string(&command).unwrap();
        assert!(!json.contains("shortcut_keys"));
    }

    #[test]
    fn duplicate_preserves_shortcut_keys() {
        let mut original = SavedCommand::new("Deploy", "cargo build");
        original.shortcut_keys = vec!["d".into()];
        let copy = original.duplicate();
        assert_eq!(copy.shortcut_keys, vec!["d"]);
    }

    // ── Parameter description ───────────────────────────────────

    #[test]
    fn parameter_description_serde_roundtrip() {
        let mut command = SavedCommand::new("Restart", "systemctl restart $SERVICE");
        command.parameters = vec![CommandParameter {
            name: "SERVICE".into(),
            label: "Service".into(),
            choices: vec!["api".into(), "web".into()],
            default: None,
            description: "Which systemd service to restart".into(),
        }];

        let json = serde_json::to_string(&[&command]).unwrap();
        let loaded: Vec<SavedCommand> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded[0].parameters[0].description, "Which systemd service to restart");
    }

    #[test]
    fn parameter_empty_description_not_serialized() {
        let param = CommandParameter {
            name: "ENV".into(),
            label: "Environment".into(),
            choices: vec!["prod".into()],
            default: None,
            description: String::new(),
        };
        let json = serde_json::to_string(&param).unwrap();
        assert!(!json.contains("description"));
    }

    #[test]
    fn legacy_parameter_json_without_description_deserializes() {
        let json = r#"{
            "name": "ENV",
            "label": "Environment",
            "choices": ["prod", "staging"],
            "default": "prod"
        }"#;
        let param: CommandParameter = serde_json::from_str(json).unwrap();
        assert!(param.description.is_empty());
    }

    #[test]
    fn duplicate_preserves_parameter_description() {
        let mut original = SavedCommand::new("Deploy", "cargo build");
        original.parameters = vec![CommandParameter {
            name: "ENV".into(),
            label: "Environment".into(),
            choices: vec!["prod".into()],
            default: None,
            description: "Target deployment environment".into(),
        }];
        let copy = original.duplicate();
        assert_eq!(copy.parameters[0].description, "Target deployment environment");
    }
}
