//! V2 persisted structs with explicit `schema_version` (RFC-022 §2–§4).
//!
//! Every struct carries a top-level `schema_version: u32` field. Loading
//! dispatches on this field to select the correct deserialization path and
//! migration chain.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

use crate::pane_tree::{PaneId, WorkspaceTree};
use crate::workspace::WorkspacePolicy;

// ── Schema version constants ────────────────────────────────────────

/// Current schema version for [`DaemonIndexV1`].
pub const DAEMON_INDEX_SCHEMA_VERSION: u32 = 1;

/// Current schema version for [`WorkspaceFileV2`].
///
/// Version 2 (RFC-031 §6) persists the authoritative [`WorkspaceTree`] —
/// structure, logical split ratios, and default-active pane — as durable state.
/// Unsupported older-version files are detected, ignored,
/// and removed on load with no migration path.
pub const RUNTIME_FILE_SCHEMA_VERSION: u32 = 2;

/// Current schema version for [`ScreenSnapshotV1`].
pub const SCREEN_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

// ── Top-level daemon index ──────────────────────────────────────────

/// Server-level index listing all known workspace IDs (RFC-022 §1–§2).
///
/// Stored at `<state_dir>/daemon.json`. Rewritten only when the set of
/// workspace IDs changes, not on every serialization tick.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonIndexV1 {
    /// Must be [`DAEMON_INDEX_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Informational server version that last wrote this file.
    pub server_version: String,
    /// IDs of all workspaces managed by this daemon.
    pub runtime_ids: Vec<Uuid>,
    /// When this daemon index was first created.
    pub created_at: SystemTime,
    /// When this file was last written.
    pub last_serialized_at: SystemTime,
}

// ── Per-workspace file ──────────────────────────────────────────────

/// Top-level wrapper stored in `<runtime_dir>/workspace.json` (RFC-031 §6).
///
/// Combines the durable spec — including the authoritative pane tree — with
/// the semi-durable instance data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceFileV2 {
    /// Must be [`RUNTIME_FILE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Durable workspace specification.
    pub spec: WorkspaceSpecV2,
    /// Semi-durable instance data (bounded age, rebuilt on restart).
    pub instance: WorkspaceInstanceV1,
}

/// Durable workspace specification — identity, policy, pane tree, and panes.
///
/// These fields survive daemon restarts and are the source of truth for
/// workspace reconstruction. The [`WorkspaceTree`] carries structure, logical
/// split ratios, ordering, and the default-active pane (RFC-031 §2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSpecV2 {
    /// Unique workspace identifier.
    pub id: Uuid,
    /// Human-readable workspace name.
    pub name: String,
    /// Retention policy.
    pub policy: WorkspacePolicy,
    /// When this workspace was created.
    pub created_at: SystemTime,
    /// Authoritative pane-arrangement tree: structure, logical ratios, and the
    /// default-active pane.
    pub tree: WorkspaceTree,
    /// Per-pane specifications, keyed by the immutable [`PaneId`] carried in the
    /// tree.
    pub panes: Vec<PaneSpecV2>,
}

/// Semi-durable instance data — revision counters and timestamps.
///
/// Written alongside the spec but considered bounded-age: the daemon
/// rebuilds these on restart from the live process state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInstanceV1 {
    /// Monotonic mutation counter.
    pub revision: u64,
    /// When the workspace was last active (had attached clients or I/O).
    pub last_active_at: SystemTime,
    /// When screen snapshots were last flushed to disk.
    pub last_snapshot_at: SystemTime,
}

// ── Pane spec ───────────────────────────────────────────────────────

/// Durable pane specification (RFC-031 §6).
///
/// Captures the identity and last-known terminal state of a single pane.
/// Scrollback and screen data live in separate files keyed on the same
/// [`PaneId`]; this struct holds only the metadata needed to reconstruct the
/// pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneSpecV2 {
    /// Immutable pane identifier (RFC-031 G1).
    pub id: PaneId,
    /// Last known working directory.
    pub cwd: Option<String>,
    /// Pane title.
    pub title: Option<String>,
    /// Exit status if the pane's process exited.
    pub exit_status: Option<i32>,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// When true, scrollback and history are not flushed to disk (RFC-022 §9).
    #[serde(default)]
    pub no_persist: bool,
}

// ── Screen snapshot ─────────────────────────────────────────────────

/// Deterministic screen snapshot for a single pane (RFC-022 §4).
///
/// Stored at `<runtime_dir>/screen/<pane_id>.snap`. Consumed on
/// resurrection to restore the visible screen without replaying raw
/// scrollback bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenSnapshotV1 {
    /// Must be [`SCREEN_SNAPSHOT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Pane this snapshot belongs to.
    pub pane_id: Uuid,
    /// Terminal columns at snapshot time.
    pub cols: u16,
    /// Terminal rows at snapshot time.
    pub rows: u16,
    /// Cursor row (0-based).
    pub cursor_row: u16,
    /// Cursor column (0-based).
    pub cursor_col: u16,
    /// Whether the cursor was visible.
    pub cursor_visible: bool,
    /// Terminal title (OSC 2).
    pub title: Option<String>,
    /// Working directory (OSC 7).
    pub cwd: Option<String>,
    /// Monotonic output counter for delta ordering.
    pub pane_output_seq: u64,
    /// Terminal mode flags at snapshot time.
    pub modes: TerminalModeSnapshot,
    /// Raw bytes representing the visible screen content.
    ///
    /// Bounded by the visible viewport size, not the full scrollback.
    /// Future iterations may replace this with a cell-grid model.
    pub screen_bytes: Vec<u8>,
    /// When true, this snapshot belongs to a no-persist pane and must be
    /// excluded from export or sync operations (RFC-022 §9).
    #[serde(default)]
    pub confidential: bool,
}

/// Terminal mode flags captured at snapshot time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalModeSnapshot {
    /// Bracketed paste mode (DECSET 2004).
    pub bracketed_paste: bool,
    /// Application cursor keys mode (DECSET 1).
    pub application_cursor_keys: bool,
    /// Application keypad mode (DECKPAM).
    pub application_keypad: bool,
    /// Mouse tracking mode: 0=off, 1000/1002/1003.
    pub mouse_tracking_mode: u16,
    /// SGR mouse mode (DECSET 1006).
    pub sgr_mouse: bool,
    /// Focus event reporting (DECSET 1004).
    pub focus_reporting: bool,
    /// Alternate screen buffer (DECSET 1049/1047) active — a full-screen TUI
    /// (e.g. Claude, Codex, vim, htop) owned the terminal at snapshot time.
    /// Added after the initial schema; older snapshots default to `false`.
    #[serde(default)]
    pub alternate_screen: bool,
}

// ── Version envelope for dispatch ───────────────────────────────────

/// Minimal envelope used to peek at `schema_version` before full
/// deserialization. Allows the loader to select the correct struct
/// version and migration path.
#[derive(Debug, Deserialize)]
pub struct SchemaVersionEnvelope {
    /// The schema version declared in the file.
    pub schema_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_tree::SplitAxis;
    use std::time::SystemTime;

    fn sample_pane_spec() -> PaneSpecV2 {
        PaneSpecV2 {
            id: PaneId::new(),
            cwd: Some("/home/user".into()),
            title: Some("bash".into()),
            exit_status: None,
            cols: 80,
            rows: 24,
            no_persist: false,
        }
    }

    /// A two-pane tree whose panes match the returned specs.
    fn sample_tree_and_panes() -> (WorkspaceTree, Vec<PaneSpecV2>) {
        let a = sample_pane_spec();
        let mut b = sample_pane_spec();
        b.title = Some("nvim".into());
        let mut tree = WorkspaceTree::new();
        tree.insert_root(a.id);
        tree.split(a.id, b.id, SplitAxis::Vertical, 0.4);
        (tree, vec![a, b])
    }

    fn sample_workspace_spec() -> WorkspaceSpecV2 {
        let (tree, panes) = sample_tree_and_panes();
        WorkspaceSpecV2 {
            id: Uuid::new_v4(),
            name: "dev".into(),
            policy: WorkspacePolicy::Persistent,
            created_at: SystemTime::now(),
            tree,
            panes,
        }
    }

    fn sample_workspace_instance() -> WorkspaceInstanceV1 {
        WorkspaceInstanceV1 {
            revision: 42,
            last_active_at: SystemTime::now(),
            last_snapshot_at: SystemTime::now(),
        }
    }

    fn sample_daemon_index() -> DaemonIndexV1 {
        DaemonIndexV1 {
            schema_version: DAEMON_INDEX_SCHEMA_VERSION,
            server_version: "0.4.0".into(),
            runtime_ids: vec![Uuid::new_v4()],
            created_at: SystemTime::now(),
            last_serialized_at: SystemTime::now(),
        }
    }

    fn sample_runtime_file() -> WorkspaceFileV2 {
        WorkspaceFileV2 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: sample_workspace_spec(),
            instance: sample_workspace_instance(),
        }
    }

    fn sample_screen_snapshot() -> ScreenSnapshotV1 {
        ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id: Uuid::new_v4(),
            cols: 80,
            rows: 24,
            cursor_row: 10,
            cursor_col: 5,
            cursor_visible: true,
            title: Some("vim".into()),
            cwd: Some("/home/user/project".into()),
            pane_output_seq: 100,
            modes: TerminalModeSnapshot {
                bracketed_paste: true,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse: false,
                focus_reporting: false,
                alternate_screen: false,
            },
            screen_bytes: b"hello world\r\n".to_vec(),
            confidential: false,
        }
    }

    // ── Round-trip serialization ────────────────────────────────────

    #[test]
    fn daemon_index_round_trip() {
        let original = sample_daemon_index();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let recovered: DaemonIndexV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn runtime_file_round_trip() {
        let original = sample_runtime_file();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let recovered: WorkspaceFileV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn screen_snapshot_round_trip() {
        let original = sample_screen_snapshot();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let recovered: ScreenSnapshotV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn pane_spec_round_trip() {
        let original = sample_pane_spec();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let recovered: PaneSpecV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn workspace_spec_persists_tree_structure_and_ratios() {
        let original = sample_workspace_spec();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let recovered: WorkspaceSpecV2 = serde_json::from_str(&json).unwrap();
        // The durable tree — structure, ratios, and default-active — round-trips
        // byte-for-byte, not just the flat set of panes.
        assert_eq!(original.tree, recovered.tree);
        assert!(recovered.tree.validate().is_ok());
    }

    // ── Schema version field presence ───────────────────────────────

    #[test]
    fn daemon_index_json_contains_schema_version() {
        let index = sample_daemon_index();
        let json = serde_json::to_string(&index).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(raw["schema_version"], DAEMON_INDEX_SCHEMA_VERSION);
    }

    #[test]
    fn runtime_file_json_contains_schema_version() {
        let file = sample_runtime_file();
        let json = serde_json::to_string(&file).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(raw["schema_version"], RUNTIME_FILE_SCHEMA_VERSION);
    }

    #[test]
    fn screen_snapshot_json_contains_schema_version() {
        let snap = sample_screen_snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(raw["schema_version"], SCREEN_SNAPSHOT_SCHEMA_VERSION);
    }

    // ── Envelope dispatch ───────────────────────────────────────────

    #[test]
    fn schema_version_envelope_extracts_version() {
        let index = sample_daemon_index();
        let json = serde_json::to_string(&index).unwrap();
        let envelope: SchemaVersionEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope.schema_version, DAEMON_INDEX_SCHEMA_VERSION);
    }

    #[test]
    fn schema_version_envelope_works_for_runtime_file() {
        let file = sample_runtime_file();
        let json = serde_json::to_string(&file).unwrap();
        let envelope: SchemaVersionEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope.schema_version, RUNTIME_FILE_SCHEMA_VERSION);
    }

    #[test]
    fn schema_version_envelope_works_for_screen_snapshot() {
        let snap = sample_screen_snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let envelope: SchemaVersionEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope.schema_version, SCREEN_SNAPSHOT_SCHEMA_VERSION);
    }

    // ── Edge cases ──────────────────────────────────────────────────

    #[test]
    fn daemon_index_with_no_workspaces() {
        let index = DaemonIndexV1 {
            schema_version: DAEMON_INDEX_SCHEMA_VERSION,
            server_version: "0.4.0".into(),
            runtime_ids: vec![],
            created_at: SystemTime::now(),
            last_serialized_at: SystemTime::now(),
        };
        let json = serde_json::to_string_pretty(&index).unwrap();
        let recovered: DaemonIndexV1 = serde_json::from_str(&json).unwrap();
        assert!(recovered.runtime_ids.is_empty());
    }

    #[test]
    fn workspace_spec_with_no_panes() {
        let spec = WorkspaceSpecV2 {
            id: Uuid::new_v4(),
            name: "empty".into(),
            policy: WorkspacePolicy::Ephemeral,
            created_at: SystemTime::now(),
            tree: WorkspaceTree::new(),
            panes: vec![],
        };
        let json = serde_json::to_string_pretty(&spec).unwrap();
        let recovered: WorkspaceSpecV2 = serde_json::from_str(&json).unwrap();
        assert!(recovered.panes.is_empty());
        assert!(recovered.tree.is_empty());
        assert_eq!(recovered.policy, WorkspacePolicy::Ephemeral);
    }

    #[test]
    fn pane_spec_with_all_optional_fields_none() {
        let pane = PaneSpecV2 {
            id: PaneId::new(),
            cwd: None,
            title: None,
            exit_status: None,
            cols: 120,
            rows: 40,
            no_persist: false,
        };
        let json = serde_json::to_string_pretty(&pane).unwrap();
        let recovered: PaneSpecV2 = serde_json::from_str(&json).unwrap();
        assert!(recovered.cwd.is_none());
        assert!(recovered.title.is_none());
        assert!(recovered.exit_status.is_none());
    }

    #[test]
    fn screen_snapshot_with_empty_bytes() {
        let snap = ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id: Uuid::new_v4(),
            cols: 80,
            rows: 24,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            title: None,
            cwd: None,
            pane_output_seq: 0,
            modes: TerminalModeSnapshot {
                bracketed_paste: false,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse: false,
                focus_reporting: false,
                alternate_screen: false,
            },
            screen_bytes: vec![],
            confidential: false,
        };
        let json = serde_json::to_string_pretty(&snap).unwrap();
        let recovered: ScreenSnapshotV1 = serde_json::from_str(&json).unwrap();
        assert!(recovered.screen_bytes.is_empty());
    }

    #[test]
    fn screen_snapshot_all_modes_active() {
        let snap = ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id: Uuid::new_v4(),
            cols: 80,
            rows: 24,
            cursor_row: 23,
            cursor_col: 79,
            cursor_visible: false,
            title: Some("htop".into()),
            cwd: Some("/".into()),
            pane_output_seq: u64::MAX,
            modes: TerminalModeSnapshot {
                bracketed_paste: true,
                application_cursor_keys: true,
                application_keypad: true,
                mouse_tracking_mode: 1003,
                sgr_mouse: true,
                focus_reporting: true,
                alternate_screen: false,
            },
            screen_bytes: vec![0x1b, b'[', b'H'],
            confidential: false,
        };
        let json = serde_json::to_string_pretty(&snap).unwrap();
        let recovered: ScreenSnapshotV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(snap.modes, recovered.modes);
    }

    // ── Backward compatibility: no_persist / confidential default ────

    #[test]
    fn pane_spec_without_no_persist_field_defaults_to_false() {
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "cwd": "/tmp",
            "title": "bash",
            "exit_status": null,
            "cols": 80,
            "rows": 24
        }"#;
        let recovered: PaneSpecV2 = serde_json::from_str(json).unwrap();
        assert!(!recovered.no_persist);
    }

    #[test]
    fn pane_spec_with_no_persist_true_round_trips() {
        let mut pane = sample_pane_spec();
        pane.no_persist = true;
        let json = serde_json::to_string_pretty(&pane).unwrap();
        let recovered: PaneSpecV2 = serde_json::from_str(&json).unwrap();
        assert!(recovered.no_persist);
    }

    #[test]
    fn screen_snapshot_without_confidential_field_defaults_to_false() {
        let snap = sample_screen_snapshot();
        let mut val: serde_json::Value = serde_json::to_value(&snap).unwrap();
        val.as_object_mut().unwrap().remove("confidential");
        let recovered: ScreenSnapshotV1 = serde_json::from_value(val).unwrap();
        assert!(!recovered.confidential);
    }

    #[test]
    fn screen_snapshot_with_confidential_true_round_trips() {
        let mut snap = sample_screen_snapshot();
        snap.confidential = true;
        let json = serde_json::to_string_pretty(&snap).unwrap();
        let recovered: ScreenSnapshotV1 = serde_json::from_str(&json).unwrap();
        assert!(recovered.confidential);
    }

    #[test]
    fn workspace_spec_serialization_omits_removed_fields() {
        // The workspace spec has no standalone `active_pane_id`; it is
        // subsumed by the tree's default-active pane.
        let spec = sample_workspace_spec();
        let json = serde_json::to_string(&spec).unwrap();
        assert!(!json.contains("active_pane_id"), "active_pane_id must be gone");
        assert!(json.contains("tree"), "the durable tree must be serialized");
    }
}
