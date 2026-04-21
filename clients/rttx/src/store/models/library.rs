//! Canonical library document model (RFC-023 §3: `library.json`).
//!
//! Combines places and commands into a single user-authored content document.

use serde::{Deserialize, Serialize};

use super::commands::RunMode;
use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Library;
pub const CURRENT_VERSION: u32 = 1;

/// A saved launch-target place.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaceRecord {
    pub id: String,
    pub name: String,
    pub path: String,
    /// Empty means global visibility. Values are endpoint keys.
    #[serde(default)]
    pub host_tags: Vec<String>,
}

/// A saved command snippet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRecord {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(default = "default_run_mode")]
    pub default_run_mode: RunMode,
    /// Empty means global visibility. Values are endpoint keys.
    #[serde(default)]
    pub host_tags: Vec<String>,
}

const fn default_run_mode() -> RunMode {
    RunMode::Run
}

/// Top-level library document combining places and commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Library {
    #[serde(default)]
    pub places: Vec<PlaceRecord>,
    #[serde(default)]
    pub commands: Vec<CommandRecord>,
}
