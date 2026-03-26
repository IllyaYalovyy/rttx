//! Session management.
//!
//! A session is a named collection of panes with a layout tree. Sessions
//! persist across GUI disconnects and can be serialized to disk.

use crate::pane::{HistoryEntry, Pane, PersistedPane};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use uuid::Uuid;

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
                pane.scrollback_log_path = Some(pp.scrollback_log_path.clone());
                (pp.id, pane)
            })
            .collect();

        Self {
            id: persisted.id,
            name: persisted.name.clone(),
            active_pane_id: persisted.active_pane_id,
            command_history: persisted.command_history.clone(),
            created_at: persisted.created_at,
            last_active_at: persisted.last_active_at,
            attached_clients: Vec::new(),
            panes,
        }
    }

    /// Add a pane to this session.
    pub fn add_pane(&mut self, pane: Pane) {
        let id = pane.id;
        self.panes.insert(id, pane);
        if self.active_pane_id.is_none() {
            self.active_pane_id = Some(id);
        }
    }

    /// Remove a pane from this session.
    pub fn remove_pane(&mut self, pane_id: Uuid) -> Option<Pane> {
        let pane = self.panes.remove(&pane_id);
        if self.active_pane_id == Some(pane_id) {
            self.active_pane_id = self.panes.keys().next().copied();
        }
        pane
    }

    /// Attach a client to this session.
    pub fn attach_client(&mut self, client_id: Uuid) {
        if !self.attached_clients.contains(&client_id) {
            self.attached_clients.push(client_id);
        }
        self.last_active_at = SystemTime::now();
    }

    /// Detach a client from this session.
    pub fn detach_client(&mut self, client_id: Uuid) {
        self.attached_clients.retain(|id| *id != client_id);
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
    }

    #[test]
    fn add_pane_sets_active() {
        let mut session = Session::new("test".into());
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let pane_id = pane.id;
        session.add_pane(pane);
        assert_eq!(session.active_pane_id, Some(pane_id));
        assert_eq!(session.panes.len(), 1);
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
    }

    #[test]
    fn attach_detach_client() {
        let mut session = Session::new("test".into());
        let client = Uuid::new_v4();
        session.attach_client(client);
        assert!(session.has_attached_clients());
        session.detach_client(client);
        assert!(!session.has_attached_clients());
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
        assert!(restored.panes.contains_key(&pane_id));
        assert_eq!(restored.panes[&pane_id].cols, 120);
    }

    #[test]
    fn duplicate_attach_is_idempotent() {
        let mut session = Session::new("test".into());
        let client = Uuid::new_v4();
        session.attach_client(client);
        session.attach_client(client);
        assert_eq!(session.attached_clients.len(), 1);
    }
}
