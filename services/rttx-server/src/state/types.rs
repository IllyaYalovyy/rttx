//! V2 persisted structs with explicit `schema_version` (RFC-022 §2–§4).
//!
//! Every struct carries a top-level `schema_version: u32` field. Loading
//! dispatches on this field to select the correct deserialization path and
//! migration chain.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

use crate::runtime::RuntimePolicy;

// ── Schema version constants ────────────────────────────────────────

/// Current schema version for [`DaemonIndexV1`].
pub const DAEMON_INDEX_SCHEMA_VERSION: u32 = 1;

/// Current schema version for [`RuntimeFileV1`].
pub const RUNTIME_FILE_SCHEMA_VERSION: u32 = 1;

/// Current schema version for [`ScreenSnapshotV1`].
pub const SCREEN_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

// ── Top-level daemon index ──────────────────────────────────────────

/// Server-level index listing all known runtime IDs (RFC-022 §1–§2).
///
/// Stored at `<state_dir>/daemon.json`. Rewritten only when the set of
/// runtime IDs changes, not on every serialization tick.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonIndexV1 {
    /// Must be [`DAEMON_INDEX_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Informational server version that last wrote this file.
    pub server_version: String,
    /// IDs of all runtimes managed by this daemon.
    pub runtime_ids: Vec<Uuid>,
    /// When this daemon index was first created.
    pub created_at: SystemTime,
    /// When this file was last written.
    pub last_serialized_at: SystemTime,
}

// ── Per-runtime file ────────────────────────────────────────────────

/// Top-level wrapper stored in `<runtime_dir>/runtime.json` (RFC-022 §3).
///
/// Combines the durable spec with the semi-durable instance data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeFileV1 {
    /// Must be [`RUNTIME_FILE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Durable runtime specification.
    pub spec: RuntimeSpecV1,
    /// Semi-durable instance data (bounded age, rebuilt on restart).
    pub instance: RuntimeInstanceV1,
}

/// Durable runtime specification — identity, policy, panes, history.
///
/// These fields survive daemon restarts and are the source of truth for
/// runtime reconstruction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSpecV1 {
    /// Unique runtime identifier.
    pub id: Uuid,
    /// Human-readable runtime name.
    pub name: String,
    /// Retention policy.
    pub policy: RuntimePolicy,
    /// When this runtime was created.
    pub created_at: SystemTime,
    /// Pane specifications.
    pub panes: Vec<PaneSpecV1>,
    /// Active pane ID within this runtime.
    pub active_pane_id: Option<Uuid>,
    /// Per-runtime command history.
    pub command_history: Vec<HistoryEntryV1>,
}

/// Semi-durable instance data — revision counters and timestamps.
///
/// Written alongside the spec but considered bounded-age: the daemon
/// rebuilds these on restart from the live process state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeInstanceV1 {
    /// Monotonic mutation counter.
    pub revision: u64,
    /// When the runtime was last active (had attached clients or I/O).
    pub last_active_at: SystemTime,
    /// When screen snapshots were last flushed to disk.
    pub last_snapshot_at: SystemTime,
}

// ── Pane spec ───────────────────────────────────────────────────────

/// Durable pane specification (RFC-022 §3).
///
/// Captures the identity and last-known terminal state of a single pane.
/// Scrollback and screen data live in separate files; this struct holds
/// only the metadata needed to reconstruct the pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneSpecV1 {
    /// Unique pane identifier.
    pub id: Uuid,
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

/// Per-runtime command history entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntryV1 {
    /// The command text.
    pub command: String,
    /// Working directory when the command was run.
    pub cwd: String,
    /// When the command was executed.
    pub timestamp: SystemTime,
    /// Which pane the command was run in.
    pub pane_id: Uuid,
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
    use std::time::SystemTime;

    fn sample_pane_spec() -> PaneSpecV1 {
        PaneSpecV1 {
            id: Uuid::new_v4(),
            cwd: Some("/home/user".into()),
            title: Some("bash".into()),
            exit_status: None,
            cols: 80,
            rows: 24,
            no_persist: false,
        }
    }

    fn sample_history_entry() -> HistoryEntryV1 {
        HistoryEntryV1 {
            command: "ls -la".into(),
            cwd: "/home/user".into(),
            timestamp: SystemTime::now(),
            pane_id: Uuid::new_v4(),
        }
    }

    fn sample_runtime_spec() -> RuntimeSpecV1 {
        RuntimeSpecV1 {
            id: Uuid::new_v4(),
            name: "dev".into(),
            policy: RuntimePolicy::Persistent,
            created_at: SystemTime::now(),
            panes: vec![sample_pane_spec()],
            active_pane_id: None,
            command_history: vec![sample_history_entry()],
        }
    }

    fn sample_runtime_instance() -> RuntimeInstanceV1 {
        RuntimeInstanceV1 {
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

    fn sample_runtime_file() -> RuntimeFileV1 {
        RuntimeFileV1 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: sample_runtime_spec(),
            instance: sample_runtime_instance(),
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
        let recovered: RuntimeFileV1 = serde_json::from_str(&json).unwrap();
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
        let recovered: PaneSpecV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn history_entry_round_trip() {
        let original = sample_history_entry();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let recovered: HistoryEntryV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(original, recovered);
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
    fn daemon_index_with_no_runtimes() {
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
    fn runtime_spec_with_no_panes_or_history() {
        let spec = RuntimeSpecV1 {
            id: Uuid::new_v4(),
            name: "empty".into(),
            policy: RuntimePolicy::Ephemeral,
            created_at: SystemTime::now(),
            panes: vec![],
            active_pane_id: None,
            command_history: vec![],
        };
        let json = serde_json::to_string_pretty(&spec).unwrap();
        let recovered: RuntimeSpecV1 = serde_json::from_str(&json).unwrap();
        assert!(recovered.panes.is_empty());
        assert!(recovered.command_history.is_empty());
        assert_eq!(recovered.policy, RuntimePolicy::Ephemeral);
    }

    #[test]
    fn pane_spec_with_all_optional_fields_none() {
        let pane = PaneSpecV1 {
            id: Uuid::new_v4(),
            cwd: None,
            title: None,
            exit_status: None,
            cols: 120,
            rows: 40,
            no_persist: false,
        };
        let json = serde_json::to_string_pretty(&pane).unwrap();
        let recovered: PaneSpecV1 = serde_json::from_str(&json).unwrap();
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
        let recovered: PaneSpecV1 = serde_json::from_str(json).unwrap();
        assert!(!recovered.no_persist);
    }

    #[test]
    fn pane_spec_with_no_persist_true_round_trips() {
        let mut pane = sample_pane_spec();
        pane.no_persist = true;
        let json = serde_json::to_string_pretty(&pane).unwrap();
        let recovered: PaneSpecV1 = serde_json::from_str(&json).unwrap();
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
}
