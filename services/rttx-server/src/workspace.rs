//! Workspace management.
//!
//! A workspace is a named collection of panes with a layout tree. Workspaces
//! persist across GUI disconnects and can be serialized to disk.

use crate::pane::Pane;
use crate::pane_tree::{CloseOutcome, PaneId, Side, SplitAxis, WorkspaceTree};
use rttx_proto::v3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use uuid::Uuid;

/// Workspace retention policy for a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePolicy {
    /// Keep the workspace alive across detach and reconnect.
    #[default]
    Persistent,
    /// Allow the workspace to be discarded when no clients remain attached.
    Ephemeral,
}

impl WorkspacePolicy {
    /// Convert from the v3 wire enum.
    #[must_use]
    pub fn from_v3_proto(value: i32) -> Self {
        match v3::WorkspacePolicy::try_from(value).ok() {
            Some(v3::WorkspacePolicy::Ephemeral) => Self::Ephemeral,
            _ => Self::Persistent,
        }
    }
}

/// Client role within a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRole {
    /// The default write owner for a workspace.
    Writer,
    /// A read-only attachment.
    Reader,
}

impl ClientRole {
    /// Convert to the v3 protocol enum value.
    #[must_use]
    pub const fn as_v3_proto(self) -> v3::WorkspaceClientRole {
        match self {
            Self::Writer => v3::WorkspaceClientRole::Writer,
            Self::Reader => v3::WorkspaceClientRole::Reader,
        }
    }
}

/// Requested attach mode from a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachMode {
    /// Request read-write ownership.
    ReadWrite,
    /// Request a read-only attachment.
    ReadOnly,
    /// Seize the write lease from the current owner, demoting it to reader.
    TakeOver,
}

impl AttachMode {
    /// Convert from the v3 wire enum.
    #[must_use]
    pub fn from_v3_proto(value: i32) -> Self {
        match v3::WorkspaceAttachMode::try_from(value).ok() {
            Some(v3::WorkspaceAttachMode::ReadOnly) => Self::ReadOnly,
            _ => Self::ReadWrite,
        }
    }
}

/// Reason a workspace was terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationReason {
    /// Explicit terminate request from a client.
    Explicit,
    /// Ephemeral workspace reached zero attached clients after a graceful detach.
    EphemeralLastDetach,
}

impl TerminationReason {
    /// Convert to the v3 wire enum.
    #[must_use]
    pub const fn as_v3_proto(self) -> v3::WorkspaceTerminationReason {
        match self {
            Self::Explicit => v3::WorkspaceTerminationReason::Explicit,
            Self::EphemeralLastDetach => v3::WorkspaceTerminationReason::EphemeralDetach,
        }
    }
}

/// Why a client attachment is being removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachReason {
    /// The client requested a graceful detach.
    ExplicitRequest,
    /// The client connection disappeared unexpectedly.
    Disconnect,
}

/// Result of attempting to attach a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    /// The client is now attached with the given role.
    Attached { role: ClientRole, revision: u64 },
    /// A conflicting writer already exists.
    Blocked { current_role: Option<ClientRole>, revision: u64 },
}

/// Errors that can occur while processing an attach request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    /// A takeover was requested for a workspace this client cannot seize.
    TakeoverNotEligible,
}

/// Result of detaching a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachOutcome {
    /// The client was detached and the workspace remains alive.
    Detached { revision: u64 },
    /// The client was not attached; no workspace state changed.
    NotAttached { revision: u64 },
    /// Graceful detach terminated an ephemeral workspace.
    Terminated { final_revision: u64, reason: TerminationReason },
}

const fn default_workspace_revision() -> u64 {
    1
}

/// Axis used when a pane is added without an explicit split request. Explicit
/// axis selection arrives with the protocol tree mutations (RFC-031 Step 3).
const DEFAULT_SPLIT_AXIS: SplitAxis = SplitAxis::Horizontal;
/// Even split for synthesized splits until an explicit ratio is provided.
const DEFAULT_SPLIT_RATIO: f32 = 0.5;

/// Workspace state of a single workspace.
pub struct Workspace {
    /// Unique workspace identifier.
    pub id: Uuid,
    /// Human-readable workspace name.
    pub name: String,
    /// Whether [`name`](Self::name) came from an explicit user rename rather
    /// than a name the client picked automatically.
    ///
    /// Persisted so a client that lost its own workspace store can tell a
    /// deliberate name from an auto-generated one when it rebuilds workspaces
    /// from the daemon inventory (issue #1084).
    pub user_renamed: bool,
    /// Panes in this workspace, keyed by pane ID.
    pub panes: HashMap<Uuid, Pane>,
    /// The currently focused pane.
    pub active_pane_id: Option<Uuid>,
    /// Authoritative pane-arrangement tree and default-active pane (RFC-031).
    ///
    /// The tree is the single source of truth for structure, split ratios, and
    /// ordering; `panes` holds the per-pane workspace state keyed by the same
    /// immutable ids.
    pub tree: WorkspaceTree,
    /// Workspace retention policy.
    pub policy: WorkspacePolicy,
    /// Whether this workspace was resurrected from persisted state.
    pub reconstructed: bool,
    /// Monotonic revision for meaningful workspace mutations.
    pub revision: u64,
    /// Revision at which this workspace was last successfully written to disk.
    /// When `revision > persisted_revision`, the workspace has unsaved changes.
    persisted_revision: u64,
    /// When this workspace was created.
    pub created_at: SystemTime,
    /// When this workspace was last active.
    pub last_active_at: SystemTime,
    /// Client roles currently attached to this workspace.
    pub attached_clients: HashMap<Uuid, ClientRole>,
}

impl Workspace {
    /// Create a new empty workspace.
    #[must_use]
    pub fn new(name: String) -> Self {
        let now = SystemTime::now();
        Self {
            id: Uuid::new_v4(),
            name,
            user_renamed: false,
            panes: HashMap::new(),
            active_pane_id: None,
            tree: WorkspaceTree::new(),
            policy: WorkspacePolicy::Persistent,
            reconstructed: false,
            revision: default_workspace_revision(),
            persisted_revision: 0,
            created_at: now,
            last_active_at: now,
            attached_clients: HashMap::new(),
        }
    }

    const fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    /// Return the current workspace revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether this workspace has unsaved changes since the last persist.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.revision > self.persisted_revision
    }

    /// Mark this workspace as successfully persisted at its current revision.
    pub const fn mark_persisted(&mut self) {
        self.persisted_revision = self.revision;
    }

    /// Add a pane to this workspace.
    ///
    /// The new pane is recorded in the authoritative tree: it seeds the root
    /// when the workspace is empty, otherwise it splits the active pane's leaf.
    pub fn add_pane(&mut self, pane: Pane) {
        let id = pane.id;
        self.panes.insert(id, pane);
        self.insert_pane_into_tree(id);
        if self.active_pane_id.is_none() {
            self.active_pane_id = Some(id);
        }
        self.bump_revision();
    }

    /// Place `id` in the tree without bumping the revision (callers do that).
    fn insert_pane_into_tree(&mut self, id: Uuid) {
        let new = PaneId::from_uuid(id);
        if self.tree.insert_root(new) {
            return;
        }
        let target = self
            .active_pane_id
            .map(PaneId::from_uuid)
            .filter(|t| self.tree.contains(*t))
            .or_else(|| self.tree.panes().first().copied());
        if let Some(target) = target {
            self.tree.split(target, new, DEFAULT_SPLIT_AXIS, DEFAULT_SPLIT_RATIO);
        }
    }

    /// Add `pane` to the workspace by splitting the leaf holding `target`.
    ///
    /// The new pane's identity is server-assigned and immutable (RFC-031 G1).
    /// Returns the new revision on success, or `None` (no change, pane not
    /// inserted) if `target` is absent, the ratio is invalid, or the new pane
    /// id already exists. The pane is added to the map only when the tree
    /// mutation succeeds, so structure and pane state never diverge.
    pub fn split_pane(
        &mut self,
        target: Uuid,
        pane: Pane,
        axis: SplitAxis,
        ratio: f32,
    ) -> Option<u64> {
        let new = PaneId::from_uuid(pane.id);
        if !self.tree.split(PaneId::from_uuid(target), new, axis, ratio) {
            return None;
        }
        if self.active_pane_id.is_none() {
            self.active_pane_id = Some(pane.id);
        }
        self.panes.insert(pane.id, pane);
        self.bump_revision();
        Some(self.revision())
    }

    /// Remove a pane from this workspace.
    ///
    /// The pane is dropped from the authoritative tree, collapsing its parent
    /// split into the sibling. When the removed pane held live focus, focus
    /// follows the tree's recomputed default-active so the two stay coherent.
    pub fn remove_pane(&mut self, pane_id: Uuid) -> Option<Pane> {
        let pane = self.panes.remove(&pane_id);
        if pane.is_some() {
            let outcome = self.tree.close(PaneId::from_uuid(pane_id));
            if self.active_pane_id == Some(pane_id) {
                self.active_pane_id = match outcome {
                    CloseOutcome::Removed { default_active } => Some(default_active.uuid()),
                    CloseOutcome::Emptied | CloseOutcome::NotFound => None,
                };
            }
            self.bump_revision();
        }
        pane
    }

    /// Set the logical ratio of the split addressed by `path`, returning the
    /// new revision on success.
    pub fn resize_split(&mut self, path: &[Side], ratio: f32) -> Option<u64> {
        if self.tree.resize_split(path, ratio) {
            self.bump_revision();
            Some(self.revision())
        } else {
            None
        }
    }

    /// Make `pane_id` the default-active pane (and live focus), returning the
    /// new revision on success, or `None` if the pane is not in the tree.
    pub fn set_default_active_pane(&mut self, pane_id: Uuid) -> Option<u64> {
        if self.tree.set_default_active(PaneId::from_uuid(pane_id)) {
            self.active_pane_id = Some(pane_id);
            self.bump_revision();
            Some(self.revision())
        } else {
            None
        }
    }

    /// The current role for a given client, if attached.
    #[must_use]
    pub fn client_role(&self, client_id: Uuid) -> Option<ClientRole> {
        self.attached_clients.get(&client_id).copied()
    }

    /// The current writer client, if any.
    #[must_use]
    pub fn writer_client_id(&self) -> Option<Uuid> {
        self.attached_clients
            .iter()
            .find_map(|(client_id, role)| (*role == ClientRole::Writer).then_some(*client_id))
    }

    /// Whether a write owner is attached.
    #[must_use]
    pub fn has_write_owner(&self) -> bool {
        self.writer_client_id().is_some()
    }

    /// Count read-only attachments.
    #[must_use]
    pub fn read_only_client_count(&self) -> usize {
        self.attached_clients.values().filter(|role| **role == ClientRole::Reader).count()
    }

    /// Count attached clients.
    #[must_use]
    pub fn attached_client_count(&self) -> usize {
        self.attached_clients.len()
    }

    /// Whether `client_id` may seize the write lease from the current owner.
    ///
    /// Only a persistent workspace can be taken over: an ephemeral workspace
    /// exists for as long as its clients stay attached, so seizing one would
    /// hand over a runtime that is already on its way out. A client that
    /// already holds the lease has nothing to seize, and a workspace with no
    /// write owner is reachable through an ordinary read-write attach.
    #[must_use]
    pub fn takeover_eligible_for(&self, client_id: Uuid) -> bool {
        self.policy == WorkspacePolicy::Persistent
            && self.writer_client_id().is_some_and(|writer| writer != client_id)
    }

    /// Whether the given client can mutate workspace state.
    #[must_use]
    pub fn client_has_write_access(&self, client_id: Uuid) -> bool {
        match self.client_role(client_id) {
            Some(ClientRole::Writer) => true,
            Some(ClientRole::Reader) => false,
            None => self.writer_client_id().is_none(),
        }
    }

    /// Attach a client to this workspace.
    pub fn attach_client(
        &mut self,
        client_id: Uuid,
        mode: AttachMode,
    ) -> Result<AttachOutcome, AttachError> {
        self.last_active_at = SystemTime::now();

        match mode {
            AttachMode::ReadOnly => {
                let changed = self.attached_clients.insert(client_id, ClientRole::Reader)
                    != Some(ClientRole::Reader);
                if changed {
                    self.bump_revision();
                }
                Ok(AttachOutcome::Attached { role: ClientRole::Reader, revision: self.revision() })
            }
            AttachMode::ReadWrite => {
                if let Some(writer_client_id) = self.writer_client_id()
                    && writer_client_id != client_id
                {
                    return Ok(AttachOutcome::Blocked {
                        current_role: self.client_role(client_id),
                        revision: self.revision(),
                    });
                }

                let changed = self.attached_clients.insert(client_id, ClientRole::Writer)
                    != Some(ClientRole::Writer);
                if changed {
                    self.bump_revision();
                }
                Ok(AttachOutcome::Attached { role: ClientRole::Writer, revision: self.revision() })
            }
            AttachMode::TakeOver => {
                if !self.takeover_eligible_for(client_id) {
                    return Err(AttachError::TakeoverNotEligible);
                }
                if let Some(previous_writer) = self.writer_client_id() {
                    self.attached_clients.insert(previous_writer, ClientRole::Reader);
                }
                self.attached_clients.insert(client_id, ClientRole::Writer);
                self.bump_revision();
                Ok(AttachOutcome::Attached { role: ClientRole::Writer, revision: self.revision() })
            }
        }
    }

    /// Detach a client from this workspace.
    pub fn detach_client(&mut self, client_id: Uuid, reason: DetachReason) -> DetachOutcome {
        let Some(_role) = self.attached_clients.remove(&client_id) else {
            return DetachOutcome::NotAttached { revision: self.revision() };
        };
        self.bump_revision();

        if matches!(reason, DetachReason::ExplicitRequest)
            && self.policy == WorkspacePolicy::Ephemeral
            && self.attached_clients.is_empty()
        {
            return DetachOutcome::Terminated {
                final_revision: self.revision(),
                reason: TerminationReason::EphemeralLastDetach,
            };
        }

        DetachOutcome::Detached { revision: self.revision() }
    }

    /// Rename this workspace and return the resulting revision.
    ///
    /// A rename always marks the workspace as user-renamed: the command only
    /// ever originates from an explicit user action.
    pub fn rename(&mut self, name: String) -> u64 {
        if self.name != name || !self.user_renamed {
            self.name = name;
            self.user_renamed = true;
            self.bump_revision();
        }
        self.revision()
    }

    /// Update a pane's size and return the resulting workspace revision.
    pub fn resize_pane(&mut self, pane_id: Uuid, cols: u16, rows: u16) -> Option<u64> {
        let changed = {
            let pane = self.panes.get_mut(&pane_id)?;
            let changed = pane.cols != cols || pane.rows != rows;
            pane.cols = cols;
            pane.rows = rows;
            changed
        };
        if changed {
            self.bump_revision();
        }
        Some(self.revision())
    }

    /// Update a pane's title and return the resulting workspace revision.
    pub fn set_pane_title(&mut self, pane_id: Uuid, title: String) -> Option<u64> {
        let changed = {
            let pane = self.panes.get_mut(&pane_id)?;
            let changed = pane.title.as_deref() != Some(title.as_str());
            pane.title = Some(title);
            changed
        };
        if changed {
            self.bump_revision();
        }
        Some(self.revision())
    }

    /// Update a pane's `no_persist` flag and return the resulting workspace revision.
    pub fn set_pane_no_persist(&mut self, pane_id: Uuid, no_persist: bool) -> Option<u64> {
        let pane = self.panes.get_mut(&pane_id)?;
        if pane.no_persist != no_persist {
            pane.no_persist = no_persist;
            self.bump_revision();
        }
        Some(self.revision())
    }

    /// Record a CWD change detected by `feed_output` and bump revision.
    pub fn set_pane_cwd(&mut self, pane_id: Uuid, _cwd: &str) -> Option<u64> {
        self.panes.get(&pane_id)?;
        self.bump_revision();
        Some(self.revision())
    }

    /// Return the effective CWD of any live pane in this workspace.
    /// Used as a fallback when `CreatePane` arrives without an explicit CWD.
    #[must_use]
    pub fn any_pane_cwd(&self) -> Option<String> {
        self.panes.values().find_map(Pane::effective_cwd)
    }

    /// Update a pane's exit status and return the resulting workspace revision.
    pub fn set_pane_exit_status(&mut self, pane_id: Uuid, status: Option<i32>) -> Option<u64> {
        let changed = {
            let pane = self.panes.get_mut(&pane_id)?;
            let changed = pane.exit_status != status;
            pane.exit_status = status;
            changed
        };
        if changed {
            self.bump_revision();
        }
        Some(self.revision())
    }

    /// Whether any client is attached.
    #[must_use]
    pub fn has_attached_clients(&self) -> bool {
        !self.attached_clients.is_empty()
    }

    /// Build a [`WorkspaceFileV2`] for per-workspace persistence (RFC-031 §6).
    #[must_use]
    pub fn to_workspace_file(&self) -> crate::state::types::WorkspaceFileV2 {
        use crate::state::types::{
            PaneSpecV2, RUNTIME_FILE_SCHEMA_VERSION, WorkspaceFileV2, WorkspaceInstanceV1,
            WorkspaceSpecV2,
        };

        let panes = self
            .panes
            .values()
            .map(|p| {
                let cwd = p.cwd.clone().or_else(|| p.read_proc_cwd());
                PaneSpecV2 {
                    id: PaneId::from_uuid(p.id),
                    cwd,
                    title: p.title.clone(),
                    exit_status: p.exit_status,
                    cols: p.cols,
                    rows: p.rows,
                    no_persist: p.no_persist,
                }
            })
            .collect();

        WorkspaceFileV2 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: WorkspaceSpecV2 {
                id: self.id,
                name: self.name.clone(),
                user_renamed: self.user_renamed,
                policy: self.policy,
                created_at: self.created_at,
                tree: self.tree.clone(),
                panes,
            },
            instance: WorkspaceInstanceV1 {
                revision: self.revision,
                last_active_at: self.last_active_at,
                last_snapshot_at: SystemTime::now(),
            },
        }
    }

    /// Resurrect a workspace from a [`WorkspaceFileV2`].
    ///
    /// The durable tree is restored verbatim; live focus follows the tree's
    /// persisted default-active pane.
    #[must_use]
    pub fn from_workspace_file(rf: &crate::state::types::WorkspaceFileV2) -> Self {
        let panes: HashMap<Uuid, Pane> = rf
            .spec
            .panes
            .iter()
            .map(|ps| {
                let id = ps.id.uuid();
                let mut pane = Pane::new(id, ps.cols, ps.rows);
                pane.cwd.clone_from(&ps.cwd);
                pane.title.clone_from(&ps.title);
                pane.exit_status = ps.exit_status;
                pane.reconstructed = true;
                pane.no_persist = ps.no_persist;
                (id, pane)
            })
            .collect();

        let active_pane_id = rf.spec.tree.default_active().map(PaneId::uuid);

        Self {
            id: rf.spec.id,
            name: rf.spec.name.clone(),
            user_renamed: rf.spec.user_renamed,
            active_pane_id,
            tree: rf.spec.tree.clone(),
            policy: rf.spec.policy,
            reconstructed: true,
            revision: rf.instance.revision.max(default_workspace_revision()),
            persisted_revision: rf.instance.revision.max(default_workspace_revision()),
            created_at: rf.spec.created_at,
            last_active_at: rf.instance.last_active_at,
            attached_clients: HashMap::new(),
            panes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_workspace_has_no_panes() {
        let workspace = Workspace::new("test".into());
        assert!(workspace.panes.is_empty());
        assert!(workspace.active_pane_id.is_none());
        assert_eq!(workspace.policy, WorkspacePolicy::Persistent);
        assert!(!workspace.reconstructed);
        assert_eq!(workspace.revision(), 1);
    }

    #[test]
    fn add_pane_sets_active() {
        let mut workspace = Workspace::new("test".into());
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let pane_id = pane.id;
        workspace.add_pane(pane);
        assert_eq!(workspace.active_pane_id, Some(pane_id));
        assert_eq!(workspace.panes.len(), 1);
        assert_eq!(workspace.revision(), 2);
    }

    #[test]
    fn remove_pane_updates_active() {
        let mut workspace = Workspace::new("test".into());
        let p1 = Pane::new(Uuid::new_v4(), 80, 24);
        let p2 = Pane::new(Uuid::new_v4(), 80, 24);
        let id1 = p1.id;
        let id2 = p2.id;
        workspace.add_pane(p1);
        workspace.add_pane(p2);
        workspace.active_pane_id = Some(id1);
        workspace.remove_pane(id1);
        assert_eq!(workspace.active_pane_id, Some(id2));
        assert_eq!(workspace.revision(), 4);
    }

    #[test]
    fn attach_detach_client() {
        let mut workspace = Workspace::new("test".into());
        let client = Uuid::new_v4();
        assert_eq!(
            workspace.attach_client(client, AttachMode::ReadWrite),
            Ok(AttachOutcome::Attached { role: ClientRole::Writer, revision: 2 })
        );
        assert!(workspace.has_attached_clients());
        assert_eq!(workspace.revision(), 2);
        assert_eq!(
            workspace.detach_client(client, DetachReason::ExplicitRequest),
            DetachOutcome::Detached { revision: 3 }
        );
        assert!(!workspace.has_attached_clients());
        assert_eq!(workspace.revision(), 3);
    }

    #[test]
    fn duplicate_attach_is_idempotent() {
        let mut workspace = Workspace::new("test".into());
        let client = Uuid::new_v4();
        let _ = workspace.attach_client(client, AttachMode::ReadWrite);
        let _ = workspace.attach_client(client, AttachMode::ReadWrite);
        assert_eq!(workspace.attached_client_count(), 1);
        assert_eq!(workspace.revision(), 2);
    }

    #[test]
    fn resize_title_and_exit_only_bump_revision_on_change() {
        let mut workspace = Workspace::new("test".into());
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let pane_id = pane.id;
        workspace.add_pane(pane);
        assert_eq!(workspace.revision(), 2);

        assert_eq!(workspace.resize_pane(pane_id, 80, 24), Some(2));
        assert_eq!(workspace.resize_pane(pane_id, 100, 30), Some(3));
        assert_eq!(workspace.set_pane_title(pane_id, "shell".into()), Some(4));
        assert_eq!(workspace.set_pane_title(pane_id, "shell".into()), Some(4));
        assert_eq!(workspace.set_pane_exit_status(pane_id, Some(7)), Some(5));
        assert_eq!(workspace.set_pane_exit_status(pane_id, Some(7)), Some(5));
        assert_eq!(workspace.set_pane_exit_status(pane_id, None), Some(6));
        assert_eq!(workspace.set_pane_cwd(pane_id, "/tmp"), Some(7));
        // The first rename bumps even for an identical name — it records that
        // the name is user-chosen (issue #1084) — the second identical one does not.
        assert_eq!(workspace.rename("test".into()), 8);
        assert_eq!(workspace.rename("test".into()), 8);
        assert_eq!(workspace.rename("renamed".into()), 9);
    }

    #[test]
    fn rename_updates_name() {
        let mut workspace = Workspace::new("original".into());
        assert_eq!(workspace.name, "original");

        workspace.rename("updated".into());
        assert_eq!(workspace.name, "updated");
    }

    #[test]
    fn read_only_attach_tracks_counts_and_role() {
        let mut workspace = Workspace::new("test".into());
        let reader = Uuid::new_v4();

        assert_eq!(
            workspace.attach_client(reader, AttachMode::ReadOnly),
            Ok(AttachOutcome::Attached { role: ClientRole::Reader, revision: 2 })
        );
        assert_eq!(workspace.client_role(reader), Some(ClientRole::Reader));
        assert_eq!(workspace.read_only_client_count(), 1);
        assert_eq!(workspace.attached_client_count(), 1);
        assert!(!workspace.client_has_write_access(reader));
    }

    #[test]
    fn second_writer_attach_is_blocked() {
        let mut workspace = Workspace::new("test".into());
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();

        let _ = workspace.attach_client(first, AttachMode::ReadWrite);
        assert_eq!(
            workspace.attach_client(second, AttachMode::ReadWrite),
            Ok(AttachOutcome::Blocked { current_role: None, revision: 2 })
        );
        assert_eq!(workspace.writer_client_id(), Some(first));
        assert_eq!(workspace.revision(), 2);
    }

    #[test]
    fn explicit_last_detach_terminates_ephemeral_workspace() {
        let mut workspace = Workspace::new("test".into());
        workspace.policy = WorkspacePolicy::Ephemeral;
        let client = Uuid::new_v4();
        let _ = workspace.attach_client(client, AttachMode::ReadWrite);

        assert_eq!(
            workspace.detach_client(client, DetachReason::ExplicitRequest),
            DetachOutcome::Terminated {
                final_revision: 3,
                reason: TerminationReason::EphemeralLastDetach,
            }
        );
    }

    #[test]
    fn disconnect_does_not_terminate_ephemeral_workspace() {
        let mut workspace = Workspace::new("test".into());
        workspace.policy = WorkspacePolicy::Ephemeral;
        let client = Uuid::new_v4();
        let _ = workspace.attach_client(client, AttachMode::ReadWrite);

        assert_eq!(
            workspace.detach_client(client, DetachReason::Disconnect),
            DetachOutcome::Detached { revision: 3 }
        );
        assert!(!workspace.has_attached_clients());
    }

    #[test]
    fn takeover_is_ineligible_without_a_write_owner() {
        let mut workspace = Workspace::new("test".into());
        let reader = Uuid::new_v4();
        let _ = workspace.attach_client(reader, AttachMode::ReadOnly);

        assert!(!workspace.takeover_eligible_for(Uuid::new_v4()));
    }

    #[test]
    fn takeover_is_eligible_for_a_client_that_is_not_the_write_owner() {
        let mut workspace = Workspace::new("test".into());
        let owner = Uuid::new_v4();
        let _ = workspace.attach_client(owner, AttachMode::ReadWrite);

        assert!(workspace.takeover_eligible_for(Uuid::new_v4()));
    }

    #[test]
    fn takeover_is_ineligible_for_the_current_write_owner() {
        let mut workspace = Workspace::new("test".into());
        let owner = Uuid::new_v4();
        let _ = workspace.attach_client(owner, AttachMode::ReadWrite);

        assert!(!workspace.takeover_eligible_for(owner));
    }

    #[test]
    fn takeover_is_ineligible_for_an_ephemeral_workspace() {
        let mut workspace = Workspace::new("test".into());
        workspace.policy = WorkspacePolicy::Ephemeral;
        let owner = Uuid::new_v4();
        let _ = workspace.attach_client(owner, AttachMode::ReadWrite);

        assert!(!workspace.takeover_eligible_for(Uuid::new_v4()));
    }

    #[test]
    fn takeover_demotes_the_previous_writer_to_reader() {
        let mut workspace = Workspace::new("test".into());
        let owner = Uuid::new_v4();
        let challenger = Uuid::new_v4();
        let _ = workspace.attach_client(owner, AttachMode::ReadWrite);
        let revision_before = workspace.revision();

        assert_eq!(
            workspace.attach_client(challenger, AttachMode::TakeOver),
            Ok(AttachOutcome::Attached { role: ClientRole::Writer, revision: revision_before + 1 })
        );
        assert_eq!(workspace.client_role(challenger), Some(ClientRole::Writer));
        assert_eq!(workspace.client_role(owner), Some(ClientRole::Reader));
        assert_eq!(workspace.writer_client_id(), Some(challenger));
        assert_eq!(workspace.read_only_client_count(), 1);
        assert!(workspace.client_has_write_access(challenger));
        assert!(!workspace.client_has_write_access(owner));
    }

    #[test]
    fn takeover_without_a_write_owner_is_rejected() {
        let mut workspace = Workspace::new("test".into());
        let client = Uuid::new_v4();

        assert_eq!(
            workspace.attach_client(client, AttachMode::TakeOver),
            Err(AttachError::TakeoverNotEligible)
        );
        assert_eq!(workspace.revision(), 1);
        assert!(!workspace.has_write_owner());
    }

    #[test]
    fn takeover_of_an_ephemeral_workspace_is_rejected() {
        let mut workspace = Workspace::new("test".into());
        workspace.policy = WorkspacePolicy::Ephemeral;
        let owner = Uuid::new_v4();
        let challenger = Uuid::new_v4();
        let _ = workspace.attach_client(owner, AttachMode::ReadWrite);

        assert_eq!(
            workspace.attach_client(challenger, AttachMode::TakeOver),
            Err(AttachError::TakeoverNotEligible)
        );
        assert_eq!(workspace.writer_client_id(), Some(owner));
    }

    #[test]
    fn runtime_file_v2_round_trip() {
        let mut workspace = Workspace::new("v2-test".into());
        workspace.policy = WorkspacePolicy::Persistent;
        let pane = Pane::new(Uuid::new_v4(), 100, 30);
        let pane_id = pane.id;
        workspace.add_pane(pane);

        let rf = workspace.to_workspace_file();
        assert_eq!(rf.spec.id, workspace.id);
        assert_eq!(rf.spec.name, "v2-test");
        assert_eq!(rf.spec.panes.len(), 1);
        assert_eq!(rf.spec.panes[0].id, PaneId::from_uuid(pane_id));
        assert_eq!(rf.spec.panes[0].cols, 100);
        assert_eq!(rf.instance.revision, workspace.revision());

        let restored = Workspace::from_workspace_file(&rf);
        assert_eq!(restored.id, workspace.id);
        assert_eq!(restored.name, "v2-test");
        assert_eq!(restored.policy, WorkspacePolicy::Persistent);
        assert!(restored.reconstructed);
        assert_eq!(restored.revision(), workspace.revision());
        assert!(restored.panes.contains_key(&pane_id));
        assert_eq!(restored.panes[&pane_id].cols, 100);
        assert_eq!(restored.panes[&pane_id].rows, 30);
    }

    // ── Dirty-flag (persisted_revision) tests ───────────────────

    #[test]
    fn new_workspace_is_dirty() {
        let workspace = Workspace::new("test".into());
        assert!(workspace.is_dirty(), "new workspace should be dirty (never persisted)");
    }

    #[test]
    fn mark_persisted_clears_dirty_flag() {
        let mut workspace = Workspace::new("test".into());
        assert!(workspace.is_dirty());
        workspace.mark_persisted();
        assert!(!workspace.is_dirty());
    }

    #[test]
    fn mutation_after_persist_makes_dirty_again() {
        let mut workspace = Workspace::new("test".into());
        workspace.mark_persisted();
        assert!(!workspace.is_dirty());

        workspace.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
        assert!(workspace.is_dirty(), "mutation should make workspace dirty again");
    }

    #[test]
    fn from_runtime_file_is_clean() {
        let mut workspace = Workspace::new("test".into());
        workspace.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
        let rf = workspace.to_workspace_file();
        let restored = Workspace::from_workspace_file(&rf);
        assert!(!restored.is_dirty(), "restored workspace should be clean");
    }

    #[test]
    fn multiple_mutations_stay_dirty_until_persisted() {
        let mut workspace = Workspace::new("test".into());
        workspace.mark_persisted();

        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let pane_id = pane.id;
        workspace.add_pane(pane);
        workspace.rename("renamed".into());
        workspace.set_pane_title(pane_id, "title".into());
        assert!(workspace.is_dirty());

        workspace.mark_persisted();
        assert!(!workspace.is_dirty());
    }

    #[test]
    fn idempotent_operations_do_not_dirty() {
        let mut workspace = Workspace::new("test".into());
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let pane_id = pane.id;
        workspace.add_pane(pane);
        workspace.set_pane_title(pane_id, "shell".into());
        // The first rename is a real change even to an identical name: it
        // records that the name is user-chosen (issue #1084).
        workspace.rename("test".into());
        workspace.mark_persisted();
        assert!(!workspace.is_dirty());

        // Same size, same title, same name — no revision bump.
        workspace.resize_pane(pane_id, 80, 24);
        workspace.set_pane_title(pane_id, "shell".into());
        workspace.rename("test".into());
        assert!(!workspace.is_dirty(), "no-op mutations should not dirty the workspace");
    }

    // ── User-rename marker (issue #1084) ────────────────────────

    #[test]
    fn new_workspace_is_not_user_renamed() {
        let workspace = Workspace::new("Workspace 1".into());
        assert!(
            !workspace.user_renamed,
            "a name chosen at creation time is not an explicit user rename"
        );
    }

    #[test]
    fn rename_marks_workspace_user_renamed() {
        let mut workspace = Workspace::new("Workspace 1".into());
        let rev_before = workspace.revision();

        let rev = workspace.rename("Payments".into());

        assert_eq!(workspace.name, "Payments");
        assert!(workspace.user_renamed, "an explicit rename must record user intent");
        assert!(rev > rev_before, "rename must bump the revision so the workspace persists");
    }

    #[test]
    fn rename_to_identical_name_still_records_user_intent() {
        let mut workspace = Workspace::new("Payments".into());
        let rev_before = workspace.revision();

        let rev = workspace.rename("Payments".into());

        assert!(workspace.user_renamed);
        assert!(
            rev > rev_before,
            "confirming the existing name is durable state and must be persisted"
        );
        assert!(workspace.is_dirty());
    }

    #[test]
    fn user_renamed_survives_workspace_file_roundtrip() {
        let mut workspace = Workspace::new("Workspace 1".into());
        workspace.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
        workspace.rename("Payments".into());

        let rf = workspace.to_workspace_file();
        assert_eq!(rf.spec.name, "Payments");
        assert!(rf.spec.user_renamed, "the marker belongs in the durable spec");

        let restored = Workspace::from_workspace_file(&rf);
        assert_eq!(restored.name, "Payments");
        assert!(restored.user_renamed, "a daemon restart must not forget the user rename");
    }

    #[test]
    fn auto_named_workspace_roundtrips_as_not_user_renamed() {
        let mut workspace = Workspace::new("Projects".into());
        workspace.add_pane(Pane::new(Uuid::new_v4(), 80, 24));

        let rf = workspace.to_workspace_file();
        assert!(!rf.spec.user_renamed);
        assert!(!Workspace::from_workspace_file(&rf).user_renamed);
    }

    #[test]
    fn set_pane_no_persist_toggles_flag_and_bumps_revision() {
        let mut workspace = Workspace::new("test".into());
        let pane_id = Uuid::new_v4();
        workspace.add_pane(Pane::new(pane_id, 80, 24));
        let rev_before = workspace.revision();

        let rev = workspace.set_pane_no_persist(pane_id, true).unwrap();
        assert!(rev > rev_before);
        assert!(workspace.panes[&pane_id].no_persist);

        // Setting same value is a no-op.
        let rev2 = workspace.set_pane_no_persist(pane_id, true).unwrap();
        assert_eq!(rev, rev2);
    }

    #[test]
    fn set_pane_no_persist_returns_none_for_missing_pane() {
        let mut workspace = Workspace::new("test".into());
        assert!(workspace.set_pane_no_persist(Uuid::new_v4(), true).is_none());
    }

    #[test]
    fn no_persist_pane_persisted_in_runtime_file() {
        let mut workspace = Workspace::new("test".into());
        let pane_id = Uuid::new_v4();
        let mut pane = Pane::new(pane_id, 80, 24);
        pane.no_persist = true;
        workspace.add_pane(pane);

        let rf = workspace.to_workspace_file();
        let pane_spec = &rf.spec.panes[0];
        assert!(pane_spec.no_persist);
    }

    #[test]
    fn no_persist_restored_from_runtime_file() {
        let mut workspace = Workspace::new("test".into());
        let pane_id = Uuid::new_v4();
        let mut pane = Pane::new(pane_id, 80, 24);
        pane.no_persist = true;
        workspace.add_pane(pane);

        let rf = workspace.to_workspace_file();
        let restored = Workspace::from_workspace_file(&rf);
        assert!(restored.panes[&pane_id].no_persist);
    }

    #[test]
    fn any_pane_cwd_returns_cwd_from_existing_pane() {
        let mut workspace = Workspace::new("test".into());
        assert!(workspace.any_pane_cwd().is_none());

        let pane_id = Uuid::new_v4();
        let mut pane = Pane::new(pane_id, 80, 24);
        pane.cwd = Some("/home/user/projects".into());
        workspace.add_pane(pane);

        assert_eq!(workspace.any_pane_cwd().as_deref(), Some("/home/user/projects"));
    }

    // ── Authoritative pane tree integration (RFC-031 Step 1) ────

    #[test]
    fn new_workspace_has_empty_tree() {
        let workspace = Workspace::new("test".into());
        assert!(workspace.tree.is_empty());
        assert_eq!(workspace.tree.default_active(), None);
        assert!(workspace.tree.validate().is_ok());
    }

    #[test]
    fn first_pane_seeds_tree_root_and_default_active() {
        let mut workspace = Workspace::new("test".into());
        let id = Uuid::new_v4();
        workspace.add_pane(Pane::new(id, 80, 24));
        assert_eq!(workspace.tree.leaf_count(), 1);
        assert_eq!(workspace.tree.default_active(), Some(PaneId::from_uuid(id)));
        assert!(workspace.tree.contains(PaneId::from_uuid(id)));
        assert!(workspace.tree.validate().is_ok());
    }

    #[test]
    fn second_pane_splits_active_leaf_in_tree() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        assert_eq!(workspace.tree.leaf_count(), 2);
        assert!(workspace.tree.contains(PaneId::from_uuid(a)));
        assert!(workspace.tree.contains(PaneId::from_uuid(b)));
        // default-active stays on the first pane after a split.
        assert_eq!(workspace.tree.default_active(), Some(PaneId::from_uuid(a)));
        assert!(workspace.tree.validate().is_ok());
    }

    #[test]
    fn pane_id_is_stable_across_tree_growth() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        for _ in 0..5 {
            workspace.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
            // The original pane's id never changes as the tree grows.
            assert!(workspace.tree.contains(PaneId::from_uuid(a)));
            assert!(workspace.panes.contains_key(&a));
        }
    }

    #[test]
    fn remove_pane_collapses_tree() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        workspace.remove_pane(a);
        assert_eq!(workspace.tree.leaf_count(), 1);
        assert!(!workspace.tree.contains(PaneId::from_uuid(a)));
        assert!(workspace.tree.contains(PaneId::from_uuid(b)));
        assert!(workspace.tree.validate().is_ok());
    }

    #[test]
    fn closing_active_pane_moves_focus_to_tree_default_active() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        workspace.active_pane_id = Some(a);
        workspace.remove_pane(a);
        // Live focus follows the tree's recomputed default-active, not an
        // arbitrary HashMap entry.
        assert_eq!(workspace.active_pane_id, Some(b));
        assert_eq!(workspace.tree.default_active(), Some(PaneId::from_uuid(b)));
    }

    #[test]
    fn closing_inactive_pane_leaves_focus_untouched() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        workspace.active_pane_id = Some(a);
        workspace.remove_pane(b);
        assert_eq!(workspace.active_pane_id, Some(a));
    }

    #[test]
    fn removing_last_pane_empties_tree() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.remove_pane(a);
        assert!(workspace.tree.is_empty());
        assert!(workspace.tree.validate().is_ok());
    }

    #[test]
    fn resize_split_updates_tree_ratio_and_revision() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        let before = workspace.revision();
        assert_eq!(workspace.resize_split(&[], 0.3), Some(before + 1));
        // Invalid ratio is rejected without bumping the revision.
        assert_eq!(workspace.resize_split(&[], 1.0), None);
        assert_eq!(workspace.revision(), before + 1);
    }

    #[test]
    fn split_pane_targets_explicit_leaf_with_axis_and_ratio() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        let before = workspace.revision();

        let rev = workspace.split_pane(a, Pane::new(b, 80, 24), SplitAxis::Vertical, 0.3);
        assert_eq!(rev, Some(before + 1));
        assert_eq!(workspace.tree.leaf_count(), 2);
        assert!(workspace.panes.contains_key(&b));
        // The target keeps its identity and stays default-active after a split.
        assert!(workspace.tree.contains(PaneId::from_uuid(a)));
        assert_eq!(workspace.tree.default_active(), Some(PaneId::from_uuid(a)));
        assert!(workspace.tree.validate().is_ok());
    }

    #[test]
    fn split_pane_rejects_unknown_target_without_inserting_pane() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        let before = workspace.revision();

        let orphan = Uuid::new_v4();
        // Target is not in the tree: the split is rejected and the candidate
        // pane is never added to the map (structure/state never diverge).
        assert_eq!(
            workspace.split_pane(
                Uuid::new_v4(),
                Pane::new(orphan, 80, 24),
                SplitAxis::Horizontal,
                0.5
            ),
            None
        );
        assert!(!workspace.panes.contains_key(&orphan));
        assert_eq!(workspace.tree.leaf_count(), 1);
        assert_eq!(workspace.revision(), before);
    }

    #[test]
    fn split_pane_rejects_invalid_ratio() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        let b = Uuid::new_v4();
        assert_eq!(workspace.split_pane(a, Pane::new(b, 80, 24), SplitAxis::Horizontal, 1.5), None);
        assert!(!workspace.panes.contains_key(&b));
    }

    #[test]
    fn set_default_active_pane_updates_tree_and_focus() {
        let mut workspace = Workspace::new("test".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        assert!(workspace.set_default_active_pane(b).is_some());
        assert_eq!(workspace.tree.default_active(), Some(PaneId::from_uuid(b)));
        assert_eq!(workspace.active_pane_id, Some(b));
        // Unknown pane is rejected.
        assert!(workspace.set_default_active_pane(Uuid::new_v4()).is_none());
    }

    #[test]
    fn reconstructed_workspace_rebuilds_tree_from_panes() {
        let mut workspace = Workspace::new("rebuild".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        workspace.add_pane(Pane::new(c, 80, 24));
        workspace.set_default_active_pane(b);

        let rf = workspace.to_workspace_file();
        let restored = Workspace::from_workspace_file(&rf);
        assert_eq!(restored.tree.leaf_count(), 3);
        for id in [a, b, c] {
            assert!(restored.tree.contains(PaneId::from_uuid(id)));
        }
        assert_eq!(restored.tree.default_active(), Some(PaneId::from_uuid(b)));
        assert!(restored.tree.validate().is_ok());
    }

    #[test]
    fn durable_tree_round_trips_exact_structure_and_ratios() {
        // Build an asymmetric tree with custom split ratios and a non-default
        // active pane, then prove the persisted tree is restored verbatim —
        // not re-synthesized from a flat pane list (which would lose ratios and
        // structure).
        let mut workspace = Workspace::new("durable".into());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        workspace.add_pane(Pane::new(a, 80, 24));
        workspace.add_pane(Pane::new(b, 80, 24));
        workspace.add_pane(Pane::new(c, 80, 24));
        // Each new pane splits the active leaf (a), so the tree is
        // Split(Split(a, c), b). Give both splits distinct ratios.
        assert_eq!(workspace.resize_split(&[], 0.25), Some(workspace.revision()));
        assert_eq!(workspace.resize_split(&[Side::First], 0.8), Some(workspace.revision()));
        workspace.set_default_active_pane(c);

        let tree_before = workspace.tree.clone();

        let rf = workspace.to_workspace_file();
        let restored = Workspace::from_workspace_file(&rf);

        assert_eq!(restored.tree, tree_before, "durable tree must round-trip exactly");
        assert_eq!(restored.tree.default_active(), Some(PaneId::from_uuid(c)));
        assert_eq!(restored.active_pane_id, Some(c));
        assert!(restored.tree.validate().is_ok());
    }
}
