//! Pane recovery recipes: source, target, and startup steps.
//!
//! These types describe *what* a pane was doing so it can be reconstructed
//! after restart. They are persisted inside `SessionState.terminal_recovery`
//! and must remain backward-compatible.

use serde::{Deserialize, Serialize};

use crate::shell_quote;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneSource {
    EmptyShell,
    Bookmark { name: String },
    Command { title: String },
    SessionTemplate { name: String },
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
    LocalTmux { session: String },
    RemoteShell { ssh_target: String, remote_folder: Option<String> },
    RemoteTmux { ssh_target: String, tmux_session: String },
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
            _ => None,
        }
    }

    #[must_use]
    pub fn managed_startup_input(&self) -> Option<String> {
        match self {
            Self::LocalFolder { .. } => None,
            Self::LocalTmux { session } => Some(format!("exec {}\n", tmux_attach_command(session))),
            Self::RemoteShell { ssh_target, remote_folder } => {
                let remote_command = remote_folder.as_deref().map(remote_shell_command);
                Some(format!("exec {}\n", ssh_exec_command(ssh_target, remote_command.as_deref())))
            }
            Self::RemoteTmux { ssh_target, tmux_session } => Some(format!(
                "exec {}\n",
                ssh_exec_command(ssh_target, Some(&tmux_attach_command(tmux_session)))
            )),
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
            Self::LocalTmux { session } => {
                format!("Failed to attach local tmux session {session} (exit status {status})")
            }
            Self::RemoteShell { ssh_target, .. } => {
                format!("Failed to connect to {ssh_target} (exit status {status})")
            }
            Self::RemoteTmux { ssh_target, tmux_session } => {
                format!(
                    "Failed to attach tmux session {tmux_session} on {ssh_target} (exit status {status})"
                )
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

fn tmux_attach_command(session: &str) -> String {
    format!("tmux attach-session -t {}", shell_quote(session))
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
    fn pane_target_managed_startup_input_uses_attach_only_tmux() {
        assert_eq!(
            PaneTarget::LocalTmux { session: "dev".into() }.managed_startup_input().as_deref(),
            Some("exec tmux attach-session -t 'dev'\n")
        );
        assert_eq!(
            PaneTarget::RemoteTmux {
                ssh_target: "deploy@example.com".into(),
                tmux_session: "web".into(),
            }
            .managed_startup_input()
            .as_deref(),
            Some("exec ssh -t deploy@example.com 'tmux attach-session -t '\"'\"'web'\"'\"''\n")
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
}
