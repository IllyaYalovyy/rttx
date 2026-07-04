//! Integration test verifying the v2 state layout path helpers (RFC-022 §1)
//! produce correct paths when composed with `OsInterface::state_dir`.

use rttx_server::os::OsInterface;
use rttx_server::state::layout;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug)]
struct FakeOs(PathBuf);

impl OsInterface for FakeOs {
    fn runtime_dir(&self) -> PathBuf {
        PathBuf::from("/unused")
    }
    fn cache_dir(&self) -> PathBuf {
        PathBuf::from("/unused")
    }
    fn state_dir(&self) -> PathBuf {
        self.0.clone()
    }
}

#[test]
fn v2_layout_rooted_at_state_dir() {
    let os = FakeOs(PathBuf::from("/home/user/.local/state/rttx/daemon"));
    let state = os.state_dir();

    let idx = layout::daemon_index(&state);
    assert_eq!(idx, PathBuf::from("/home/user/.local/state/rttx/daemon/daemon.json"));

    let workspaces = layout::runtimes_dir(&state);
    assert_eq!(workspaces, PathBuf::from("/home/user/.local/state/rttx/daemon/workspaces"));
}

#[test]
fn v2_workspace_tree_is_self_contained() {
    let os = FakeOs(PathBuf::from("/state/rttx/daemon"));
    let state = os.state_dir();
    let rt = Uuid::new_v4();
    let pane = Uuid::new_v4();

    let rt_dir = layout::runtime_dir(&state, rt);

    // Every artifact lives under the workspace directory.
    assert!(layout::runtime_file(&state, rt).starts_with(&rt_dir));
    assert!(layout::screen_snapshot(&state, rt, pane).starts_with(&rt_dir));
    assert!(layout::scrollback_log(&state, rt, pane).starts_with(&rt_dir));
    assert!(layout::history_file(&state, rt, pane).starts_with(&rt_dir));
}

#[test]
fn v2_paths_never_overlap_v1_cache_paths() {
    let os = rttx_server::os::unix::UnixOs;
    let cache = os.cache_dir();
    let state = os.state_dir();

    let rt = Uuid::new_v4();
    let pane = Uuid::new_v4();

    let v2_index = layout::daemon_index(&state);
    let v2_scrollback = layout::scrollback_log(&state, rt, pane);
    let v2_history = layout::history_file(&state, rt, pane);

    // State paths must not fall under the old cache directory.
    assert!(!v2_index.starts_with(&cache));
    assert!(!v2_scrollback.starts_with(&cache));
    assert!(!v2_history.starts_with(&cache));
}
