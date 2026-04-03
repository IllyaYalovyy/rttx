//! Persistence compatibility fixture tests.
//!
//! Verifies that legacy, minimal, and forward-compatible JSON state
//! loads with predictable safe defaults.

use rttx_server::serialization::{ServerState, load_state, write_state_atomic};
use rttx_server::session::{PersistedSession, RuntimePolicy, Session};
use tempfile::TempDir;

fn load_json(json: &str) -> ServerState {
    serde_json::from_str(json).expect("fixture must parse")
}

fn resurrect_first(state: &ServerState) -> Session {
    Session::from_persisted(&state.sessions[0])
}

// ── Missing policy defaults to Persistent ───────────────────────

#[test]
fn missing_policy_defaults_to_persistent() {
    let state = load_json(
        r#"{
            "sessions": [{
                "id": "00000000-0000-0000-0000-000000000001",
                "name": "legacy",
                "panes": [],
                "active_pane_id": null,
                "command_history": [],
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.0.1"
        }"#,
    );
    assert_eq!(state.sessions[0].policy, RuntimePolicy::Persistent);
}

// ── Missing revision defaults to 1 ─────────────────────────────

#[test]
fn missing_revision_defaults_to_one() {
    let state = load_json(
        r#"{
            "sessions": [{
                "id": "00000000-0000-0000-0000-000000000002",
                "name": "no-revision",
                "panes": [],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.0.1"
        }"#,
    );
    let session = resurrect_first(&state);
    assert!(session.revision() >= 1, "missing revision must default to at least 1");
}

// ── Ephemeral policy roundtrips ─────────────────────────────────

#[test]
fn ephemeral_policy_roundtrips_through_json() {
    let state = load_json(
        r#"{
            "sessions": [{
                "id": "00000000-0000-0000-0000-000000000003",
                "name": "ephemeral",
                "panes": [],
                "active_pane_id": null,
                "command_history": [],
                "policy": "ephemeral",
                "revision": 5,
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.1.0"
        }"#,
    );
    assert_eq!(state.sessions[0].policy, RuntimePolicy::Ephemeral);
    assert_eq!(state.sessions[0].revision, 5);
}

// ── Pane with exit status roundtrips ────────────────────────────

#[test]
fn pane_exit_status_roundtrips() {
    let state = load_json(
        r#"{
            "sessions": [{
                "id": "00000000-0000-0000-0000-000000000004",
                "name": "exited-pane",
                "panes": [{
                    "id": "00000000-0000-0000-0000-000000000010",
                    "cwd": "/tmp",
                    "title": "bash",
                    "scrollback_log_path": "/dev/null",
                    "exit_status": 137,
                    "cols": 80,
                    "rows": 24
                }],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "revision": 2,
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.1.0"
        }"#,
    );
    let session = resurrect_first(&state);
    let pane = session.panes.values().next().unwrap();
    assert_eq!(pane.exit_status, Some(137));
}

// ── Pane with null optional fields ──────────────────────────────

#[test]
fn pane_with_null_cwd_and_title_loads() {
    let state = load_json(
        r#"{
            "sessions": [{
                "id": "00000000-0000-0000-0000-000000000005",
                "name": "null-fields",
                "panes": [{
                    "id": "00000000-0000-0000-0000-000000000011",
                    "cwd": null,
                    "title": null,
                    "scrollback_log_path": "/dev/null",
                    "exit_status": null,
                    "cols": 80,
                    "rows": 24
                }],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "revision": 1,
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.1.0"
        }"#,
    );
    let session = resurrect_first(&state);
    let pane = session.panes.values().next().unwrap();
    assert!(pane.cwd.is_none());
    assert!(pane.title.is_none());
    assert!(pane.exit_status.is_none());
}

// ── Unknown fields are ignored ──────────────────────────────────

#[test]
fn unknown_session_fields_are_ignored() {
    let state = load_json(
        r#"{
            "sessions": [{
                "id": "00000000-0000-0000-0000-000000000006",
                "name": "future-session",
                "panes": [],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "revision": 3,
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "future_field": "should be ignored",
                "another_future": 42
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "99.0.0",
            "global_future": true
        }"#,
    );
    assert_eq!(state.sessions[0].name, "future-session");
    assert_eq!(state.sessions[0].revision, 3);
}

#[test]
fn unknown_pane_fields_are_ignored() {
    let state = load_json(
        r#"{
            "sessions": [{
                "id": "00000000-0000-0000-0000-000000000007",
                "name": "future-pane",
                "panes": [{
                    "id": "00000000-0000-0000-0000-000000000012",
                    "cwd": "/home",
                    "title": "zsh",
                    "scrollback_log_path": "/dev/null",
                    "exit_status": null,
                    "cols": 80,
                    "rows": 24,
                    "future_pane_field": [1, 2, 3]
                }],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "revision": 1,
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.1.0"
        }"#,
    );
    let session = resurrect_first(&state);
    let pane = session.panes.values().next().unwrap();
    assert_eq!(pane.cwd.as_deref(), Some("/home"));
    assert_eq!(pane.title.as_deref(), Some("zsh"));
}

// ── Write-then-load preserves all metadata ──────────────────────

#[test]
fn write_load_roundtrip_preserves_policy_and_revision() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("state.json");

    let state = ServerState {
        sessions: vec![PersistedSession {
            id: uuid::Uuid::new_v4(),
            name: "roundtrip".into(),
            panes: Vec::new(),
            active_pane_id: None,
            command_history: Vec::new(),
            policy: RuntimePolicy::Ephemeral,
            revision: 42,
            created_at: std::time::SystemTime::now(),
            last_active_at: std::time::SystemTime::now(),
        }],
        serialized_at: std::time::SystemTime::now(),
        server_version: env!("CARGO_PKG_VERSION").into(),
    };

    write_state_atomic(&state, &path).unwrap();
    let loaded = load_state(&path).unwrap().unwrap();

    assert_eq!(loaded.sessions[0].policy, RuntimePolicy::Ephemeral);
    assert_eq!(loaded.sessions[0].revision, 42);
    assert_eq!(loaded.sessions[0].name, "roundtrip");
}

// ── Multiple sessions with mixed policies ───────────────────────

#[test]
fn mixed_policy_sessions_load_correctly() {
    let state = load_json(
        r#"{
            "sessions": [
                {
                    "id": "00000000-0000-0000-0000-000000000008",
                    "name": "persistent-one",
                    "panes": [],
                    "active_pane_id": null,
                    "command_history": [],
                    "policy": "persistent",
                    "revision": 10,
                    "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                    "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
                },
                {
                    "id": "00000000-0000-0000-0000-000000000009",
                    "name": "ephemeral-one",
                    "panes": [],
                    "active_pane_id": null,
                    "command_history": [],
                    "policy": "ephemeral",
                    "revision": 7,
                    "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                    "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
                }
            ],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.1.0"
        }"#,
    );
    assert_eq!(state.sessions.len(), 2);
    assert_eq!(state.sessions[0].policy, RuntimePolicy::Persistent);
    assert_eq!(state.sessions[1].policy, RuntimePolicy::Ephemeral);
    assert_eq!(state.sessions[0].revision, 10);
    assert_eq!(state.sessions[1].revision, 7);
}
