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
        .application_id(config::APP_ID)
        .flags(gtk4::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

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
        window.present();
    });

    app.set_accels_for_action("win.new-session", &["<Ctrl><Shift>T"]);

    app.run()
}
