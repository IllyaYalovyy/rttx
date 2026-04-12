//! Pane recovery recipes: source, target, and startup steps.
//!
//! These types describe *what* a pane was doing so it can be reconstructed
//! after restart. They are persisted inside `SessionState.terminal_recovery`
//! and must remain backward-compatible.

use serde::{Deserialize, Serialize};

use crate::shell_quote;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneSource {
    EmptyShell,
    Bookmark { name: String },
    Command { title: String },
    Manual,
}

impl<'de> Deserialize<'de> for PaneSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Mirror of `PaneSource` that includes removed variants for backward
        /// compatibility with persisted state.
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        #[allow(dead_code)] // fields in removed variants are intentionally discarded
        enum Raw {
            EmptyShell,
            Bookmark { name: String },
            Command { title: String },
            SessionTemplate { name: String },
            Manual,
        }
        match Raw::deserialize(deserializer)? {
            Raw::EmptyShell => Ok(Self::EmptyShell),
            Raw::Bookmark { name } => Ok(Self::Bookmark { name }),
            Raw::Command { title } => Ok(Self::Command { title }),
            Raw::SessionTemplate { .. } | Raw::Manual => Ok(Self::Manual),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StartupStep {
    SendText { text: String, execute: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneTarget {
    LocalFolder { path: String },
    RemoteShell { ssh_target: String, remote_folder: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneRecovery {
    pub source: PaneSource,
    #[serde(default, deserialize_with = "deserialize_pane_target")]
    pub target: Option<PaneTarget>,
    #[serde(default)]
    pub startup: Vec<StartupStep>,
}

/// Deserializes `Option<PaneTarget>`, mapping removed variants (local-tmux,
/// remote-tmux) to `None` so old persisted state loads without error.
fn deserialize_pane_target<'de, D>(deserializer: D) -> Result<Option<PaneTarget>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    Ok(serde_json::from_value::<PaneTarget>(value).ok())
}

impl PaneRecovery {
    #[must_use]
    pub const fn empty_shell() -> Self {
        Self { source: PaneSource::EmptyShell, target: None, startup: Vec::new() }
    }
}

impl PaneTarget {
    #[must_use]
    pub const fn initial_cwd(&self) -> Option<&str> {
        match self {
            Self::LocalFolder { path } => Some(path.as_str()),
            Self::RemoteShell { .. } => None,
        }
    }

    #[must_use]
    pub fn managed_startup_input(&self) -> Option<String> {
        match self {
            Self::LocalFolder { .. } => None,
            Self::RemoteShell { ssh_target, remote_folder } => {
                let remote_command = remote_folder.as_deref().map(remote_shell_command);
                Some(format!("exec {}\n", ssh_exec_command(ssh_target, remote_command.as_deref())))
            }
        }
    }

    #[must_use]
    pub const fn manages_child_lifecycle(&self) -> bool {
        !matches!(self, Self::LocalFolder { .. })
    }

    #[must_use]
    pub fn failure_message(&self, status: i32) -> String {
        match self {
            Self::LocalFolder { path } => {
                format!("Failed to open local folder {path} (exit status {status})")
            }
            Self::RemoteShell { ssh_target, .. } => {
                format!("Failed to connect to {ssh_target} (exit status {status})")
            }
        }
    }
}

impl StartupStep {
    #[must_use]
    pub fn terminal_input(&self) -> String {
        match self {
            Self::SendText { text, execute } => {
                if *execute {
                    format!("{text}\n")
                } else {
                    text.clone()
                }
            }
        }
    }
}

fn remote_shell_command(path: &str) -> String {
    format!("cd {} && exec ${{SHELL:-/bin/bash}} -l", shell_quote(path))
}

fn ssh_exec_command(ssh_target: &str, remote_command: Option<&str>) -> String {
    remote_command.map_or_else(
        || format!("ssh {ssh_target}"),
        |remote_command| format!("ssh -t {ssh_target} {}", shell_quote(remote_command)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_step_terminal_input_respects_execute_flag() {
        assert_eq!(
            StartupStep::SendText { text: "echo hi".into(), execute: true }.terminal_input(),
            "echo hi\n"
        );
        assert_eq!(
            StartupStep::SendText { text: "echo hi".into(), execute: false }.terminal_input(),
            "echo hi"
        );
    }

    #[test]
    fn pane_target_remote_shell_with_folder_builds_replayable_command() {
        assert_eq!(
            PaneTarget::RemoteShell {
                ssh_target: "deploy@example.com".into(),
                remote_folder: Some("/srv/app".into()),
            }
            .managed_startup_input()
            .as_deref(),
            Some(
                "exec ssh -t deploy@example.com 'cd '\"'\"'/srv/app'\"'\"' && exec ${SHELL:-/bin/bash} -l'\n"
            )
        );
    }

    #[test]
    fn legacy_local_tmux_target_deserializes_as_none() {
        let json = r#"{"source":"empty-shell","target":{"local-tmux":{"session":"dev"}}}"#;
        let recovery: PaneRecovery = serde_json::from_str(json).unwrap();
        assert_eq!(recovery.target, None);
    }

    #[test]
    fn legacy_remote_tmux_target_deserializes_as_none() {
        let json = r#"{"source":{"bookmark":{"name":"Prod"}},"target":{"remote-tmux":{"ssh_target":"host","tmux_session":"web"}}}"#;
        let recovery: PaneRecovery = serde_json::from_str(json).unwrap();
        assert_eq!(recovery.target, None);
    }

    #[test]
    fn legacy_session_template_source_deserializes_as_manual() {
        let json = r#"{"source":{"session-template":{"name":"Dev Setup"}}}"#;
        let recovery: PaneRecovery = serde_json::from_str(json).unwrap();
        assert_eq!(recovery.source, PaneSource::Manual);
    }

    #[test]
    fn session_template_variant_absent_from_enum() {
        let json = serde_json::to_string(&PaneSource::Manual).unwrap();
        assert!(!json.contains("session-template"));
    }
}
