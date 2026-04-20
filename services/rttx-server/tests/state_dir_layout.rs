//! Integration test verifying the `XDG_STATE_HOME` directory layout
//! coordinated between RFC-022 (daemon) and RFC-023 (client).

use rttx_server::os::OsInterface;
use rttx_server::os::unix::UnixOs;
use std::path::PathBuf;

#[test]
fn state_dir_lives_under_xdg_state_home_with_daemon_subdir() {
    let os = UnixOs;
    let state = os.state_dir();

    assert!(state.ends_with("daemon"), "state_dir must end with 'daemon', got {}", state.display());

    let cache = os.cache_dir();
    assert!(
        !state.starts_with(&cache) && !cache.starts_with(&state),
        "state_dir ({}) and cache_dir ({}) must be disjoint",
        state.display(),
        cache.display()
    );
}

#[test]
fn test_os_state_dir_follows_rfc_022_layout() {
    #[derive(Debug)]
    struct TestOs {
        state_base: PathBuf,
    }
    impl OsInterface for TestOs {
        fn runtime_dir(&self) -> PathBuf {
            PathBuf::from("/unused")
        }
        fn cache_dir(&self) -> PathBuf {
            PathBuf::from("/unused")
        }
        fn state_dir(&self) -> PathBuf {
            self.state_base.join("rttx").join("daemon")
        }
    }

    let os = TestOs { state_base: PathBuf::from("/home/user/.local/state") };
    assert_eq!(os.state_dir(), PathBuf::from("/home/user/.local/state/rttx/daemon"));
}
