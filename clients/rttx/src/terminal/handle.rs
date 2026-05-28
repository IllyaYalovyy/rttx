use gtk4::glib;

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
    fn vte(&self) -> &vte4::Terminal {
        match self {
            Self::Direct(terminal) => terminal.vte(),
            Self::Managed(pane) => pane.vte(),
        }
    }

    /// Capture the current VTE scroll position as a vadjustment value.
    #[must_use]
    pub fn scroll_position(&self) -> Option<f64> {
        self.vte().vadjustment().map(|adj| adj.value())
    }

    /// Schedule restoration of a saved scroll position after the next layout
    /// pass. Reparenting resets the vadjustment, so the value must be
    /// reapplied once GTK has laid out the widget.
    pub fn restore_scroll_position(&self, value: f64) {
        let vte = self.vte().clone();
        // Use idle to let GTK finish the current reparenting/layout pass
        // before touching the adjustment.
        glib::idle_add_local_once(move || {
            if let Some(adj) = vte.vadjustment() {
                // Clamp to valid range — the upper bound may have changed
                // if the terminal was resized during the rebuild.
                let clamped =
                    value.clamp(adj.lower(), (adj.upper() - adj.page_size()).max(adj.lower()));
                adj.set_value(clamped);
            }
        });
    }

    /// Human-readable title for notifications and UI actions.
    #[must_use]
    pub fn title(&self) -> String {
        match self {
            Self::Direct(terminal) => terminal.title_label().label().to_string(),
            Self::Managed(pane) => pane.title_label().label().to_string(),
        }
    }

    /// Get the custom title, if set.
    #[must_use]
    pub fn custom_title(&self) -> Option<String> {
        match self {
            Self::Direct(terminal) => terminal.custom_title(),
            Self::Managed(pane) => pane.custom_title(),
        }
    }

    /// Set or clear the custom title override.
    pub fn set_custom_title(&self, title: Option<&str>) {
        match self {
            Self::Direct(terminal) => terminal.set_custom_title(title),
            Self::Managed(pane) => pane.set_custom_title(title),
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
        let vte = self.vte();

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

    /// Copy the current terminal selection to the clipboard.
    pub fn copy_clipboard(&self) {
        crate::terminal::copy_to_clipboard(self.vte());
    }

    /// Mark the pane as active or inactive in the UI.
    pub fn set_active(&self, active: bool) {
        match self {
            Self::Direct(terminal) => terminal.set_active(active),
            Self::Managed(pane) => pane.set_active(active),
        }
    }

    /// Focus the terminal widget backing this pane.
    #[must_use]
    pub fn grab_focus(&self) -> bool {
        self.vte().grab_focus()
    }

    /// Feed the terminal cleanup byte sequence into VTE to restore sane
    /// state after a remote TUI dies without cleaning up. Resets tracked
    /// mode state for managed panes so key encoding stays correct.
    /// Also clears the crashed state if the pane was previously isolated.
    pub fn repair_terminal(&self) {
        if let Self::Managed(pane) = self {
            pane.clear_crashed();
            pane.reset_tracked_modes();
        }
        // Use safe_feed for the cleanup sequence since the VTE may be in a
        // broken state from a prior crash.
        let _ = crate::terminal::persistent_widget::safe_feed(
            self.vte(),
            crate::terminal::terminal_cleanup_bytes(),
        );
    }
}
