//! Session management.
//!
//! A session is a named collection of panes with a layout tree. Sessions
//! persist across GUI disconnects and can be serialized to disk.

use crate::pane::{HistoryEntry, Pane, PersistedPane};
use rttx_proto::proto;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use uuid::Uuid;

/// Runtime retention policy for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePolicy {
    /// Keep the runtime alive across detach and reconnect.
    #[default]
    Persistent,
    /// Allow the runtime to be discarded when no clients remain attached.
    Ephemeral,
}

impl RuntimePolicy {
    /// Convert to the protocol enum value used on the wire.
    #[must_use]
    pub const fn as_proto(self) -> proto::RuntimePolicy {
        match self {
            Self::Persistent => proto::RuntimePolicy::Persistent,
            Self::Ephemeral => proto::RuntimePolicy::Ephemeral,
        }
    }

    /// Convert from the wire enum, defaulting to `Persistent` for legacy/unknown values.
    #[must_use]
    pub fn from_proto(value: i32) -> Self {
        match proto::RuntimePolicy::try_from(value).ok() {
            Some(proto::RuntimePolicy::Ephemeral) => Self::Ephemeral,
            _ => Self::Persistent,
        }
    }
}

/// Client role within a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRole {
    /// The default write owner for a runtime.
    Writer,
    /// A read-only attachment.
    Reader,
}

impl ClientRole {
    /// Convert to the protocol enum value used on the wire.
    #[must_use]
    pub const fn as_proto(self) -> proto::RuntimeClientRole {
        match self {
            Self::Writer => proto::RuntimeClientRole::Writer,
            Self::Reader => proto::RuntimeClientRole::Reader,
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
    /// Reserve room for a future explicit takeover flow.
    TakeOver,
}

impl AttachMode {
    /// Convert from the wire enum, defaulting to `ReadWrite` for compatibility.
    #[must_use]
    pub fn from_proto(value: i32) -> Self {
        match proto::RuntimeAttachMode::try_from(value).ok() {
            Some(proto::RuntimeAttachMode::ReadOnly) => Self::ReadOnly,
            Some(proto::RuntimeAttachMode::TakeOver) => Self::TakeOver,
            _ => Self::ReadWrite,
        }
    }
}

/// Reason a runtime was terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationReason {
    /// Explicit terminate request from a client.
    Explicit,
    /// Ephemeral runtime reached zero attached clients after a graceful detach.
    EphemeralLastDetach,
}

impl TerminationReason {
    /// Convert to the wire enum.
    #[must_use]
    pub const fn as_proto(self) -> proto::RuntimeTerminationReason {
        match self {
            Self::Explicit => proto::RuntimeTerminationReason::Explicit,
            Self::EphemeralLastDetach => proto::RuntimeTerminationReason::EphemeralLastDetach,
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
    /// The future takeover mode is not implemented yet.
    UnsupportedTakeOver,
}

/// Result of detaching a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachOutcome {
    /// The client was detached and the runtime remains alive.
    Detached { revision: u64 },
    /// The client was not attached; no runtime state changed.
    NotAttached { revision: u64 },
    /// Graceful detach terminated an ephemeral runtime.
    Terminated { final_revision: u64, reason: TerminationReason },
}

const fn default_session_revision() -> u64 {
    1
}

/// Runtime state of a single session.
pub struct Session {
    /// Unique session identifier.
    pub id: Uuid,
    /// Human-readable session name.
    pub name: String,
    /// Panes in this session, keyed by pane ID.
    pub panes: HashMap<Uuid, Pane>,
    /// The currently focused pane.
    pub active_pane_id: Option<Uuid>,
    /// Per-session command history.
    pub command_history: Vec<HistoryEntry>,
    /// Runtime retention policy.
    pub policy: RuntimePolicy,
    /// Whether this session was resurrected from persisted state.
    pub reconstructed: bool,
    /// Monotonic revision for meaningful runtime mutations.
    pub revision: u64,
    /// When this session was created.
    pub created_at: SystemTime,
    /// When this session was last active.
    pub last_active_at: SystemTime,
    /// Client roles currently attached to this session.
    pub attached_clients: HashMap<Uuid, ClientRole>,
}

impl Session {
    /// Create a new empty session.
    #[must_use]
    pub fn new(name: String) -> Self {
        let now = SystemTime::now();
        Self {
            id: Uuid::new_v4(),
            name,
            panes: HashMap::new(),
            active_pane_id: None,
            command_history: Vec::new(),
            policy: RuntimePolicy::Persistent,
            reconstructed: false,
            revision: default_session_revision(),
            created_at: now,
            last_active_at: now,
            attached_clients: HashMap::new(),
        }
    }

    /// Create a session from persisted state (resurrection).
    #[must_use]
    pub fn from_persisted(persisted: &PersistedSession) -> Self {
        let panes: HashMap<Uuid, Pane> = persisted
            .panes
            .iter()
            .map(|pp| {
                let mut pane = Pane::new(pp.id, pp.cols, pp.rows);
                pane.cwd.clone_from(&pp.cwd);
                pane.title.clone_from(&pp.title);
                pane.exit_status = pp.exit_status;
                pane.reconstructed = true;
                pane.scrollback_log_path = Some(pp.scrollback_log_path.clone());
                (pp.id, pane)
            })
            .collect();

        Self {
            id: persisted.id,
            name: persisted.name.clone(),
            active_pane_id: persisted.active_pane_id,
            command_history: persisted.command_history.clone(),
            policy: persisted.policy,
            reconstructed: true,
            revision: persisted.revision.max(default_session_revision()),
            created_at: persisted.created_at,
            last_active_at: persisted.last_active_at,
            attached_clients: HashMap::new(),
            panes,
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    /// Return the current runtime revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Add a pane to this session.
    pub fn add_pane(&mut self, pane: Pane) {
        let id = pane.id;
        self.panes.insert(id, pane);
        if self.active_pane_id.is_none() {
            self.active_pane_id = Some(id);
        }
        self.bump_revision();
    }

    /// Remove a pane from this session.
    pub fn remove_pane(&mut self, pane_id: Uuid) -> Option<Pane> {
        let pane = self.panes.remove(&pane_id);
        if pane.is_some() {
            if self.active_pane_id == Some(pane_id) {
                self.active_pane_id = self.panes.keys().next().copied();
            }
            self.bump_revision();
        }
        pane
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

    /// Whether the given client can mutate runtime state.
    #[must_use]
    pub fn client_has_write_access(&self, client_id: Uuid) -> bool {
        match self.client_role(client_id) {
            Some(ClientRole::Writer) => true,
            Some(ClientRole::Reader) => false,
            None => self.writer_client_id().is_none(),
        }
    }

    /// Attach a client to this session.
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
            AttachMode::TakeOver => Err(AttachError::UnsupportedTakeOver),
        }
    }

    /// Detach a client from this session.
    pub fn detach_client(&mut self, client_id: Uuid, reason: DetachReason) -> DetachOutcome {
        let Some(_role) = self.attached_clients.remove(&client_id) else {
            return DetachOutcome::NotAttached { revision: self.revision() };
        };
        self.bump_revision();

        if matches!(reason, DetachReason::ExplicitRequest)
            && self.policy == RuntimePolicy::Ephemeral
            && self.attached_clients.is_empty()
        {
            return DetachOutcome::Terminated {
                final_revision: self.revision(),
                reason: TerminationReason::EphemeralLastDetach,
            };
        }

        DetachOutcome::Detached { revision: self.revision() }
    }

    /// Update a pane's size and return the resulting session revision.
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

    /// Update a pane's title and return the resulting session revision.
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

    /// Update a pane's exit status and return the resulting session revision.
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

    /// Build a persistable snapshot of this session.
    #[must_use]
    pub fn to_persisted(&self) -> PersistedSession {
        PersistedSession {
            id: self.id,
            name: self.name.clone(),
            panes: self.panes.values().map(Pane::to_persisted).collect(),
            active_pane_id: self.active_pane_id,
            command_history: self.command_history.clone(),
            policy: self.policy,
            revision: self.revision,
            created_at: self.created_at,
            last_active_at: self.last_active_at,
        }
    }
}

/// Serializable session state for disk persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    /// Unique session identifier.
    pub id: Uuid,
    /// Session name.
    pub name: String,
    /// Persisted pane states.
    pub panes: Vec<PersistedPane>,
    /// Active pane ID.
    pub active_pane_id: Option<Uuid>,
    /// Per-session command history.
    pub command_history: Vec<HistoryEntry>,
    /// Runtime retention policy.
    #[serde(default)]
    pub policy: RuntimePolicy,
    /// Monotonic runtime revision.
    #[serde(default = "default_session_revision")]
    pub revision: u64,
    /// When the session was created.
    pub created_at: SystemTime,
    /// When the session was last active.
    pub last_active_at: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_has_no_panes() {
        let session = Session::new("test".into());
        assert!(session.panes.is_empty());
        assert!(session.active_pane_id.is_none());
        assert_eq!(session.policy, RuntimePolicy::Persistent);
        assert!(!session.reconstructed);
        assert_eq!(session.revision(), 1);
    }

    #[test]
    fn add_pane_sets_active() {
        let mut session = Session::new("test".into());
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let pane_id = pane.id;
        session.add_pane(pane);
        assert_eq!(session.active_pane_id, Some(pane_id));
        assert_eq!(session.panes.len(), 1);
        assert_eq!(session.revision(), 2);
    }

    #[test]
    fn remove_pane_updates_active() {
        let mut session = Session::new("test".into());
        let p1 = Pane::new(Uuid::new_v4(), 80, 24);
        let p2 = Pane::new(Uuid::new_v4(), 80, 24);
        let id1 = p1.id;
        let id2 = p2.id;
        session.add_pane(p1);
        session.add_pane(p2);
        session.active_pane_id = Some(id1);
        session.remove_pane(id1);
        assert_eq!(session.active_pane_id, Some(id2));
        assert_eq!(session.revision(), 4);
    }

    #[test]
    fn attach_detach_client() {
        let mut session = Session::new("test".into());
        let client = Uuid::new_v4();
        assert_eq!(
            session.attach_client(client, AttachMode::ReadWrite),
            Ok(AttachOutcome::Attached { role: ClientRole::Writer, revision: 2 })
        );
        assert!(session.has_attached_clients());
        assert_eq!(session.revision(), 2);
        assert_eq!(
            session.detach_client(client, DetachReason::ExplicitRequest),
            DetachOutcome::Detached { revision: 3 }
        );
        assert!(!session.has_attached_clients());
        assert_eq!(session.revision(), 3);
    }

    #[test]
    fn persisted_roundtrip() {
        let mut session = Session::new("test".into());
        session.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
        let persisted = session.to_persisted();
        let json = serde_json::to_string(&persisted).unwrap();
        let recovered: PersistedSession = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.id, session.id);
        assert_eq!(recovered.name, "test");
        assert_eq!(recovered.panes.len(), 1);
    }

    #[test]
    fn from_persisted_restores_panes() {
        let mut session = Session::new("test".into());
        let pane = Pane::new(Uuid::new_v4(), 120, 40);
        let pane_id = pane.id;
        session.add_pane(pane);
        let persisted = session.to_persisted();

        let restored = Session::from_persisted(&persisted);
        assert_eq!(restored.id, session.id);
        assert_eq!(restored.policy, RuntimePolicy::Persistent);
        assert!(restored.reconstructed);
        assert_eq!(restored.revision(), session.revision());
        assert!(restored.panes.contains_key(&pane_id));
        assert_eq!(restored.panes[&pane_id].cols, 120);
        assert!(restored.panes[&pane_id].reconstructed);
    }

    #[test]
    fn duplicate_attach_is_idempotent() {
        let mut session = Session::new("test".into());
        let client = Uuid::new_v4();
        let _ = session.attach_client(client, AttachMode::ReadWrite);
        let _ = session.attach_client(client, AttachMode::ReadWrite);
        assert_eq!(session.attached_client_count(), 1);
        assert_eq!(session.revision(), 2);
    }

    #[test]
    fn persisted_policy_roundtrip() {
        let mut session = Session::new("test".into());
        session.policy = RuntimePolicy::Ephemeral;
        let persisted = session.to_persisted();
        let json = serde_json::to_string(&persisted).unwrap();
        let recovered: PersistedSession = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.policy, RuntimePolicy::Ephemeral);
    }

    #[test]
    fn persisted_session_defaults_policy_for_legacy_state() {
        let persisted: PersistedSession = serde_json::from_str(
            r#"{
                "id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "name":"legacy",
                "panes":[],
                "active_pane_id":null,
                "command_history":[],
                "created_at":{"secs_since_epoch":1,"nanos_since_epoch":0},
                "last_active_at":{"secs_since_epoch":2,"nanos_since_epoch":0}
            }"#,
        )
        .unwrap();

        assert_eq!(persisted.policy, RuntimePolicy::Persistent);
        assert_eq!(persisted.revision, 1);
    }

    #[test]
    fn resize_title_and_exit_only_bump_revision_on_change() {
        let mut session = Session::new("test".into());
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let pane_id = pane.id;
        session.add_pane(pane);
        assert_eq!(session.revision(), 2);

        assert_eq!(session.resize_pane(pane_id, 80, 24), Some(2));
        assert_eq!(session.resize_pane(pane_id, 100, 30), Some(3));
        assert_eq!(session.set_pane_title(pane_id, "shell".into()), Some(4));
        assert_eq!(session.set_pane_title(pane_id, "shell".into()), Some(4));
        assert_eq!(session.set_pane_exit_status(pane_id, Some(7)), Some(5));
        assert_eq!(session.set_pane_exit_status(pane_id, Some(7)), Some(5));
        assert_eq!(session.set_pane_exit_status(pane_id, None), Some(6));
    }

    #[test]
    fn persisted_revision_roundtrip() {
        let mut session = Session::new("test".into());
        session.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
        let persisted = session.to_persisted();
        let json = serde_json::to_string(&persisted).unwrap();
        let recovered: PersistedSession = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.revision, session.revision());
    }

    #[test]
    fn read_only_attach_tracks_counts_and_role() {
        let mut session = Session::new("test".into());
        let reader = Uuid::new_v4();

        assert_eq!(
            session.attach_client(reader, AttachMode::ReadOnly),
            Ok(AttachOutcome::Attached { role: ClientRole::Reader, revision: 2 })
        );
        assert_eq!(session.client_role(reader), Some(ClientRole::Reader));
        assert_eq!(session.read_only_client_count(), 1);
        assert_eq!(session.attached_client_count(), 1);
        assert!(!session.client_has_write_access(reader));
    }

    #[test]
    fn second_writer_attach_is_blocked() {
        let mut session = Session::new("test".into());
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();

        let _ = session.attach_client(first, AttachMode::ReadWrite);
        assert_eq!(
            session.attach_client(second, AttachMode::ReadWrite),
            Ok(AttachOutcome::Blocked { current_role: None, revision: 2 })
        );
        assert_eq!(session.writer_client_id(), Some(first));
        assert_eq!(session.revision(), 2);
    }

    #[test]
    fn explicit_last_detach_terminates_ephemeral_runtime() {
        let mut session = Session::new("test".into());
        session.policy = RuntimePolicy::Ephemeral;
        let client = Uuid::new_v4();
        let _ = session.attach_client(client, AttachMode::ReadWrite);

        assert_eq!(
            session.detach_client(client, DetachReason::ExplicitRequest),
            DetachOutcome::Terminated {
                final_revision: 3,
                reason: TerminationReason::EphemeralLastDetach,
            }
        );
    }

    #[test]
    fn disconnect_does_not_terminate_ephemeral_runtime() {
        let mut session = Session::new("test".into());
        session.policy = RuntimePolicy::Ephemeral;
        let client = Uuid::new_v4();
        let _ = session.attach_client(client, AttachMode::ReadWrite);

        assert_eq!(
            session.detach_client(client, DetachReason::Disconnect),
            DetachOutcome::Detached { revision: 3 }
        );
        assert!(!session.has_attached_clients());
    }

    #[test]
    fn take_over_attach_is_reserved_for_future_work() {
        let mut session = Session::new("test".into());
        let client = Uuid::new_v4();
        assert_eq!(
            session.attach_client(client, AttachMode::TakeOver),
            Err(AttachError::UnsupportedTakeOver)
        );
        assert_eq!(session.revision(), 1);
    }
}
