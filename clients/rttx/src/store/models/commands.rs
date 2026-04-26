//! Canonical commands model — shared run-mode enum used by `library.json`.

use serde::{Deserialize, Serialize};

/// How a command is executed by default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    Run,
    Insert,
    RunInNewPane,
}
