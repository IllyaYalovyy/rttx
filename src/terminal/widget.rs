use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use vte4::prelude::*;

use crate::color_scheme;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default, Debug)]
    pub struct TerminalWidget {
        pub uuid: RefCell<String>,
        pub custom_title: RefCell<Option<String>>,
        pub initial_cwd: RefCell<Option<String>>,
        pub shell_spawned: Cell<bool>,
        pub smart_clipboard: Cell<bool>,
        pub pending_shell_inputs: RefCell<Vec<String>>,
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

            self.header.set_orientation(gtk4::Orientation::Horizontal);
            self.header.set_spacing(4);
            self.header.add_css_class("terminal-header");

            self.title_label.set_hexpand(true);
            self.title_label.set_xalign(0.0);
            self.title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            self.title_label.set_label("Terminal");

            self.split_h_button.set_icon_name("object-flip-horizontal-symbolic");
            self.split_h_button.add_css_class("flat");
            self.split_h_button.set_tooltip_text(Some("Split horizontally"));

            self.split_v_button.set_icon_name("object-flip-vertical-symbolic");
            self.split_v_button.add_css_class("flat");
            self.split_v_button.set_tooltip_text(Some("Split vertically"));

            self.close_button.set_icon_name("window-close-symbolic");
            self.close_button.add_css_class("flat");
            self.close_button.set_tooltip_text(Some("Close terminal"));

            self.header.append(&self.title_label);
            self.header.append(&self.split_h_button);
            self.header.append(&self.split_v_button);
            self.header.append(&self.close_button);

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

                        label.set_visible(false);
                        header.prepend(&entry);
                        entry.grab_focus();

                        let header2 = header.clone();
                        let label2 = label.clone();
                        let commit = move |entry: &gtk4::Entry| {
                            let text = entry.text().to_string();
                            if !text.is_empty() {
                                label2.set_label(&text);
                                obj.imp().custom_title.replace(Some(text));
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

            self.search_entry.set_hexpand(true);
            self.search_bar.set_child(Some(&self.search_entry));
            self.search_bar.set_show_close_button(true);

            self.vte.set_hexpand(true);
            self.vte.set_vexpand(true);
            self.vte.set_scroll_on_output(false);
            self.vte.set_scroll_on_keystroke(true);
            self.vte.set_scrollback_lines(10000);

            let key_controller = gtk4::EventControllerKey::new();
            key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let vte = self.vte.clone();
            let smart_clipboard = self.smart_clipboard.clone();
            key_controller.connect_key_pressed(move |_, key, _, modifiers| {
                match smart_clipboard_action(key, modifiers, vte.has_selection(), smart_clipboard.get()) {
                    SmartClipboardAction::Copy => {
                        vte.copy_clipboard_format(vte4::Format::Text);
                        glib::Propagation::Stop
                    }
                    SmartClipboardAction::Paste => {
                        vte.paste_clipboard();
                        glib::Propagation::Stop
                    }
                    SmartClipboardAction::PassThrough => glib::Propagation::Proceed,
                }
            });
            self.vte.add_controller(key_controller);

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
    #[must_use]
    pub fn new(uuid: &str, cwd: Option<&str>) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().uuid.replace(uuid.to_string());
        obj.imp().initial_cwd.replace(cwd.map(str::to_string));
        obj
    }

    #[must_use]
    pub fn uuid(&self) -> String {
        self.imp().uuid.borrow().clone()
    }

    #[must_use]
    pub fn vte(&self) -> &vte4::Terminal {
        &self.imp().vte
    }

    #[must_use]
    pub fn title_label(&self) -> &gtk4::Label {
        &self.imp().title_label
    }

    #[must_use]
    pub fn close_button(&self) -> &gtk4::Button {
        &self.imp().close_button
    }

    #[must_use]
    pub fn split_h_button(&self) -> &gtk4::Button {
        &self.imp().split_h_button
    }

    #[must_use]
    pub fn split_v_button(&self) -> &gtk4::Button {
        &self.imp().split_v_button
    }

    #[must_use]
    pub fn search_bar(&self) -> &gtk4::SearchBar {
        &self.imp().search_bar
    }

    #[must_use]
    pub fn search_entry(&self) -> &gtk4::SearchEntry {
        &self.imp().search_entry
    }

    pub fn set_title(&self, title: &str) {
        self.imp().title_label.set_label(title);
    }

    #[must_use]
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

    #[must_use]
    pub fn current_directory(&self) -> Option<String> {
        self.imp().vte.current_directory_uri().and_then(|uri| parse_file_uri(uri.as_str()))
    }

    pub fn set_smart_clipboard(&self, enabled: bool) {
        self.imp().smart_clipboard.set(enabled);
    }

    pub fn queue_input_for_shell(&self, input: impl Into<String>) {
        let input = input.into();
        if self.imp().shell_spawned.get() {
            self.imp().vte.feed_child(input.as_bytes());
            return;
        }
        self.imp().pending_shell_inputs.borrow_mut().push(input);
        self.ensure_shell_spawned_when_ready();
    }

    pub fn ensure_shell_spawned_when_ready(&self) {
        if self.imp().shell_spawned.get() {
            return;
        }
        if self.vte().width() > 0 && self.vte().height() > 0 {
            self.spawn_shell_once();
            return;
        }
        let term = self.clone();
        self.add_tick_callback(move |_, _| {
            if term.vte().width() > 0 && term.vte().height() > 0 {
                term.spawn_shell_once();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn spawn_shell(&self, cwd: Option<&str>) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let cwd_path = cwd.map(std::string::ToString::to_string);

        let vte = self.imp().vte.clone();
        let title_label = self.imp().title_label.clone();
        let custom_title = self.imp().custom_title.clone();

        vte.connect_window_title_changed(move |vte| {
            if custom_title.borrow().is_none() {
                if let Some(title) = vte.window_title() {
                    title_label.set_label(&title);
                }
            }
        });

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

    fn spawn_shell_once(&self) {
        if self.imp().shell_spawned.replace(true) {
            return;
        }
        if std::env::var_os("RTTX_DISABLE_SHELL_SPAWN").is_none() {
            let cwd = self.imp().initial_cwd.borrow().clone();
            self.spawn_shell(cwd.as_deref());
        }
        let pending_inputs: Vec<String> = self.imp().pending_shell_inputs.borrow_mut().drain(..).collect();
        for input in pending_inputs {
            self.imp().vte.feed_child(input.as_bytes());
        }
    }

    #[cfg(test)]
    pub(crate) fn shell_spawned_for_test(&self) -> bool {
        self.imp().shell_spawned.get()
    }

    #[cfg(test)]
    pub(crate) fn pending_shell_inputs_for_test(&self) -> Vec<String> {
        self.imp().pending_shell_inputs.borrow().clone()
    }

    /// Disconnect the `child_exited` signal handler to prevent re-entrancy
    /// panics when the terminal is dropped while a `RefCell` is borrowed.
    pub fn disconnect_child_exited(&self) {
        if let Some(id) = self.imp().child_exited_handler.borrow_mut().take() {
            self.imp().vte.disconnect(id);
        }
    }

    pub fn apply_color_scheme(&self, scheme: &color_scheme::ColorScheme) {
        let vte = &self.imp().vte;

        if let Some(fg) = scheme.foreground_rgba() {
            if let Some(bg) = scheme.background_rgba() {
                let palette = scheme.palette_rgba();
                let palette_refs: Vec<&gtk4::gdk::RGBA> = palette.iter().collect();
                vte.set_colors(Some(&fg), Some(&bg), &palette_refs);
            }
        }

        if scheme.use_cursor_color {
            if let Some(cursor_fg) = color_scheme::ColorScheme::parse_color(&scheme.cursor_fg) {
                vte.set_color_cursor_foreground(Some(&cursor_fg));
            }
            if let Some(cursor_bg) = color_scheme::ColorScheme::parse_color(&scheme.cursor_bg) {
                vte.set_color_cursor(Some(&cursor_bg));
            }
        }

        if scheme.use_highlight_color {
            if let Some(hl_fg) = color_scheme::ColorScheme::parse_color(&scheme.highlight_fg) {
                vte.set_color_highlight_foreground(Some(&hl_fg));
            }
            if let Some(hl_bg) = color_scheme::ColorScheme::parse_color(&scheme.highlight_bg) {
                vte.set_color_highlight(Some(&hl_bg));
            }
        }

        if scheme.use_bold_color {
            if let Some(bold) = color_scheme::ColorScheme::parse_color(&scheme.bold_color) {
                vte.set_color_bold(Some(&bold));
            }
        }
    }
}

pub(crate) fn parse_file_uri(uri: &str) -> Option<String> {
    glib::filename_from_uri(uri).ok().map(|(path, _hostname)| path.display().to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmartClipboardAction {
    Copy,
    Paste,
    PassThrough,
}

fn smart_clipboard_action(
    key: gtk4::gdk::Key,
    modifiers: gtk4::gdk::ModifierType,
    has_selection: bool,
    smart_clipboard_enabled: bool,
) -> SmartClipboardAction {
    if !smart_clipboard_enabled {
        return SmartClipboardAction::PassThrough;
    }

    let ignored_modifiers = gtk4::gdk::ModifierType::LOCK_MASK;
    let normalized = modifiers & !ignored_modifiers;
    if normalized != gtk4::gdk::ModifierType::CONTROL_MASK {
        return SmartClipboardAction::PassThrough;
    }

    match key {
        gtk4::gdk::Key::c | gtk4::gdk::Key::C if has_selection => SmartClipboardAction::Copy,
        gtk4::gdk::Key::v | gtk4::gdk::Key::V => SmartClipboardAction::Paste,
        _ => SmartClipboardAction::PassThrough,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_file_uri, smart_clipboard_action, SmartClipboardAction, TerminalWidget};
    use gtk4::prelude::*;
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

    fn pump_events(max_ms: u64) {
        let ctx = gtk4::glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
        while std::time::Instant::now() < deadline {
            if !ctx.iteration(false) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn parse_standard_vte_uri() {
        assert_eq!(
            parse_file_uri("file:///home/user/projects"),
            Some("/home/user/projects".into())
        );
    }

    #[test]
    fn parse_uri_with_percent_encoding() {
        assert_eq!(
            parse_file_uri("file:///home/user/my%20project"),
            Some("/home/user/my project".into())
        );
    }

    #[test]
    fn parse_uri_root() {
        assert_eq!(parse_file_uri("file:///"), Some("/".into()));
    }

    #[test]
    fn parse_non_file_uri_returns_none() {
        assert_eq!(parse_file_uri("https://example.com/path"), None);
        assert_eq!(parse_file_uri("ssh://host/path"), None);
    }

    #[test]
    fn parse_empty_string_returns_none() {
        assert_eq!(parse_file_uri(""), None);
    }

    #[test]
    fn strip_prefix_regression() {
        let old_way = "file:///home/user/my%20dir".strip_prefix("file://").map(|p| p.to_string());
        let new_way = parse_file_uri("file:///home/user/my%20dir");
        assert_eq!(old_way, Some("/home/user/my%20dir".into()));
        assert_eq!(new_way, Some("/home/user/my dir".into()));
        assert_ne!(old_way, new_way);
    }

    #[test]
    fn smart_clipboard_only_copies_selected_ctrl_c() {
        assert_eq!(
            smart_clipboard_action(
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                true,
                true,
            ),
            SmartClipboardAction::Copy
        );
        assert_eq!(
            smart_clipboard_action(
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            SmartClipboardAction::PassThrough
        );
    }

    #[test]
    fn smart_clipboard_paste_requires_plain_ctrl_v_and_opt_in() {
        assert_eq!(
            smart_clipboard_action(
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            SmartClipboardAction::Paste
        );
        assert_eq!(
            smart_clipboard_action(
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                true,
            ),
            SmartClipboardAction::PassThrough
        );
        assert_eq!(
            smart_clipboard_action(
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                false,
            ),
            SmartClipboardAction::PassThrough
        );
    }

    #[test]
    fn shell_spawn_waits_for_real_terminal_size() {
        require_display!();

        std::env::set_var("RTTX_DISABLE_SHELL_SPAWN", "1");

        let term = TerminalWidget::new("t1", Some("/tmp"));
        term.ensure_shell_spawned_when_ready();
        pump_events(50);
        assert!(!term.shell_spawned_for_test());

        let window = gtk4::Window::new();
        window.set_default_size(800, 600);
        window.set_child(Some(&term));
        window.present();
        pump_events(200);

        assert!(term.shell_spawned_for_test());

        window.close();
        std::env::remove_var("RTTX_DISABLE_SHELL_SPAWN");
    }
}
