//! Native engine: owns PTYs directly.

use super::{Engine, PaneSpawnConfig};
use crate::pty::{Pty, PtyConfig, PtyError};
use uuid::Uuid;

/// Engine that spawns real PTY processes.
#[derive(Debug, Default)]
pub struct NativeEngine;

impl Engine for NativeEngine {
    fn spawn_pane(&self, pane_id: Uuid, config: &PaneSpawnConfig) -> Result<Pty, PtyError> {
        let pty_config = PtyConfig {
            command: if config.command.is_empty() {
                PtyConfig::default().command
            } else {
                config.command.clone()
            },
            cwd: config.cwd.as_ref().map(std::path::PathBuf::from),
            env: config.env.clone(),
            cols: config.cols,
            rows: config.rows,
        };
        Pty::spawn(pane_id, &pty_config)
    }
}
