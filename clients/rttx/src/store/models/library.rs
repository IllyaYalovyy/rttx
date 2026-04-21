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

// ── Conversions to/from the existing domain types ───────────

impl From<PlaceRecord> for crate::places::Place {
    fn from(rec: PlaceRecord) -> Self {
        Self { uuid: rec.id, name: rec.name, path: rec.path, host_tags: rec.host_tags }
    }
}

impl From<&crate::places::Place> for PlaceRecord {
    fn from(place: &crate::places::Place) -> Self {
        Self {
            id: place.uuid.clone(),
            name: place.name.clone(),
            path: place.path.clone(),
            host_tags: place.host_tags.clone(),
        }
    }
}

impl From<CommandRecord> for crate::commands::SavedCommand {
    fn from(rec: CommandRecord) -> Self {
        Self {
            uuid: rec.id,
            title: rec.title,
            body: rec.body,
            default_run_mode: match rec.default_run_mode {
                RunMode::Run => crate::commands::CommandRunMode::Run,
                RunMode::Insert => crate::commands::CommandRunMode::Insert,
            },
            host_tags: rec.host_tags,
        }
    }
}

impl From<&crate::commands::SavedCommand> for CommandRecord {
    fn from(cmd: &crate::commands::SavedCommand) -> Self {
        Self {
            id: cmd.uuid.clone(),
            title: cmd.title.clone(),
            body: cmd.body.clone(),
            default_run_mode: match cmd.default_run_mode {
                crate::commands::CommandRunMode::Run => RunMode::Run,
                crate::commands::CommandRunMode::Insert => RunMode::Insert,
            },
            host_tags: cmd.host_tags.clone(),
        }
    }
}
