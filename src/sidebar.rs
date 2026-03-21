use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::RefCell;

    pub struct SessionRow {
        pub uuid: RefCell<String>,
        pub name: RefCell<String>,
        pub label: gtk4::Label,
        pub terminal_count_label: gtk4::Label,
        pub close_button: gtk4::Button,
    }

    impl Default for SessionRow {
        fn default() -> Self {
            Self {
                uuid: RefCell::new(String::new()),
                name: RefCell::new(String::new()),
                label: gtk4::Label::new(None),
                terminal_count_label: gtk4::Label::new(None),
                close_button: gtk4::Button::from_icon_name("window-close-symbolic"),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SessionRow {
        const NAME: &'static str = "RttxSessionRow";
        type Type = super::SessionRow;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for SessionRow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk4::Orientation::Horizontal);
            obj.set_spacing(6);
            obj.set_margin_start(6);
            obj.set_margin_end(6);
            obj.set_margin_top(4);
            obj.set_margin_bottom(4);

            self.label.set_hexpand(true);
            self.label.set_xalign(0.0);
            self.label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

            self.terminal_count_label.add_css_class("dim-label");
            self.terminal_count_label.add_css_class("caption");

            self.close_button.add_css_class("flat");
            self.close_button.add_css_class("circular");

            obj.append(&self.label);
            obj.append(&self.terminal_count_label);
            obj.append(&self.close_button);
        }
    }

    impl WidgetImpl for SessionRow {}
    impl BoxImpl for SessionRow {}
}

glib::wrapper! {
    pub struct SessionRow(ObjectSubclass<imp::SessionRow>)
        @extends gtk4::Box, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl SessionRow {
    pub fn new(uuid: &str, name: &str, terminal_count: usize) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().uuid.replace(uuid.to_string());
        obj.imp().name.replace(name.to_string());
        obj.imp().label.set_label(name);
        obj.update_terminal_count(terminal_count);
        obj
    }

    pub fn uuid(&self) -> String {
        self.imp().uuid.borrow().clone()
    }

    pub fn session_name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub fn set_session_name(&self, name: &str) {
        self.imp().name.replace(name.to_string());
        self.imp().label.set_label(name);
    }

    pub fn update_terminal_count(&self, count: usize) {
        self.imp()
            .terminal_count_label
            .set_label(&format!("{}", count));
    }

    pub fn close_button(&self) -> &gtk4::Button {
        &self.imp().close_button
    }
}
