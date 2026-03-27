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
    /// Client IDs currently attached to this session.
    pub attached_clients: Vec<Uuid>,
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
            attached_clients: Vec::new(),
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
            attached_clients: Vec::new(),
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

    /// Attach a client to this session.
    pub fn attach_client(&mut self, client_id: Uuid) {
        if !self.attached_clients.contains(&client_id) {
            self.attached_clients.push(client_id);
            self.bump_revision();
        }
        self.last_active_at = SystemTime::now();
    }

    /// Detach a client from this session.
    pub fn detach_client(&mut self, client_id: Uuid) {
        let attached_before = self.attached_clients.len();
        self.attached_clients.retain(|id| *id != client_id);
        if self.attached_clients.len() != attached_before {
            self.bump_revision();
        }
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
    pub const fn has_attached_clients(&self) -> bool {
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
        session.attach_client(client);
        assert!(session.has_attached_clients());
        assert_eq!(session.revision(), 2);
        session.detach_client(client);
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
        session.attach_client(client);
        session.attach_client(client);
        assert_eq!(session.attached_clients.len(), 1);
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
}
