//! Engine abstraction for pane process management.
//!
//! The `Engine` trait defines the contract for spawning and managing pane
//! processes. Two implementations are planned:
//! - `NativeEngine` — owns PTYs directly (ships first)
//! - `TmuxEngine` — delegates to tmux control mode (ships second)

pub mod native;
pub mod tmux;

use crate::pty::Pty;
use uuid::Uuid;

/// Configuration for spawning a pane.
#[derive(Debug, Clone)]
pub struct PaneSpawnConfig {
    /// Command to run (default: user's shell).
    pub command: Vec<String>,
    /// Working directory.
    pub cwd: Option<String>,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
}

/// Trait for pane process engines.
pub trait Engine: Send + Sync {
    /// Spawn a new pane process, returning the PTY handle.
    fn spawn_pane(
        &self,
        pane_id: Uuid,
        config: &PaneSpawnConfig,
    ) -> Result<Pty, crate::pty::PtyError>;
}
