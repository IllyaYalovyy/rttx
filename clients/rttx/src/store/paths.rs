//! Injectable path roots for the client store (RFC-023 §1).

use std::path::{Path, PathBuf};

/// Root directories for the three XDG-based storage locations.
///
/// Config holds durable user choices, state holds restorable application state,
/// and cache holds disposable runtime data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePaths {
    config: PathBuf,
    state: PathBuf,
    cache: PathBuf,
}

impl StorePaths {
    #[must_use]
    pub const fn new(config: PathBuf, state: PathBuf, cache: PathBuf) -> Self {
        Self { config, state, cache }
    }

    #[must_use]
    pub fn config(&self) -> &Path {
        &self.config
    }

    #[must_use]
    pub fn state(&self) -> &Path {
        &self.state
    }

    #[must_use]
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// Path to the `backups/` directory under the state root.
    #[must_use]
    pub fn backups(&self) -> PathBuf {
        self.state.join("backups")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_expose_injected_roots() {
        let paths = StorePaths::new("/c".into(), "/s".into(), "/k".into());
        assert_eq!(paths.config(), Path::new("/c"));
        assert_eq!(paths.state(), Path::new("/s"));
        assert_eq!(paths.cache(), Path::new("/k"));
    }

    #[test]
    fn backups_dir_is_under_state() {
        let paths = StorePaths::new("/c".into(), "/s/client".into(), "/k".into());
        assert_eq!(paths.backups(), PathBuf::from("/s/client/backups"));
    }
}
