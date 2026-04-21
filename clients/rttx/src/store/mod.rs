//! Client-side document store with versioned envelopes and atomic writes (RFC-023 §2, §4, §6).
//!
//! Every persisted JSON document uses a self-describing envelope with `schema`,
//! `version`, and diagnostic fields. Writes are atomic (temp + fsync + rename).
//! Loads recover from malformed files by falling back to the last-good backup.

mod envelope;
mod io;
mod paths;

pub use envelope::{DocumentEnvelope, Schema};
pub use io::{LoadOutcome, atomic_load, atomic_save};
pub use paths::StorePaths;

/// Client store providing typed document persistence with atomic I/O.
#[derive(Debug, Clone)]
pub struct ClientStore {
    paths: StorePaths,
}

impl ClientStore {
    #[must_use]
    pub const fn new(paths: StorePaths) -> Self {
        Self { paths }
    }

    #[must_use]
    pub const fn paths(&self) -> &StorePaths {
        &self.paths
    }
}
