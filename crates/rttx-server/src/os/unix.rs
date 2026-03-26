//! Unix implementation of the OS abstraction layer.

use super::OsInterface;
use std::path::PathBuf;

/// Real Unix implementation using XDG directories.
#[derive(Debug)]
pub struct UnixOs;

impl OsInterface for UnixOs {
    fn runtime_dir(&self) -> PathBuf {
        std::env::var("XDG_RUNTIME_DIR")
            .map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from)
            .join("rttx-server")
            .join("v1")
    }

    fn cache_dir(&self) -> PathBuf {
        std::env::var("XDG_CACHE_HOME")
            .map_or_else(|_| dirs_fallback_cache_dir(), PathBuf::from)
            .join("rttx-server")
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
    fn cache_dir_contains_rttx_server() {
        let os = UnixOs;
        let path = os.cache_dir();
        assert!(path.to_string_lossy().contains("rttx-server"));
    }
}
