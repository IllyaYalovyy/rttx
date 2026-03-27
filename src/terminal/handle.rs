use crate::terminal::persistent_widget::PersistentPaneView;
use crate::terminal::widget::TerminalWidget;
use vte4::prelude::*;

/// Common UI-facing operations for direct and managed terminal panes.
#[derive(Clone, Debug)]
pub enum TerminalHandle {
    Direct(TerminalWidget),
    Managed(PersistentPaneView),
}

impl TerminalHandle {
    /// Human-readable title for notifications and UI actions.
    #[must_use]
    pub fn title(&self) -> String {
        match self {
            Self::Direct(terminal) => terminal.title_label().label().to_string(),
            Self::Managed(pane) => pane.title_label().label().to_string(),
        }
    }

    /// Toggle the pane's inline search UI.
    pub fn toggle_search(&self) {
        match self {
            Self::Direct(terminal) => terminal.toggle_search(),
            Self::Managed(pane) => pane.toggle_search(),
        }
    }

    /// Apply a font zoom delta to the pane.
    pub fn zoom(&self, direction: i32) {
        let vte = match self {
            Self::Direct(terminal) => terminal.vte(),
            Self::Managed(pane) => pane.vte(),
        };

        match direction {
            1 => {
                let scale = vte.font_scale();
                vte.set_font_scale(scale * 1.1);
            }
            -1 => {
                let scale = vte.font_scale();
                vte.set_font_scale(scale / 1.1);
            }
            _ => vte.set_font_scale(1.0),
        }
    }

    /// Current working directory when known.
    #[must_use]
    pub fn current_directory(&self) -> Option<String> {
        match self {
            Self::Direct(terminal) => terminal.current_directory(),
            Self::Managed(pane) => pane.current_directory(),
        }
    }
}
