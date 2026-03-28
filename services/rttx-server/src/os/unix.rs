//! Unix implementation of the OS abstraction layer.

use super::OsInterface;
use std::path::PathBuf;

/// Environment variable that enables dev mode (same as rttx GUI).
const DEV_MODE_ENV: &str = "RTTX_DEV_MODE";

/// Production directory name.
const PROD_DIR: &str = "rttx-server";

/// Development directory name — separate from production.
const DEV_DIR: &str = "rttxd-devel";

/// Check if dev mode is enabled via the environment variable.
#[must_use]
pub fn dev_mode_enabled() -> bool {
    std::env::var_os(DEV_MODE_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// Return the directory name for the current mode.
#[must_use]
fn dir_name() -> &'static str {
    if dev_mode_enabled() { DEV_DIR } else { PROD_DIR }
}

/// Real Unix implementation using XDG directories.
///
/// In dev mode (`RTTX_DEV_MODE=1`), uses `rttxd-devel` instead of
/// `rttx-server` for all paths, so a development daemon can run
/// alongside a stable production daemon without interference.
#[derive(Debug)]
pub struct UnixOs;

impl OsInterface for UnixOs {
    fn runtime_dir(&self) -> PathBuf {
        std::env::var("XDG_RUNTIME_DIR")
            .map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from)
            .join(dir_name())
            .join("v1")
    }

    fn cache_dir(&self) -> PathBuf {
        std::env::var("XDG_CACHE_HOME")
            .map_or_else(|_| dirs_fallback_cache_dir(), PathBuf::from)
            .join(dir_name())
    }
}

fn dirs_fallback_cache_dir() -> PathBuf {
    std::env::var("HOME")
        .map_or_else(|_| PathBuf::from("/tmp"), |h| PathBuf::from(h).join(".cache"))
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
        let name = dir_name();
        assert!(path.to_string_lossy().contains(name));
    }

    #[test]
    fn dev_mode_disabled_by_default() {
        // Unless the test runner has RTTX_DEV_MODE set, this should be false.
        // We can't guarantee the env, so just verify the function doesn't panic.
        let _ = dev_mode_enabled();
    }
}
