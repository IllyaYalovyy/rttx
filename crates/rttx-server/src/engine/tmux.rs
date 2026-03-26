//! Tmux control mode engine (placeholder for future implementation).

use super::{Engine, PaneSpawnConfig};
use crate::pty::{Pty, PtyError};
use uuid::Uuid;

/// Engine that delegates to tmux via control mode.
///
/// This is a placeholder. The native engine ships first.
#[derive(Debug)]
pub struct TmuxEngine;

impl Engine for TmuxEngine {
    fn spawn_pane(&self, _pane_id: Uuid, _config: &PaneSpawnConfig) -> Result<Pty, PtyError> {
        Err(PtyError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "tmux engine not yet implemented",
        )))
    }
}
