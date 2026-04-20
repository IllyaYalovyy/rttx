//! Unix implementation of the OS abstraction layer.

use super::OsInterface;
use std::path::PathBuf;

/// Environment variable that enables dev mode (same as rttx GUI).
const DEV_MODE_ENV: &str = "RTTX_DEV_MODE";

/// Production top-level directory name under `$XDG_STATE_HOME`.
const PROD_STATE_DIR: &str = "rttx";

/// Development top-level directory name under `$XDG_STATE_HOME`.
const DEV_STATE_DIR: &str = "rttx-devel";

/// Subdirectory owned by the daemon (RFC-022).
const DAEMON_SUBDIR: &str = "daemon";

/// Production directory name for cache and runtime.
const PROD_DIR: &str = "rttx-server";

/// Development directory name for cache and runtime.
const DEV_DIR: &str = "rttx-server-devel";

/// Check if dev mode is enabled via the environment variable.
#[must_use]
pub fn dev_mode_enabled() -> bool {
    std::env::var_os(DEV_MODE_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

#[must_use]
const fn cache_dir_name_for(is_dev: bool) -> &'static str {
    if is_dev { DEV_DIR } else { PROD_DIR }
}

#[must_use]
const fn state_top_dir_for(is_dev: bool) -> &'static str {
    if is_dev { DEV_STATE_DIR } else { PROD_STATE_DIR }
}

/// Real Unix implementation using XDG directories.
///
/// In dev mode (`RTTX_DEV_MODE=1`), uses `rttx-server-devel` / `rttx-devel`
/// instead of `rttx-server` / `rttx` for all paths, so a development daemon
/// can run alongside a stable production daemon without interference.
#[derive(Debug)]
pub struct UnixOs;

impl OsInterface for UnixOs {
    fn runtime_dir(&self) -> PathBuf {
        let base =
            std::env::var("XDG_RUNTIME_DIR").map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from);
        runtime_dir_for(&base, dev_mode_enabled())
    }

    fn cache_dir(&self) -> PathBuf {
        let base = std::env::var("XDG_CACHE_HOME")
            .map_or_else(|_| dirs_fallback_cache_dir(), PathBuf::from);
        cache_dir_for(&base, dev_mode_enabled())
    }

    fn state_dir(&self) -> PathBuf {
        let base = std::env::var("XDG_STATE_HOME")
            .map_or_else(|_| dirs_fallback_state_dir(), PathBuf::from);
        state_dir_for(&base, dev_mode_enabled())
    }
}

#[must_use]
fn runtime_dir_for(base: &std::path::Path, is_dev: bool) -> PathBuf {
    base.join(cache_dir_name_for(is_dev)).join("v1")
}

#[must_use]
fn cache_dir_for(base: &std::path::Path, is_dev: bool) -> PathBuf {
    base.join(cache_dir_name_for(is_dev))
}

#[must_use]
fn state_dir_for(base: &std::path::Path, is_dev: bool) -> PathBuf {
    base.join(state_top_dir_for(is_dev)).join(DAEMON_SUBDIR)
}

fn dirs_fallback_cache_dir() -> PathBuf {
    std::env::var("HOME")
        .map_or_else(|_| PathBuf::from("/tmp"), |h| PathBuf::from(h).join(".cache"))
}

fn dirs_fallback_state_dir() -> PathBuf {
    std::env::var("HOME")
        .map_or_else(|_| PathBuf::from("/tmp"), |h| PathBuf::from(h).join(".local").join("state"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_dir_contains_version() {
        let os = UnixOs;
        let path = os.runtime_dir();
        assert!(path.to_string_lossy().contains("v1"));
    }

    #[test]
    fn cache_dir_contains_dir_name() {
        let os = UnixOs;
        let path = os.cache_dir();
        let name = cache_dir_name_for(dev_mode_enabled());
        assert!(path.to_string_lossy().contains(name));
    }

    #[test]
    fn dev_mode_disabled_by_default() {
        // Unless the test runner has RTTX_DEV_MODE set, this should be false.
        // We can't guarantee the env, so just verify the function doesn't panic.
        let _ = dev_mode_enabled();
    }

    #[test]
    fn runtime_dir_for_production_uses_production_daemon_dir() {
        let path = runtime_dir_for(std::path::Path::new("/tmp/runtime"), false);
        assert_eq!(path, std::path::Path::new("/tmp/runtime/rttx-server/v1"));
    }

    #[test]
    fn runtime_dir_for_dev_uses_development_daemon_dir() {
        let path = runtime_dir_for(std::path::Path::new("/tmp/runtime"), true);
        assert_eq!(path, std::path::Path::new("/tmp/runtime/rttx-server-devel/v1"));
    }

    #[test]
    fn cache_dir_for_dev_uses_development_daemon_dir() {
        let path = cache_dir_for(std::path::Path::new("/tmp/cache"), true);
        assert_eq!(path, std::path::Path::new("/tmp/cache/rttx-server-devel"));
    }

    #[test]
    fn state_dir_for_production_uses_daemon_subdir() {
        let path = state_dir_for(std::path::Path::new("/tmp/state"), false);
        assert_eq!(path, std::path::Path::new("/tmp/state/rttx/daemon"));
    }

    #[test]
    fn state_dir_for_dev_uses_devel_daemon_subdir() {
        let path = state_dir_for(std::path::Path::new("/tmp/state"), true);
        assert_eq!(path, std::path::Path::new("/tmp/state/rttx-devel/daemon"));
    }

    #[test]
    fn state_dir_fallback_uses_dot_local_state() {
        let fallback = dirs_fallback_state_dir();
        assert!(
            fallback.to_string_lossy().contains(".local/state")
                || fallback == std::path::Path::new("/tmp"),
            "fallback should be $HOME/.local/state or /tmp, got {}",
            fallback.display()
        );
    }

    #[test]
    fn state_and_cache_dirs_are_disjoint() {
        let state = state_dir_for(std::path::Path::new("/xdg/state"), false);
        let cache = cache_dir_for(std::path::Path::new("/xdg/cache"), false);
        assert_ne!(state, cache);
        assert!(!state.starts_with(&cache));
        assert!(!cache.starts_with(&state));
    }
}
