//! Canonical workspaces document model (RFC-023 §3: `workspaces.json`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Workspaces;
pub const CURRENT_VERSION: u32 = 1;

/// Workspace accent color.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceColor {
    #[default]
    Blue,
    Green,
    Yellow,
    Red,
    Purple,
    Pink,
    Teal,
    Orange,
}

/// Runtime retention policy.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspacePolicy {
    #[default]
    Persistent,
    Ephemeral,
}

/// Durable reference to a daemon runtime for reconnection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeRef {
    pub runtime_id: String,
    #[serde(default = "default_attachment_kind")]
    pub attachment_kind: RuntimeAttachmentKind,
}

/// How the workspace was attached to the runtime.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeAttachmentKind {
    #[default]
    Created,
    Attached,
    Recovered,
}

const fn default_attachment_kind() -> RuntimeAttachmentKind {
    RuntimeAttachmentKind::Created
}

/// Input synchronization state.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InputSyncState {
    #[default]
    Off,
    On,
}

/// Pane recovery source — what the pane was doing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneSource {
    EmptyShell,
    Command { title: String },
    Manual,
}

/// Pane recovery target — where the pane was pointed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneTarget {
    LocalFolder { path: String },
    RemoteShell { ssh_target: String, remote_folder: Option<String> },
}

/// Startup step for pane recovery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StartupStep {
    SendText { text: String, execute: bool },
}

/// Per-pane recovery recipe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneRecoveryRecord {
    pub source: PaneSource,
    #[serde(default)]
    pub target: Option<PaneTarget>,
    #[serde(default)]
    pub startup: Vec<StartupStep>,
}

/// Split orientation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SplitOrientation {
    Horizontal,
    Vertical,
}

/// Recursive layout tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutNode {
    Terminal {
        uuid: String,
        #[serde(default)]
        profile: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        custom_title: Option<String>,
    },
    Split {
        orientation: SplitOrientation,
        ratio: f64,
        first: Box<Self>,
        second: Box<Self>,
    },
}

/// A single workspace record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub user_renamed: bool,
    #[serde(default = "default_endpoint_key")]
    pub endpoint_key: String,
    #[serde(default)]
    pub policy: WorkspacePolicy,
    #[serde(default)]
    pub runtime_ref: Option<RuntimeRef>,
    pub layout: LayoutNode,
    #[serde(default)]
    pub active_pane_id: Option<String>,
    #[serde(default)]
    pub zoomed_pane_id: Option<String>,
    #[serde(default)]
    pub input_sync: InputSyncState,
    #[serde(default)]
    pub color: WorkspaceColor,
    #[serde(default)]
    pub pane_recovery: BTreeMap<String, PaneRecoveryRecord>,
}

fn default_endpoint_key() -> String {
    "local".into()
}

/// Top-level workspace store document.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceStore {
    #[serde(default)]
    pub active_workspace_id: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceRecord>,
}
