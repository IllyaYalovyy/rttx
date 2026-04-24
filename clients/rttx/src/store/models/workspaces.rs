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

// ── Conversions from domain types ───────────────────────────────

use crate::runtime::{RuntimeEndpoint, WorkspaceRuntime};
use crate::workspace::recovery;
use crate::workspace::state;

impl From<&state::WorkspaceColor> for WorkspaceColor {
    fn from(c: &state::WorkspaceColor) -> Self {
        match c {
            state::WorkspaceColor::Blue => Self::Blue,
            state::WorkspaceColor::Green => Self::Green,
            state::WorkspaceColor::Yellow => Self::Yellow,
            state::WorkspaceColor::Red => Self::Red,
            state::WorkspaceColor::Purple => Self::Purple,
            state::WorkspaceColor::Pink => Self::Pink,
            state::WorkspaceColor::Teal => Self::Teal,
            state::WorkspaceColor::Orange => Self::Orange,
        }
    }
}

impl From<&WorkspaceColor> for state::WorkspaceColor {
    fn from(c: &WorkspaceColor) -> Self {
        match c {
            WorkspaceColor::Blue => Self::Blue,
            WorkspaceColor::Green => Self::Green,
            WorkspaceColor::Yellow => Self::Yellow,
            WorkspaceColor::Red => Self::Red,
            WorkspaceColor::Purple => Self::Purple,
            WorkspaceColor::Pink => Self::Pink,
            WorkspaceColor::Teal => Self::Teal,
            WorkspaceColor::Orange => Self::Orange,
        }
    }
}

impl From<&crate::runtime::WorkspacePolicy> for WorkspacePolicy {
    fn from(p: &crate::runtime::WorkspacePolicy) -> Self {
        match p {
            crate::runtime::WorkspacePolicy::Persistent => Self::Persistent,
            crate::runtime::WorkspacePolicy::Ephemeral => Self::Ephemeral,
        }
    }
}

impl From<&WorkspacePolicy> for crate::runtime::WorkspacePolicy {
    fn from(p: &WorkspacePolicy) -> Self {
        match p {
            WorkspacePolicy::Persistent => Self::Persistent,
            WorkspacePolicy::Ephemeral => Self::Ephemeral,
        }
    }
}

impl From<&recovery::PaneSource> for PaneSource {
    fn from(s: &recovery::PaneSource) -> Self {
        match s {
            recovery::PaneSource::EmptyShell => Self::EmptyShell,
            recovery::PaneSource::Command { title } => Self::Command { title: title.clone() },
            recovery::PaneSource::Manual => Self::Manual,
        }
    }
}

impl From<&PaneSource> for recovery::PaneSource {
    fn from(s: &PaneSource) -> Self {
        match s {
            PaneSource::EmptyShell => Self::EmptyShell,
            PaneSource::Command { title } => Self::Command { title: title.clone() },
            PaneSource::Manual => Self::Manual,
        }
    }
}

impl From<&recovery::PaneTarget> for PaneTarget {
    fn from(t: &recovery::PaneTarget) -> Self {
        match t {
            recovery::PaneTarget::LocalFolder { path } => Self::LocalFolder { path: path.clone() },
            recovery::PaneTarget::RemoteShell { ssh_target, remote_folder } => Self::RemoteShell {
                ssh_target: ssh_target.clone(),
                remote_folder: remote_folder.clone(),
            },
        }
    }
}

impl From<&PaneTarget> for recovery::PaneTarget {
    fn from(t: &PaneTarget) -> Self {
        match t {
            PaneTarget::LocalFolder { path } => Self::LocalFolder { path: path.clone() },
            PaneTarget::RemoteShell { ssh_target, remote_folder } => Self::RemoteShell {
                ssh_target: ssh_target.clone(),
                remote_folder: remote_folder.clone(),
            },
        }
    }
}

impl From<&recovery::StartupStep> for StartupStep {
    fn from(s: &recovery::StartupStep) -> Self {
        match s {
            recovery::StartupStep::SendText { text, execute } => {
                Self::SendText { text: text.clone(), execute: *execute }
            }
        }
    }
}

impl From<&StartupStep> for recovery::StartupStep {
    fn from(s: &StartupStep) -> Self {
        match s {
            StartupStep::SendText { text, execute } => {
                Self::SendText { text: text.clone(), execute: *execute }
            }
        }
    }
}

impl From<&recovery::PaneRecovery> for PaneRecoveryRecord {
    fn from(r: &recovery::PaneRecovery) -> Self {
        Self {
            source: (&r.source).into(),
            target: r.target.as_ref().map(Into::into),
            startup: r.startup.iter().map(Into::into).collect(),
        }
    }
}

impl From<&PaneRecoveryRecord> for recovery::PaneRecovery {
    fn from(r: &PaneRecoveryRecord) -> Self {
        Self {
            source: (&r.source).into(),
            target: r.target.as_ref().map(Into::into),
            startup: r.startup.iter().map(Into::into).collect(),
        }
    }
}

impl From<&crate::workspace::layout::SplitOrientation> for SplitOrientation {
    fn from(o: &crate::workspace::layout::SplitOrientation) -> Self {
        match o {
            crate::workspace::layout::SplitOrientation::Horizontal => Self::Horizontal,
            crate::workspace::layout::SplitOrientation::Vertical => Self::Vertical,
        }
    }
}

impl From<&SplitOrientation> for crate::workspace::layout::SplitOrientation {
    fn from(o: &SplitOrientation) -> Self {
        match o {
            SplitOrientation::Horizontal => Self::Horizontal,
            SplitOrientation::Vertical => Self::Vertical,
        }
    }
}

impl From<&crate::workspace::layout::LayoutNode> for LayoutNode {
    fn from(n: &crate::workspace::layout::LayoutNode) -> Self {
        match n {
            crate::workspace::layout::LayoutNode::Terminal { uuid, profile, cwd, custom_title } => {
                Self::Terminal {
                    uuid: uuid.clone(),
                    profile: profile.clone(),
                    cwd: cwd.clone(),
                    custom_title: custom_title.clone(),
                }
            }
            crate::workspace::layout::LayoutNode::Split { orientation, ratio, first, second } => {
                Self::Split {
                    orientation: orientation.into(),
                    ratio: *ratio,
                    first: Box::new(first.as_ref().into()),
                    second: Box::new(second.as_ref().into()),
                }
            }
        }
    }
}

impl From<&LayoutNode> for crate::workspace::layout::LayoutNode {
    fn from(n: &LayoutNode) -> Self {
        match n {
            LayoutNode::Terminal { uuid, profile, cwd, custom_title } => Self::Terminal {
                uuid: uuid.clone(),
                profile: profile.clone(),
                cwd: cwd.clone(),
                custom_title: custom_title.clone(),
            },
            LayoutNode::Split { orientation, ratio, first, second } => Self::Split {
                orientation: orientation.into(),
                ratio: *ratio,
                first: Box::new(first.as_ref().into()),
                second: Box::new(second.as_ref().into()),
            },
        }
    }
}

fn endpoint_key_from_runtime(runtime: &WorkspaceRuntime) -> String {
    match &runtime.endpoint {
        RuntimeEndpoint::Local => "local".into(),
        RuntimeEndpoint::Remote { host } => crate::host::normalize_ssh_key(host),
    }
}

fn runtime_ref_from_workspace(runtime: &WorkspaceRuntime) -> Option<RuntimeRef> {
    runtime.runtime_id.as_ref().map(|id| RuntimeRef {
        runtime_id: id.clone(),
        attachment_kind: RuntimeAttachmentKind::Created,
    })
}

impl From<&state::WorkspaceState> for WorkspaceRecord {
    fn from(ws: &state::WorkspaceState) -> Self {
        Self {
            id: ws.uuid.clone(),
            name: ws.name.clone(),
            user_renamed: ws.user_renamed,
            endpoint_key: if ws.runtime.is_managed() {
                endpoint_key_from_runtime(&ws.runtime)
            } else {
                "local".into()
            },
            policy: (&ws.runtime.policy).into(),
            runtime_ref: runtime_ref_from_workspace(&ws.runtime),
            layout: (&ws.layout).into(),
            active_pane_id: ws.active_terminal_uuid.clone(),
            zoomed_pane_id: ws.zoomed_terminal_uuid.clone(),
            input_sync: if ws.input_sync { InputSyncState::On } else { InputSyncState::Off },
            color: (&ws.color).into(),
            pane_recovery: ws
                .terminal_recovery
                .iter()
                .map(|(k, v)| (k.clone(), v.into()))
                .collect(),
        }
    }
}

impl WorkspaceRecord {
    /// Convert back to the domain `WorkspaceState`, reconstructing runtime metadata.
    #[must_use]
    pub fn to_workspace_state(&self) -> state::WorkspaceState {
        let endpoint = if self.endpoint_key == "local" {
            RuntimeEndpoint::Local
        } else {
            RuntimeEndpoint::Remote { host: self.endpoint_key.clone() }
        };

        let layout: crate::workspace::layout::LayoutNode = (&self.layout).into();
        let layout_terminal_uuids = layout.terminal_uuids();

        let managed = self.endpoint_key != "local" || self.runtime_ref.is_some();

        let mut runtime = WorkspaceRuntime {
            managed,
            endpoint,
            policy: (&self.policy).into(),
            runtime_id: self.runtime_ref.as_ref().map(|r| r.runtime_id.clone()),
            ..WorkspaceRuntime::default()
        };
        if managed {
            runtime.ensure_placeholder_bindings(&layout_terminal_uuids);
        }

        let mut ws = state::WorkspaceState {
            uuid: self.id.clone(),
            name: self.name.clone(),
            layout,
            terminal_recovery: self
                .pane_recovery
                .iter()
                .map(|(k, v)| (k.clone(), v.into()))
                .collect(),
            active_terminal_uuid: self.active_pane_id.clone(),
            input_sync: matches!(self.input_sync, InputSyncState::On),
            runtime,
            color: (&self.color).into(),
            zoomed_terminal_uuid: self.zoomed_pane_id.clone(),
            user_renamed: self.user_renamed,
        };
        ws.normalize_active_terminal();
        ws
    }
}
