use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug)]
    pub struct SessionRow {
        pub uuid: RefCell<String>,
        pub name: RefCell<String>,
        pub terminal_count_label: gtk4::Label,
        pub close_button: gtk4::Button,
    }

    impl Default for SessionRow {
        fn default() -> Self {
            Self {
                uuid: RefCell::new(String::new()),
                name: RefCell::new(String::new()),
                terminal_count_label: gtk4::Label::new(None),
                close_button: gtk4::Button::from_icon_name("window-close-symbolic"),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SessionRow {
        const NAME: &'static str = "RttxSessionRow";
        type Type = super::SessionRow;
        type ParentType = adw::ActionRow;
    }

    impl ObjectImpl for SessionRow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_activatable(true);
            obj.set_selectable(true);
            obj.set_title_lines(1);

            self.terminal_count_label.add_css_class("dim-label");
            self.terminal_count_label.add_css_class("caption");

            self.close_button.add_css_class("flat");
            self.close_button.add_css_class("circular");

            obj.add_suffix(&self.terminal_count_label);
            obj.add_suffix(&self.close_button);
        }
    }

    impl WidgetImpl for SessionRow {}
    impl ListBoxRowImpl for SessionRow {}
    impl PreferencesRowImpl for SessionRow {}
    impl ActionRowImpl for SessionRow {}
}

glib::wrapper! {
    pub struct SessionRow(ObjectSubclass<imp::SessionRow>)
        @extends adw::ActionRow, adw::PreferencesRow, gtk4::ListBoxRow, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Actionable;
}

impl SessionRow {
    #[must_use]
    pub fn new(uuid: &str, name: &str, terminal_count: usize) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().uuid.replace(uuid.to_string());
        obj.imp().name.replace(name.to_string());
        obj.set_title(name);
        obj.update_terminal_count(terminal_count);
        obj
    }

    #[must_use]
    pub fn uuid(&self) -> String {
        self.imp().uuid.borrow().clone()
    }

    #[must_use]
    pub fn session_name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub fn set_session_name(&self, name: &str) {
        self.imp().name.replace(name.to_string());
        self.set_title(name);
    }

    pub fn update_terminal_count(&self, count: usize) {
        self.imp().terminal_count_label.set_label(&format!("{count}"));
    }

    #[must_use]
    pub fn close_button(&self) -> &gtk4::Button {
        &self.imp().close_button
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    static GTK_INIT: Once = Once::new();

    fn ensure_gtk_init() -> bool {
        let mut success = false;
        GTK_INIT.call_once(|| {
            std::env::set_var("GTK_A11Y", "none");
            success = gtk4::init().is_ok();
        });
        if !success {
            success = std::panic::catch_unwind(|| {
                let _ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            })
            .is_ok();
        }
        success
    }

    macro_rules! require_display {
        () => {
            if !ensure_gtk_init() {
                eprintln!("SKIPPED: no display available");
                return;
            }
        };
    }

    #[test]
    fn session_row_is_an_action_row() {
        require_display!();

        let row = SessionRow::new("session-1", "Session 1", 3);

        assert!(row.is::<adw::ActionRow>());
        assert!(row.is::<gtk4::ListBoxRow>());
        assert_eq!(row.title().as_str(), "Session 1");
        assert_eq!(row.imp().terminal_count_label.label().as_str(), "3");
    }

    #[test]
    fn session_row_updates_title_and_count() {
        require_display!();

        let row = SessionRow::new("session-1", "Session 1", 1);
        row.set_session_name("Renamed");
        row.update_terminal_count(5);

        assert_eq!(row.session_name(), "Renamed");
        assert_eq!(row.title().as_str(), "Renamed");
        assert_eq!(row.imp().terminal_count_label.label().as_str(), "5");
    }
}
