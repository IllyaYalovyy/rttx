//! Pane recovery recipes: source, target, and startup steps.
//!
//! These types describe *what* a pane was doing so it can be reconstructed
//! after restart. They are persisted inside `WorkspaceState.terminal_recovery`
//! and tolerate files that omit optional recovery fields.

use serde::{Deserialize, Serialize};

use crate::shell_quote;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneSource {
    EmptyShell,
    Command { title: String },
    Manual,
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
    #[serde(default)]
    pub target: Option<PaneTarget>,
    #[serde(default)]
    pub startup: Vec<StartupStep>,
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
    fn unknown_pane_source_variant_fails_to_deserialize() {
        let json = r#"{"source":{"unrecognized_source":{"name":"X"}}}"#;
        assert!(serde_json::from_str::<PaneRecovery>(json).is_err());
    }

    #[test]
    fn unknown_pane_target_variant_fails_to_deserialize() {
        let json = r#"{"source":"empty-shell","target":{"unrecognized_target":{"x":"y"}}}"#;
        assert!(serde_json::from_str::<PaneRecovery>(json).is_err());
    }
}
