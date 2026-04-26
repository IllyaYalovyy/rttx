pub mod color_scheme;
pub mod commands;
pub mod commands_window;
pub mod config;
pub mod connect_existing_dialog;
pub mod daemon;
pub mod daemon_bridge;
pub mod form_dialog;
pub mod host;
pub mod host_tag_picker;
pub mod new_workspace_dialog;
pub mod parameter_dialog;
pub mod places;
pub mod places_window;
pub mod preferences;
pub mod runtime;
pub mod shortcuts;
pub mod store;
pub mod workspace;
pub mod workspace_state;

pub mod application;
pub mod preferences_window;
pub mod sidebar;
pub mod terminal;
pub mod window;

#[cfg(test)]
pub mod test_helpers;

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
