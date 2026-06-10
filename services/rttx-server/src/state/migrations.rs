//! Typed schema migration chain (RFC-022 §2).
//!
//! Each persisted file type has a current schema version. When loading a
//! file whose `schema_version` is older than current, the migration chain
//! walks it forward one version at a time until it reaches the current
//! struct.
//!
//! Migrations are total: any supported past version can be walked forward
//! to current. Unknown future versions are rejected — the daemon refuses
//! to load data it does not understand rather than guessing.

use crate::state::types::{
    DAEMON_INDEX_SCHEMA_VERSION, DaemonIndexV1, SCREEN_SNAPSHOT_SCHEMA_VERSION,
    SchemaVersionEnvelope, ScreenSnapshotV1,
};
use std::fmt;

/// Errors that can occur during schema migration.
#[derive(Debug)]
pub enum MigrationError {
    /// The file's `schema_version` is newer than this daemon understands.
    UnsupportedFutureVersion { file_kind: &'static str, found: u32, max_supported: u32 },
    /// JSON deserialization failed during migration.
    DeserializationFailed { file_kind: &'static str, version: u32, source: serde_json::Error },
    /// Could not read the `schema_version` envelope.
    InvalidEnvelope { source: serde_json::Error },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFutureVersion { file_kind, found, max_supported } => write!(
                f,
                "{file_kind}: schema_version {found} is newer than max supported {max_supported}"
            ),
            Self::DeserializationFailed { file_kind, version, source } => {
                write!(f, "{file_kind}: failed to deserialize schema_version {version}: {source}")
            }
            Self::InvalidEnvelope { source } => {
                write!(f, "failed to read schema_version envelope: {source}")
            }
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DeserializationFailed { source, .. } | Self::InvalidEnvelope { source } => {
                Some(source)
            }
            Self::UnsupportedFutureVersion { .. } => None,
        }
    }
}

/// Peek at the `schema_version` field without fully deserializing.
pub fn peek_schema_version(json: &str) -> Result<u32, MigrationError> {
    let envelope: SchemaVersionEnvelope =
        serde_json::from_str(json).map_err(|e| MigrationError::InvalidEnvelope { source: e })?;
    Ok(envelope.schema_version)
}

/// Load and migrate a daemon index from JSON to the current version.
pub fn load_daemon_index(json: &str) -> Result<DaemonIndexV1, MigrationError> {
    let version = peek_schema_version(json)?;
    match version {
        1 => serde_json::from_str(json).map_err(|e| MigrationError::DeserializationFailed {
            file_kind: "DaemonIndex",
            version: 1,
            source: e,
        }),
        v if v > DAEMON_INDEX_SCHEMA_VERSION => Err(MigrationError::UnsupportedFutureVersion {
            file_kind: "DaemonIndex",
            found: v,
            max_supported: DAEMON_INDEX_SCHEMA_VERSION,
        }),
        // Future: add `0 => migrate_daemon_index_v0_to_v1(...)` etc.
        v => Err(MigrationError::UnsupportedFutureVersion {
            file_kind: "DaemonIndex",
            found: v,
            max_supported: DAEMON_INDEX_SCHEMA_VERSION,
        }),
    }
}

/// Load and migrate a screen snapshot from JSON to the current version.
pub fn load_screen_snapshot(json: &str) -> Result<ScreenSnapshotV1, MigrationError> {
    let version = peek_schema_version(json)?;
    match version {
        1 => serde_json::from_str(json).map_err(|e| MigrationError::DeserializationFailed {
            file_kind: "ScreenSnapshot",
            version: 1,
            source: e,
        }),
        v if v > SCREEN_SNAPSHOT_SCHEMA_VERSION => Err(MigrationError::UnsupportedFutureVersion {
            file_kind: "ScreenSnapshot",
            found: v,
            max_supported: SCREEN_SNAPSHOT_SCHEMA_VERSION,
        }),
        v => Err(MigrationError::UnsupportedFutureVersion {
            file_kind: "ScreenSnapshot",
            found: v,
            max_supported: SCREEN_SNAPSHOT_SCHEMA_VERSION,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::*;
    use std::time::SystemTime;

    fn sample_daemon_index_json() -> String {
        let index = DaemonIndexV1 {
            schema_version: DAEMON_INDEX_SCHEMA_VERSION,
            server_version: "0.4.0".into(),
            runtime_ids: vec![uuid::Uuid::new_v4()],
            created_at: SystemTime::now(),
            last_serialized_at: SystemTime::now(),
        };
        serde_json::to_string_pretty(&index).unwrap()
    }

    fn sample_screen_snapshot_json() -> String {
        let snap = ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id: uuid::Uuid::new_v4(),
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
        serde_json::to_string_pretty(&snap).unwrap()
    }

    // ── Happy path: current version loads directly ──────────────────

    #[test]
    fn load_daemon_index_v1() {
        let json = sample_daemon_index_json();
        let result = load_daemon_index(&json);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().schema_version, 1);
    }

    #[test]
    fn load_screen_snapshot_v1() {
        let json = sample_screen_snapshot_json();
        let result = load_screen_snapshot(&json);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().schema_version, 1);
    }

    // ── Future version rejection ────────────────────────────────────

    #[test]
    fn daemon_index_rejects_future_version() {
        let json = r#"{"schema_version": 99, "server_version": "9.0.0", "runtime_ids": [], "created_at": {"secs_since_epoch": 0, "nanos_since_epoch": 0}, "last_serialized_at": {"secs_since_epoch": 0, "nanos_since_epoch": 0}}"#;
        let err = load_daemon_index(json).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::UnsupportedFutureVersion { file_kind: "DaemonIndex", found: 99, .. }
        ));
    }

    #[test]
    fn screen_snapshot_rejects_future_version() {
        let json = r#"{"schema_version": 200}"#;
        let err = load_screen_snapshot(json).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::UnsupportedFutureVersion {
                file_kind: "ScreenSnapshot",
                found: 200,
                ..
            }
        ));
    }

    // ── Invalid JSON ────────────────────────────────────────────────

    #[test]
    fn invalid_json_returns_envelope_error() {
        let err = peek_schema_version("not json at all").unwrap_err();
        assert!(matches!(err, MigrationError::InvalidEnvelope { .. }));
    }

    #[test]
    fn missing_schema_version_returns_envelope_error() {
        let err = peek_schema_version(r#"{"server_version": "1.0"}"#).unwrap_err();
        assert!(matches!(err, MigrationError::InvalidEnvelope { .. }));
    }

    #[test]
    fn daemon_index_corrupt_body_returns_deserialization_error() {
        // Valid schema_version but missing required fields.
        let json = r#"{"schema_version": 1, "server_version": "0.4.0"}"#;
        let err = load_daemon_index(json).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::DeserializationFailed { file_kind: "DaemonIndex", version: 1, .. }
        ));
    }

    // ── Peek helper ─────────────────────────────────────────────────

    #[test]
    fn peek_extracts_version_from_any_json_with_schema_version() {
        let json = r#"{"schema_version": 7, "extra": "ignored"}"#;
        assert_eq!(peek_schema_version(json).unwrap(), 7);
    }

    // ── Display formatting ──────────────────────────────────────────

    #[test]
    fn migration_error_display_future_version() {
        let err = MigrationError::UnsupportedFutureVersion {
            file_kind: "DaemonIndex",
            found: 5,
            max_supported: 1,
        };
        let msg = err.to_string();
        assert!(msg.contains("DaemonIndex"));
        assert!(msg.contains('5'));
        assert!(msg.contains('1'));
    }

    #[test]
    fn migration_error_display_envelope() {
        let err = MigrationError::InvalidEnvelope {
            source: serde_json::from_str::<serde_json::Value>("bad").unwrap_err(),
        };
        let msg = err.to_string();
        assert!(msg.contains("envelope"));
    }
}
