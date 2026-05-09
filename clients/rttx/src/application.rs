use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config;
use crate::window::Window;

pub const APP_CSS: &str = "\
    .terminal-pane {
        border-radius: 10px;
        border: 1px solid alpha(@window_fg_color, 0.10);
        background: alpha(@view_bg_color, 0.78);
    }
    .terminal-pane-active {
        border-color: alpha(@accent_bg_color, 0.85);
        background: alpha(@view_bg_color, 0.92);
    }
    .terminal-header {
        padding: 6px 8px;
        border-top-left-radius: 10px;
        border-top-right-radius: 10px;
        border-bottom: 1px solid alpha(@window_fg_color, 0.08);
        background: alpha(@headerbar_bg_color, 0.72);
    }
    .terminal-pane-active .terminal-header {
        background: alpha(@accent_bg_color, 0.14);
    }
    .terminal-scroller {
        background: transparent;
    }
    vte-terminal {
        margin: 2px 6px;
    }
    @keyframes bell-flash {
        from { background-color: alpha(@warning_color, 0.4); }
        to   { background-color: transparent; }
    }
    .bell-flash {
        animation: bell-flash 0.15s ease-out;
    }
    @keyframes activity-pulse {
        0%   { box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.9); }
        50%  { box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.4); }
        100% { box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.9); }
    }
    .session-activity-active {
        box-shadow: inset 3px 0 0 0 @accent_bg_color;
        animation: activity-pulse 1.8s ease-in-out infinite;
    }
    .session-activity-idle {
        box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.45);
    }
    .session-row .subtitle {
        opacity: 0.55;
    }";

const ACCENT_CSS_DARK: &str = "\
    .accent-blue   { color: @blue_3; }
    .accent-green  { color: @green_3; }
    .accent-yellow { color: @yellow_3; }
    .accent-red    { color: @red_3; }
    .accent-purple { color: @purple_3; }
    .accent-pink   { color: @pink_3; }
    .accent-teal   { color: @teal_3; }
    .accent-orange { color: @orange_3; }";

const ACCENT_CSS_LIGHT: &str = "\
    .accent-blue   { color: @blue_5; }
    .accent-green  { color: @green_5; }
    .accent-yellow { color: @yellow_5; }
    .accent-red    { color: @red_5; }
    .accent-purple { color: @purple_5; }
    .accent-pink   { color: @pink_5; }
    .accent-teal   { color: @teal_5; }
    .accent-orange { color: @orange_5; }";

/// Return the accent color CSS for the current color scheme.
#[must_use]
pub const fn accent_css_for_dark(is_dark: bool) -> &'static str {
    if is_dark { ACCENT_CSS_DARK } else { ACCENT_CSS_LIGHT }
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let is_dev = config::is_development();
    let default_level = if is_dev { "debug" } else { "rttx=info,warn" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let log_dir = log_dir_path();
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::daily(&log_dir, "rttx.log");
    cleanup_old_logs(&log_dir, "rttx.log", 3);

    tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_env_filter(filter)
        .with_ansi(false)
        .init();
}

fn cleanup_old_logs(dir: &std::path::Path, prefix: &str, keep_days: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut log_files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with(prefix)
                && e.file_type().is_ok_and(|t| t.is_file())
        })
        .collect();
    log_files.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    for old in log_files.into_iter().skip(keep_days + 1) {
        let _ = std::fs::remove_file(old.path());
    }
}

/// Return the log directory for the GUI.
#[must_use]
pub fn log_dir_path() -> std::path::PathBuf {
    let cache_dir = glib::user_cache_dir();
    let dir_name = if config::is_development() { "rttx-devel" } else { "rttx" };
    cache_dir.join(dir_name)
}

/// Build and run the application.
#[must_use]
pub fn run() -> glib::ExitCode {
    init_logging();

    let app = adw::Application::builder()
        .application_id(config::app_id())
        .flags(gtk4::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup(|_| {
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };

        let css = gtk4::CssProvider::new();
        css.load_from_string(APP_CSS);
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let accent_css = gtk4::CssProvider::new();
        let is_dark = adw::StyleManager::default().is_dark();
        accent_css.load_from_string(accent_css_for_dark(is_dark));
        gtk4::style_context_add_provider_for_display(
            &display,
            &accent_css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        adw::StyleManager::default().connect_dark_notify(move |mgr| {
            accent_css.load_from_string(accent_css_for_dark(mgr.is_dark()));
        });

        if !config::is_development() {
            return;
        }

        let icon_search_path = config::dev_icon_search_path();
        if !icon_search_path.exists() {
            return;
        }

        gtk4::IconTheme::for_display(&display).add_search_path(&icon_search_path);
    });

    app.connect_command_line(|app, _cmdline| {
        app.activate();
        0
    });

    app.connect_activate(|app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        let window = Window::new(app);
        window.set_icon_name(Some(config::icon_name()));
        window.present();
    });

    app.set_accels_for_action("win.new-session", &["<Ctrl><Shift>T"]);

    // Application-level action so D-Bus ActivateAction works (used by UI tests).
    let new_session_action = gtk4::gio::SimpleAction::new("new-session", None);
    let app_ref = app.clone();
    new_session_action.connect_activate(move |_, _| {
        if let Some(win) = app_ref.active_window() {
            let _ = win.activate_action("new-session", None);
        }
    });
    app.add_action(&new_session_action);

    // Create a managed local workspace directly (used by UI tests to bypass dialogs).
    let create_managed_action = gtk4::gio::SimpleAction::new("create-managed-local", None);
    let app_ref = app.clone();
    create_managed_action.connect_activate(move |_, _| {
        if let Some(win) = app_ref.active_window()
            && let Ok(win) = win.downcast::<Window>()
        {
            win.add_managed_session_at(None);
        }
    });
    app.add_action(&create_managed_action);

    // Create a direct workspace without showing the new-workspace dialog (used by UI tests).
    let create_direct_action = gtk4::gio::SimpleAction::new("create-direct", None);
    let app_ref = app.clone();
    create_direct_action.connect_activate(move |_, _| {
        if let Some(win) = app_ref.active_window()
            && let Ok(win) = win.downcast::<Window>()
        {
            win.add_direct_session();
        }
    });
    app.add_action(&create_direct_action);

    // Close the currently visible workspace (used by UI tests).
    let close_workspace_action = gtk4::gio::SimpleAction::new("close-current-workspace", None);
    let app_ref = app.clone();
    close_workspace_action.connect_activate(move |_, _| {
        if let Some(win) = app_ref.active_window()
            && let Ok(win) = win.downcast::<Window>()
            && let Some(uuid) = win.visible_session_uuid()
        {
            win.close_session(&uuid);
        }
    });
    app.add_action(&close_workspace_action);

    let rename_workspace_action =
        gtk4::gio::SimpleAction::new("rename-current-workspace", Some(glib::VariantTy::STRING));
    let app_ref = app.clone();
    rename_workspace_action.connect_activate(move |_, param| {
        if let Some(win) = app_ref.active_window()
            && let Ok(win) = win.downcast::<Window>()
            && let Some(name) = param.and_then(glib::Variant::get::<String>)
            && let Some(uuid) = win.visible_session_uuid()
        {
            win.rename_runtime(&uuid, &name);
        }
    });
    app.add_action(&rename_workspace_action);

    let rename_pane_action =
        gtk4::gio::SimpleAction::new("rename-pane", Some(glib::VariantTy::STRING));
    let app_ref = app.clone();
    rename_pane_action.connect_activate(move |_, param| {
        if let Some(win) = app_ref.active_window()
            && let Ok(win) = win.downcast::<Window>()
            && let Some(name) = param.and_then(glib::Variant::get::<String>)
        {
            win.rename_focused_pane_direct(&name);
        }
    });
    app.add_action(&rename_pane_action);

    let save_state_action = gtk4::gio::SimpleAction::new("save-state", None);
    let app_ref = app.clone();
    save_state_action.connect_activate(move |_, _| {
        if let Some(win) = app_ref.active_window()
            && let Ok(win) = win.downcast::<Window>()
        {
            win.save_state();
        }
    });
    app.add_action(&save_state_action);

    // Data management actions.
    let export_action = gtk4::gio::SimpleAction::new("export-config", None);
    let app_ref = app.clone();
    export_action.connect_activate(move |_, _| {
        let Some(win) = app_ref.active_window() else { return };
        let Some(win) = win.downcast_ref::<Window>() else { return };
        export_config_dialog(win);
    });
    app.add_action(&export_action);

    let import_action = gtk4::gio::SimpleAction::new("import-config", None);
    let app_ref = app.clone();
    import_action.connect_activate(move |_, _| {
        let Some(win) = app_ref.active_window() else { return };
        let Some(win) = win.downcast_ref::<Window>() else { return };
        import_config_confirm(win);
    });
    app.add_action(&import_action);

    let reset_action = gtk4::gio::SimpleAction::new("reset-config", None);
    reset_action.connect_activate(|_, _| {
        tracing::info!("reset-config action triggered (not yet implemented)");
    });
    app.add_action(&reset_action);

    app.run()
}

fn today_date_string() -> String {
    use std::time::SystemTime;
    let secs =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = secs / 86400;
    let (year, month, day) = crate::store::envelope::days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn export_config_dialog(win: &Window) {
    let filter = gtk4::FileFilter::new();
    filter.add_suffix("json");
    filter.set_name(Some("JSON files"));

    let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
    filters.append(&filter);

    let dialog = gtk4::FileDialog::builder()
        .title("Export Configuration")
        .initial_name(format!("rttx-config-{}.json", today_date_string()))
        .default_filter(&filter)
        .filters(&filters)
        .build();

    let win_clone = win.clone();
    dialog.save(Some(win), gtk4::gio::Cancellable::NONE, move |result| {
        let file = match result {
            Ok(f) => f,
            Err(e) => {
                if !e.matches(gtk4::gio::IOErrorEnum::Cancelled) {
                    win_clone.show_toast(&format!("Export failed: {e}"));
                }
                return;
            }
        };

        let Some(path) = file.path() else {
            win_clone.show_toast("Export failed: invalid file path");
            return;
        };

        let store = crate::store::default_store();
        let bundle = store.export_bundle();
        let envelope = crate::store::models::export::ExportEnvelope::new(bundle);

        let json = match serde_json::to_string_pretty(&envelope) {
            Ok(j) => j,
            Err(e) => {
                win_clone.show_toast(&format!("Export failed: {e}"));
                return;
            }
        };

        if let Err(e) = std::fs::write(&path, json) {
            win_clone.show_toast(&format!("Export failed: {e}"));
            return;
        }

        win_clone.show_toast("Configuration exported");
    });
}

fn import_config_confirm(win: &Window) {
    let dialog = adw::AlertDialog::builder()
        .heading("Replace Configuration?")
        .body(
            "This will replace all current settings, bookmarks, hosts, and workspaces \
             with the contents of the imported file. This cannot be undone.",
        )
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("replace", "Replace");
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);

    let win_clone = win.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "replace" {
            import_config_file_dialog(&win_clone);
        }
    });
    dialog.present(Some(win));
}

fn import_config_file_dialog(win: &Window) {
    let filter = gtk4::FileFilter::new();
    filter.add_suffix("json");
    filter.set_name(Some("JSON files"));

    let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
    filters.append(&filter);

    let dialog = gtk4::FileDialog::builder()
        .title("Import Configuration")
        .default_filter(&filter)
        .filters(&filters)
        .build();

    let win_clone = win.clone();
    dialog.open(Some(win), gtk4::gio::Cancellable::NONE, move |result| {
        let file = match result {
            Ok(f) => f,
            Err(e) => {
                if !e.matches(gtk4::gio::IOErrorEnum::Cancelled) {
                    show_error_dialog(&win_clone, &format!("Import failed: {e}"));
                }
                return;
            }
        };

        let Some(path) = file.path() else {
            show_error_dialog(&win_clone, "Import failed: invalid file path");
            return;
        };

        let json = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                show_error_dialog(&win_clone, &format!("Could not read the file: {e}"));
                return;
            }
        };

        let bundle = match crate::store::models::export::parse_export_file(&json) {
            Ok(b) => b,
            Err(e) => {
                show_error_dialog(&win_clone, &e.to_string());
                return;
            }
        };

        if let Err(e) = crate::store::default_store().import_bundle(&bundle) {
            show_error_dialog(&win_clone, &format!("Import failed: {e}"));
            return;
        }

        win_clone.show_toast("Configuration imported. Please restart rttx.");
        glib::timeout_add_seconds_local_once(2, move || {
            std::process::exit(0);
        });
    });
}

fn show_error_dialog(win: &Window, message: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading("Import Error")
        .body(message)
        .close_response("ok")
        .default_response("ok")
        .build();
    dialog.add_response("ok", "OK");
    dialog.present(Some(win));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_date_string_matches_yyyy_mm_dd_format() {
        let date = today_date_string();
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        let year: u32 = date[..4].parse().unwrap();
        let month: u32 = date[5..7].parse().unwrap();
        let day: u32 = date[8..10].parse().unwrap();
        assert!(year >= 2024);
        assert!((1..=12).contains(&month));
        assert!((1..=31).contains(&day));
    }

    #[test]
    fn cleanup_old_logs_keeps_recent_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["rttx.log.2025-01-01", "rttx.log.2025-01-02", "rttx.log.2025-01-03"] {
            std::fs::write(dir.path().join(name), "data").unwrap();
        }
        cleanup_old_logs(dir.path(), "rttx.log", 2);
        let remaining: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(remaining.len(), 3);
    }

    #[test]
    fn cleanup_old_logs_removes_excess_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "rttx.log.2025-01-01",
            "rttx.log.2025-01-02",
            "rttx.log.2025-01-03",
            "rttx.log.2025-01-04",
            "rttx.log.2025-01-05",
        ] {
            std::fs::write(dir.path().join(name), "data").unwrap();
        }
        cleanup_old_logs(dir.path(), "rttx.log", 2);
        let mut remaining: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec!["rttx.log.2025-01-03", "rttx.log.2025-01-04", "rttx.log.2025-01-05"]
        );
    }

    #[test]
    fn cleanup_old_logs_ignores_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["rttx.log.2025-01-01", "rttx.log.2025-01-02", "other.txt"] {
            std::fs::write(dir.path().join(name), "data").unwrap();
        }
        cleanup_old_logs(dir.path(), "rttx.log", 0);
        let mut remaining: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        remaining.sort();
        assert_eq!(remaining, vec!["other.txt", "rttx.log.2025-01-02"]);
    }

    #[test]
    fn cleanup_old_logs_handles_missing_directory() {
        let dir = std::path::Path::new("/tmp/rttx-nonexistent-test-dir");
        cleanup_old_logs(dir, "rttx.log", 2);
    }
}
