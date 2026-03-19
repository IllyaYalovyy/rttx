use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use vte4::prelude::*;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct TerminalWidget {
        pub uuid: RefCell<String>,
        pub custom_title: RefCell<Option<String>>,
        pub vte: vte4::Terminal,
        pub header: gtk4::Box,
        pub title_label: gtk4::Label,
        pub close_button: gtk4::Button,
        pub split_h_button: gtk4::Button,
        pub split_v_button: gtk4::Button,
        pub search_bar: gtk4::SearchBar,
        pub search_entry: gtk4::SearchEntry,
        pub child_exited_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TerminalWidget {
        const NAME: &'static str = "RttxTerminalWidget";
        type Type = super::TerminalWidget;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for TerminalWidget {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk4::Orientation::Vertical);
            obj.set_spacing(0);

            // Header bar
            self.header.set_orientation(gtk4::Orientation::Horizontal);
            self.header.set_spacing(4);
            self.header.add_css_class("terminal-header");

            self.title_label.set_hexpand(true);
            self.title_label.set_xalign(0.0);
            self.title_label
                .set_ellipsize(gtk4::pango::EllipsizeMode::End);
            self.title_label.set_label("Terminal");

            self.split_h_button
                .set_icon_name("object-flip-horizontal-symbolic");
            self.split_h_button.add_css_class("flat");
            self.split_h_button
                .set_tooltip_text(Some("Split horizontally"));

            self.split_v_button
                .set_icon_name("object-flip-vertical-symbolic");
            self.split_v_button.add_css_class("flat");
            self.split_v_button
                .set_tooltip_text(Some("Split vertically"));

            self.close_button.set_icon_name("window-close-symbolic");
            self.close_button.add_css_class("flat");
            self.close_button.set_tooltip_text(Some("Close terminal"));

            self.header.append(&self.title_label);
            self.header.append(&self.split_h_button);
            self.header.append(&self.split_v_button);
            self.header.append(&self.close_button);

            // Double-click title label to edit custom title
            let gesture = gtk4::GestureClick::new();
            gesture.set_button(1);
            let header = self.header.clone();
            let label = self.title_label.clone();
            let obj_weak = obj.downgrade();
            gesture.connect_released(move |g, n_press, _, _| {
                if n_press == 2 {
                    if let Some(obj) = obj_weak.upgrade() {
                        let entry = gtk4::Entry::new();
                        entry.set_text(&label.label());
                        entry.set_hexpand(true);

                        // Replace label with entry
                        label.set_visible(false);
                        header.prepend(&entry);
                        entry.grab_focus();

                        let header2 = header.clone();
                        let label2 = label.clone();
                        let obj2 = obj.clone();
                        let commit = move |entry: &gtk4::Entry| {
                            let text = entry.text().to_string();
                            if !text.is_empty() {
                                label2.set_label(&text);
                                obj2.imp().custom_title.replace(Some(text));
                            }
                            label2.set_visible(true);
                            header2.remove(entry);
                        };

                        let commit2 = commit.clone();
                        entry.connect_activate(move |e| commit2(e));

                        let focus_ctrl = gtk4::EventControllerFocus::new();
                        let entry_ref = entry.clone();
                        focus_ctrl.connect_leave(move |_| commit(&entry_ref));
                        entry.add_controller(focus_ctrl);
                    }
                    g.set_state(gtk4::EventSequenceState::Claimed);
                }
            });
            self.title_label.add_controller(gesture);

            // Search bar
            self.search_entry.set_hexpand(true);
            self.search_bar.set_child(Some(&self.search_entry));
            self.search_bar.set_show_close_button(true);

            // VTE terminal
            self.vte.set_hexpand(true);
            self.vte.set_vexpand(true);
            self.vte.set_scroll_on_output(false);
            self.vte.set_scroll_on_keystroke(true);
            self.vte.set_scrollback_lines(10000);

            // Assemble
            obj.append(&self.header);
            obj.append(&self.search_bar);
            obj.append(&self.vte);
        }
    }

    impl WidgetImpl for TerminalWidget {}
    impl BoxImpl for TerminalWidget {}
}

glib::wrapper! {
    pub struct TerminalWidget(ObjectSubclass<imp::TerminalWidget>)
        @extends gtk4::Box, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl TerminalWidget {
    pub fn new(uuid: &str, cwd: Option<&str>) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().uuid.replace(uuid.to_string());
        obj.spawn_shell(cwd);
        obj
    }

    pub fn uuid(&self) -> String {
        self.imp().uuid.borrow().clone()
    }

    pub fn vte(&self) -> &vte4::Terminal {
        &self.imp().vte
    }

    pub fn title_label(&self) -> &gtk4::Label {
        &self.imp().title_label
    }

    pub fn close_button(&self) -> &gtk4::Button {
        &self.imp().close_button
    }

    pub fn split_h_button(&self) -> &gtk4::Button {
        &self.imp().split_h_button
    }

    pub fn split_v_button(&self) -> &gtk4::Button {
        &self.imp().split_v_button
    }

    pub fn search_bar(&self) -> &gtk4::SearchBar {
        &self.imp().search_bar
    }

    pub fn search_entry(&self) -> &gtk4::SearchEntry {
        &self.imp().search_entry
    }

    pub fn set_title(&self, title: &str) {
        self.imp().title_label.set_label(title);
    }

    /// Get the custom title, if set by the user.
    pub fn custom_title(&self) -> Option<String> {
        self.imp().custom_title.borrow().clone()
    }

    pub fn toggle_search(&self) {
        let bar = &self.imp().search_bar;
        bar.set_search_mode(!bar.is_search_mode());
        if bar.is_search_mode() {
            self.imp().search_entry.grab_focus();
        }
    }

    /// Get the current working directory from VTE, if available.
    pub fn current_directory(&self) -> Option<String> {
        self.imp().vte.current_directory_uri().and_then(|uri| {
            let uri_str = uri.as_str();
            uri_str.strip_prefix("file://").map(|p| p.to_string())
        })
    }

    fn spawn_shell(&self, cwd: Option<&str>) {
        let shell =
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let cwd_path = cwd.map(|s| s.to_string());

        let vte = self.imp().vte.clone();
        let title_label = self.imp().title_label.clone();
        let custom_title = self.imp().custom_title.clone();

        // Update title when VTE title changes (only if no custom title set)
        vte.connect_window_title_changed(move |vte| {
            if custom_title.borrow().is_none() {
                if let Some(title) = vte.window_title() {
                    title_label.set_label(&title);
                }
            }
        });

        // Spawn the shell
        vte.spawn_async(
            vte4::PtyFlags::DEFAULT,
            cwd_path.as_deref(),
            &[shell.as_str()],
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gtk4::gio::Cancellable::NONE,
            move |_result| {},
        );
    }

    /// Disconnect the child_exited signal handler to prevent re-entrancy
    /// panics when the terminal is dropped while a RefCell is borrowed.
    pub fn disconnect_child_exited(&self) {
        if let Some(id) = self.imp().child_exited_handler.borrow_mut().take() {
            self.imp().vte.disconnect(id);
        }
    }

    /// Apply a color scheme to this terminal.
    pub fn apply_color_scheme(&self, scheme: &rttx::color_scheme::ColorScheme) {
        let vte = &self.imp().vte;

        if let Some(fg) = scheme.foreground_rgba() {
            if let Some(bg) = scheme.background_rgba() {
                let palette = scheme.palette_rgba();
                let palette_refs: Vec<&gtk4::gdk::RGBA> = palette.iter().collect();
                vte.set_colors(Some(&fg), Some(&bg), &palette_refs);
            }
        }

        if scheme.use_cursor_color {
            if let Some(cursor_fg) =
                rttx::color_scheme::ColorScheme::parse_color(&scheme.cursor_fg)
            {
                vte.set_color_cursor_foreground(Some(&cursor_fg));
            }
            if let Some(cursor_bg) =
                rttx::color_scheme::ColorScheme::parse_color(&scheme.cursor_bg)
            {
                vte.set_color_cursor(Some(&cursor_bg));
            }
        }

        if scheme.use_highlight_color {
            if let Some(hl_fg) =
                rttx::color_scheme::ColorScheme::parse_color(&scheme.highlight_fg)
            {
                vte.set_color_highlight_foreground(Some(&hl_fg));
            }
            if let Some(hl_bg) =
                rttx::color_scheme::ColorScheme::parse_color(&scheme.highlight_bg)
            {
                vte.set_color_highlight(Some(&hl_bg));
            }
        }

        if scheme.use_bold_color {
            if let Some(bold) =
                rttx::color_scheme::ColorScheme::parse_color(&scheme.bold_color)
            {
                vte.set_color_bold(Some(&bold));
            }
        }
    }
}
