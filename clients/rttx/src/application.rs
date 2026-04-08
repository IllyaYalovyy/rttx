use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config;
use crate::window::Window;

pub(crate) const APP_CSS: &str = "\
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
    }
    @media (prefers-color-scheme: dark) {
        .accent-blue   { color: @blue_3; }
        .accent-green  { color: @green_3; }
        .accent-yellow { color: @yellow_3; }
        .accent-red    { color: @red_3; }
        .accent-purple { color: @purple_3; }
        .accent-pink   { color: @pink_3; }
        .accent-teal   { color: @teal_3; }
        .accent-orange { color: @orange_3; }
    }
    @media (prefers-color-scheme: light) {
        .accent-blue   { color: @blue_5; }
        .accent-green  { color: @green_5; }
        .accent-yellow { color: @yellow_5; }
        .accent-red    { color: @red_5; }
        .accent-purple { color: @purple_5; }
        .accent-pink   { color: @pink_5; }
        .accent-teal   { color: @teal_5; }
        .accent-orange { color: @orange_5; }
    }";

/// Build and run the application.
#[must_use]
pub fn run() -> glib::ExitCode {
    if config::is_development() && std::env::var_os("RUST_LOG").is_none() {
        pretty_env_logger::formatted_builder().filter_level(log::LevelFilter::Debug).init();
    } else {
        pretty_env_logger::init();
    }

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

    app.run()
}
