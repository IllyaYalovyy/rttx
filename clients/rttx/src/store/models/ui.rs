//! Canonical UI state document model (RFC-023 §3: `ui.json`).

use serde::{Deserialize, Serialize};

use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Ui;
pub const CURRENT_VERSION: u32 = 2;

/// Restorable UI state that is not workspace data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiState {
    #[serde(default = "default_width")]
    pub window_width: i32,
    #[serde(default = "default_height")]
    pub window_height: i32,
    #[serde(default)]
    pub is_maximized: bool,
    #[serde(default = "default_left_sidebar_width")]
    pub left_sidebar_width: i32,
    #[serde(default = "default_right_sidebar_width")]
    pub right_sidebar_width: i32,
    #[serde(default = "default_sidebar_visible")]
    pub left_sidebar_visible: bool,
    #[serde(default = "default_sidebar_visible")]
    pub right_sidebar_visible: bool,
    #[serde(default)]
    pub selected_right_tool: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            window_width: default_width(),
            window_height: default_height(),
            is_maximized: false,
            left_sidebar_width: default_left_sidebar_width(),
            right_sidebar_width: default_right_sidebar_width(),
            left_sidebar_visible: default_sidebar_visible(),
            right_sidebar_visible: default_sidebar_visible(),
            selected_right_tool: None,
        }
    }
}

pub(crate) const fn migrate(mut state: UiState, version: u32) -> UiState {
    // V1 wrote false for both fields regardless of the widgets' visibility.
    // Preserve the historical startup appearance instead of treating them as choices.
    if version < 2 {
        state.left_sidebar_visible = true;
        state.right_sidebar_visible = true;
    }
    state
}

const fn default_sidebar_visible() -> bool {
    true
}

const fn default_width() -> i32 {
    900
}

const fn default_height() -> i32 {
    600
}

const fn default_left_sidebar_width() -> i32 {
    220
}

const fn default_right_sidebar_width() -> i32 {
    320
}
