//! Integration tests for state serialization and resurrection.

use rttx_server::pane::PersistedPane;
use rttx_server::serialization::{ServerState, default_state_path, load_state, write_state_atomic};
use rttx_server::session::{PersistedSession, RuntimePolicy, Session};
use std::path::PathBuf;
use std::time::SystemTime;
use uuid::Uuid;

#[test]
fn serialize_and_resurrect_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_path = default_state_path(tmp.path());

    // Build state with one session and one pane.
    let state = ServerState {
        sessions: vec![PersistedSession {
            id: Uuid::new_v4(),
            name: "persist-test".into(),
            panes: vec![PersistedPane {
                id: Uuid::new_v4(),
                cwd: Some("/home/user".into()),
                title: Some("bash".into()),
                scrollback_log_path: PathBuf::from("/tmp/scrollback.log"),
                exit_status: None,
                cols: 120,
                rows: 40,
            }],
            active_pane_id: None,
            command_history: Vec::new(),
            policy: RuntimePolicy::Persistent,
            revision: 4,
            created_at: SystemTime::now(),
            last_active_at: SystemTime::now(),
        }],
        serialized_at: SystemTime::now(),
        server_version: "0.1.0".into(),
    };

    // Write to disk.
    write_state_atomic(&state, &state_path).unwrap();

    // Load and resurrect.
    let loaded = load_state(&state_path).unwrap().unwrap();
    assert_eq!(loaded.sessions.len(), 1);

    let session = Session::from_persisted(&loaded.sessions[0]);
    assert_eq!(session.name, "persist-test");
    assert_eq!(session.panes.len(), 1);
    assert_eq!(session.revision(), 4);

    let pane = session.panes.values().next().unwrap();
    assert_eq!(pane.cols, 120);
    assert_eq!(pane.rows, 40);
    assert_eq!(pane.cwd.as_deref(), Some("/home/user"));
    assert_eq!(pane.title.as_deref(), Some("bash"));
    // Resurrected panes keep their exit status from persisted state.
    assert!(pane.exit_status.is_none());
}

#[test]
fn corrupt_state_file_returns_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_path = default_state_path(tmp.path());
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(&state_path, "{corrupt json").unwrap();

    let result = load_state(&state_path);
    assert!(result.is_err());
}

#[test]
fn oversized_command_history_truncated_on_resurrection() {
    use rttx_server::pane::HistoryEntry;
    use rttx_server::session::MAX_COMMAND_HISTORY;

    let tmp = tempfile::TempDir::new().unwrap();
    let state_path = default_state_path(tmp.path());

    let oversized_history: Vec<HistoryEntry> = (0..MAX_COMMAND_HISTORY + 300)
        .map(|i| HistoryEntry {
            command: format!("cmd-{i}"),
            cwd: "/tmp".into(),
            timestamp: SystemTime::UNIX_EPOCH,
            pane_id: Uuid::new_v4(),
        })
        .collect();

    let state = ServerState {
        sessions: vec![PersistedSession {
            id: Uuid::new_v4(),
            name: "history-cap".into(),
            panes: vec![],
            active_pane_id: None,
            command_history: oversized_history,
            policy: RuntimePolicy::Persistent,
            revision: 1,
            created_at: SystemTime::now(),
            last_active_at: SystemTime::now(),
        }],
        serialized_at: SystemTime::now(),
        server_version: "0.1.0".into(),
    };

    write_state_atomic(&state, &state_path).unwrap();
    let loaded = load_state(&state_path).unwrap().unwrap();
    let session = Session::from_persisted(&loaded.sessions[0]);

    assert_eq!(
        session.command_history.len(),
        MAX_COMMAND_HISTORY,
        "oversized history should be truncated on resurrection"
    );
    assert_eq!(session.command_history[0].command, "cmd-300", "oldest entries should be dropped");
}
