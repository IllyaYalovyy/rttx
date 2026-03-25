use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config;
use crate::window::Window;

/// Build and run the application.
#[must_use]
pub fn run() -> glib::ExitCode {
    pretty_env_logger::init();

    let app = adw::Application::builder()
        .application_id(config::app_id())
        .flags(gtk4::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup(|_| {
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };

        let css = gtk4::CssProvider::new();
        css.load_from_string(
            "vte-terminal {
                padding: 2px 6px;
            }
            @keyframes bell-flash {
                from { background-color: alpha(@warning_color, 0.4); }
                to   { background-color: transparent; }
            }
            .bell-flash {
                animation: bell-flash 0.15s ease-out;
            }
            .session-activity-idle {
                opacity: 0.45;
            }",
        );
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
