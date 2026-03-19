mod application;
mod sidebar;
mod terminal;
mod window;

fn main() -> gtk4::glib::ExitCode {
    application::run()
}
