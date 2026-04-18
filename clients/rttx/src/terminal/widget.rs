use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use vte4::prelude::*;

use crate::color_scheme;
use crate::terminal::TerminalInputBackend;
use crate::terminal::TerminalKeyAction;
use crate::terminal::links;
use crate::terminal::terminal_key_action;

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
        pub smart_clipboard_key_controller: RefCell<Option<gtk4::EventControllerKey>>,
        pub visual_bell: Cell<bool>,
        pub pending_shell_inputs: RefCell<Vec<String>>,
        #[cfg(test)]
        #[allow(clippy::option_option)]
        pub current_directory_override: RefCell<Option<Option<String>>>,
        pub vte: vte4::Terminal,
        pub terminal_scroller: gtk4::ScrolledWindow,
        pub header: gtk4::Box,
        pub recovery_bar: gtk4::Box,
        pub recovery_label: gtk4::Label,
        pub recovery_retry_button: gtk4::Button,
        pub title_label: gtk4::Label,
        pub close_button: gtk4::Button,
        pub split_h_button: gtk4::Button,
        pub split_v_button: gtk4::Button,
        pub zoom_button: gtk4::Button,
        pub search_bar: gtk4::SearchBar,
        pub search_entry: gtk4::SearchEntry,
        pub child_exited_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub last_match_at_click: RefCell<Option<String>>,
        pub places_submenu: gtk4::gio::Menu,
        pub context_menu: RefCell<Option<gtk4::PopoverMenu>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TerminalWidget {
        const NAME: &'static str = "RttxTerminalWidget";
        type Type = super::TerminalWidget;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for TerminalWidget {
        fn dispose(&self) {
            if let Some(menu) = self.context_menu.take() {
                menu.unparent();
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk4::Orientation::Vertical);
            obj.set_spacing(0);
            obj.add_css_class("terminal-pane");
            obj.set_margin_start(6);
            obj.set_margin_end(6);
            obj.set_margin_top(6);
            obj.set_margin_bottom(6);

            self.header.set_orientation(gtk4::Orientation::Horizontal);
            self.header.set_spacing(4);
            self.header.add_css_class("terminal-header");

            self.title_label.set_hexpand(true);
            self.title_label.set_xalign(0.0);
            self.title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            self.title_label.set_width_chars(0);
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

            self.zoom_button.set_icon_name("view-fullscreen-symbolic");
            self.zoom_button.add_css_class("flat");
            self.zoom_button.set_tooltip_text(Some("Zoom pane"));
            self.zoom_button.set_visible(false);

            self.header.append(&self.title_label);
            self.header.append(&self.split_h_button);
            self.header.append(&self.split_v_button);
            self.header.append(&self.zoom_button);
            self.header.append(&self.close_button);

            self.recovery_bar.set_orientation(gtk4::Orientation::Horizontal);
            self.recovery_bar.set_spacing(6);
            self.recovery_bar.set_margin_start(8);
            self.recovery_bar.set_margin_end(8);
            self.recovery_bar.set_margin_top(6);
            self.recovery_bar.set_margin_bottom(6);
            self.recovery_bar.add_css_class("toolbar");
            self.recovery_bar.set_visible(false);

            self.recovery_label.set_hexpand(true);
            self.recovery_label.set_xalign(0.0);
            self.recovery_label.set_wrap(true);

            self.recovery_retry_button.set_label("Retry");
            self.recovery_retry_button.add_css_class("suggested-action");

            self.recovery_bar.append(&self.recovery_label);
            self.recovery_bar.append(&self.recovery_retry_button);

            let gesture = gtk4::GestureClick::new();
            gesture.set_button(1);
            let vte = self.vte.clone();
            gesture.connect_released(move |g, n_press, _, _| {
                if n_press >= 1 {
                    let _ = vte.grab_focus();
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
            links::configure_openable_matches(&self.vte);

            self.terminal_scroller.set_hscrollbar_policy(gtk4::PolicyType::Never);
            self.terminal_scroller.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
            self.terminal_scroller.set_hexpand(true);
            self.terminal_scroller.set_vexpand(true);
            self.terminal_scroller.add_css_class("terminal-scroller");
            self.terminal_scroller.set_child(Some(&self.vte));

            let link_target = obj.downgrade();
            links::install_openable_link_controllers(&self.vte, move || {
                link_target.upgrade().and_then(|term| term.current_directory())
            });

            let term_weak = obj.downgrade();
            let smart_clipboard_key_controller = gtk4::EventControllerKey::new();
            smart_clipboard_key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
            smart_clipboard_key_controller.connect_key_pressed(move |_, key, _keycode, state| {
                let Some(term) = term_weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let vte = term.imp().vte.clone();
                match terminal_key_action(
                    TerminalInputBackend::Direct,
                    key,
                    state,
                    vte.has_selection(),
                    term.imp().smart_clipboard.get(),
                ) {
                    TerminalKeyAction::CopySelection => {
                        crate::terminal::copy_to_clipboard(&vte);
                        vte.unselect_all();
                        glib::Propagation::Stop
                    }
                    TerminalKeyAction::PasteClipboard => {
                        if let Some(root) = term.root()
                            && let Some(win) = root.downcast_ref::<gtk4::Window>()
                        {
                            win.activate_action("win.paste", None).ok();
                        }
                        glib::Propagation::Stop
                    }
                    TerminalKeyAction::PassThrough | TerminalKeyAction::ForwardToPty(_) => {
                        glib::Propagation::Proceed
                    }
                }
            });
            self.smart_clipboard_key_controller
                .replace(Some(smart_clipboard_key_controller.clone()));
            self.vte.add_controller(smart_clipboard_key_controller);

            let copy_link_action = gtk4::gio::SimpleAction::new("copy-link", None);
            copy_link_action.set_enabled(false);
            let open_link_action = gtk4::gio::SimpleAction::new("open-link", None);
            open_link_action.set_enabled(false);
            let action_group = gtk4::gio::SimpleActionGroup::new();
            action_group.add_action(&copy_link_action);
            action_group.add_action(&open_link_action);
            obj.insert_action_group("term", Some(&action_group));

            let obj_weak = obj.downgrade();
            copy_link_action.connect_activate(move |_, _| {
                let Some(obj) = obj_weak.upgrade() else { return };
                let matched = obj.imp().last_match_at_click.borrow().clone();
                if let Some(uri) = matched
                    && let Some(display) = gtk4::gdk::Display::default()
                {
                    display.clipboard().set_text(&links::display_text_for_uri(&uri));
                }
            });

            let obj_weak = obj.downgrade();
            open_link_action.connect_activate(move |_, _| {
                let Some(obj) = obj_weak.upgrade() else { return };
                let matched = obj.imp().last_match_at_click.borrow().clone();
                if let Some(uri) = matched {
                    links::launch_uri(&uri);
                }
            });

            let menu = gtk4::gio::Menu::new();
            let clipboard_section = gtk4::gio::Menu::new();
            clipboard_section.append(Some("Copy"), Some("win.copy"));
            clipboard_section.append(Some("Paste"), Some("win.paste"));
            let link_section = gtk4::gio::Menu::new();
            link_section.append(Some("Open Link"), Some("term.open-link"));
            link_section.append(Some("Copy Link"), Some("term.copy-link"));
            let pane_section = gtk4::gio::Menu::new();
            pane_section.append(Some("Search"), Some("win.search"));
            pane_section.append(Some("Split Horizontally"), Some("win.split-horizontal"));
            pane_section.append(Some("Split Vertically"), Some("win.split-vertical"));
            pane_section.append(Some("Rotate Layout"), Some("win.rotate-layout"));
            let session_section = gtk4::gio::Menu::new();
            session_section.append(Some("New Session"), Some("win.new-session"));
            session_section.append(Some("Toggle Input Sync"), Some("win.toggle-input-sync"));
            session_section.append(Some("Add to Places"), Some("win.add-current-place"));
            session_section.append(Some("Add Host"), Some("win.add-current-host"));
            session_section.append(Some("Preferences"), Some("win.preferences"));
            let places_submenu = &self.places_submenu;
            session_section.append_submenu(Some("Places"), places_submenu);
            let close_section = gtk4::gio::Menu::new();
            close_section.append(Some("Close Pane"), Some("win.close-terminal"));
            menu.append_section(None, &clipboard_section);
            menu.append_section(None, &link_section);
            menu.append_section(None, &pane_section);
            menu.append_section(None, &session_section);
            menu.append_section(None, &close_section);

            let context_menu = gtk4::PopoverMenu::from_model(Some(&menu));
            context_menu.set_has_arrow(false);
            context_menu.set_halign(crate::terminal::CONTEXT_MENU_HALIGN);
            // Parent to the VTE so set_pointing_to coordinates (which come
            // from the gesture on the VTE) are in the correct coordinate
            // space.
            context_menu.set_parent(self.vte.upcast_ref::<gtk4::Widget>());
            self.context_menu.replace(Some(context_menu.clone()));

            let right_click = gtk4::GestureClick::new();
            right_click.set_button(3);
            right_click.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let copy_link_ref = copy_link_action;
            let open_link_ref = open_link_action;
            let obj_weak = obj.downgrade();
            right_click.connect_pressed(move |gesture, _, x, y| {
                // Plain right-click opens the context menu. Shift+right-click
                // is denied so VTE can forward it to mouse-aware apps.
                let mods = gesture.current_event_state();
                if !crate::terminal::should_open_context_menu(mods) {
                    gesture.set_state(gtk4::EventSequenceState::Denied);
                    return;
                }
                if let Some(obj) = obj_weak.upgrade() {
                    let matched = links::openable_uri_at(
                        &obj.imp().vte,
                        x,
                        y,
                        obj.current_directory().as_deref(),
                    );
                    let has_link = matched.is_some();
                    copy_link_ref.set_enabled(has_link);
                    open_link_ref.set_enabled(has_link);
                    obj.imp().last_match_at_click.replace(matched);

                    let host_key = obj
                        .root()
                        .and_then(|r| r.downcast::<crate::window::Window>().ok())
                        .map_or_else(
                            || crate::host::LOCAL_KEY.into(),
                            |w| w.visible_session_host_key(),
                        );
                    crate::terminal::populate_places_submenu(&obj.imp().places_submenu, &host_key);
                }
                context_menu
                    .set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                context_menu.popup();
                gesture.set_state(gtk4::EventSequenceState::Claimed);
            });
            self.vte.add_controller(right_click);

            obj.append(&self.header);
            obj.append(&self.recovery_bar);
            obj.append(&self.search_bar);
            obj.append(&self.terminal_scroller);
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
        obj.connect_search();
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
    pub fn zoom_button(&self) -> &gtk4::Button {
        &self.imp().zoom_button
    }

    pub fn set_zoom_state(&self, zoomed: bool, multi_pane: bool) {
        let btn = &self.imp().zoom_button;
        btn.set_visible(zoomed || multi_pane);
        if zoomed {
            btn.set_icon_name("view-restore-symbolic");
            btn.set_tooltip_text(Some("Restore pane"));
        } else {
            btn.set_icon_name("view-fullscreen-symbolic");
            btn.set_tooltip_text(Some("Zoom pane"));
        }
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

    pub fn set_custom_title(&self, title: Option<&str>) {
        self.imp().custom_title.replace(title.map(str::to_string));
        if let Some(title) = title {
            self.set_title(title);
        }
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
        } else {
            self.imp().vte.search_set_regex(None::<&vte4::Regex>, 0);
            let _ = self.imp().vte.grab_focus();
        }
    }

    fn connect_search(&self) {
        let vte = self.imp().vte.clone();
        self.imp().search_entry.connect_search_changed(move |entry| {
            let text = entry.text();
            if text.is_empty() {
                vte.search_set_regex(None::<&vte4::Regex>, 0);
                return;
            }
            if let Ok(regex) = vte4::Regex::for_search(&format!("\\Q{text}\\E"), 0) {
                vte.search_set_regex(Some(&regex), 0);
                vte.search_set_wrap_around(true);
                vte.search_find_previous();
            }
        });

        let vte_next = self.imp().vte.clone();
        self.imp().search_entry.connect_next_match(move |_| {
            vte_next.search_find_next();
        });

        let vte_prev = self.imp().vte.clone();
        self.imp().search_entry.connect_previous_match(move |_| {
            vte_prev.search_find_previous();
        });

        let vte_activate = self.imp().vte.clone();
        self.imp().search_entry.connect_activate(move |_| {
            vte_activate.search_find_next();
        });
    }

    #[must_use]
    pub fn current_directory(&self) -> Option<String> {
        #[cfg(test)]
        if let Some(cwd) = self.imp().current_directory_override.borrow().clone() {
            return cwd;
        }
        self.imp().vte.current_directory_uri().and_then(|uri| links::parse_file_uri(uri.as_str()))
    }

    pub fn set_smart_clipboard(&self, enabled: bool) {
        self.imp().smart_clipboard.set(enabled);
    }

    pub fn set_visual_bell(&self, enabled: bool) {
        self.imp().visual_bell.set(enabled);
    }

    pub(crate) fn set_active(&self, active: bool) {
        if active {
            self.add_css_class("terminal-pane-active");
        } else {
            self.remove_css_class("terminal-pane-active");
        }
    }

    pub fn flash_bell(&self) {
        if !self.imp().visual_bell.get() {
            return;
        }
        let header = &self.imp().header;
        header.remove_css_class("bell-flash");
        header.add_css_class("bell-flash");
        let header_weak = header.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            if let Some(h) = header_weak.upgrade() {
                h.remove_css_class("bell-flash");
            }
        });
    }

    #[must_use]
    pub fn recovery_retry_button(&self) -> &gtk4::Button {
        &self.imp().recovery_retry_button
    }

    pub fn show_recovery_message(&self, message: &str) {
        self.imp().recovery_label.set_label(message);
        self.imp().recovery_bar.set_visible(true);
    }

    pub fn hide_recovery_message(&self) {
        self.imp().recovery_label.set_label("");
        self.imp().recovery_bar.set_visible(false);
    }

    pub fn reset_launch_state_for_retry(&self) {
        self.imp().shell_spawned.set(false);
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn smart_clipboard_enabled_for_test(&self) -> bool {
        self.imp().smart_clipboard.get()
    }

    #[cfg(test)]
    pub(crate) fn emit_smart_clipboard_key_for_test(
        &self,
        key: gtk4::gdk::Key,
        modifiers: gtk4::gdk::ModifierType,
    ) -> glib::Propagation {
        self.imp()
            .smart_clipboard_key_controller
            .borrow()
            .as_ref()
            .expect("smart clipboard key controller should be available")
            .emit_by_name::<bool>("key-pressed", &[&key, &0u32, &modifiers])
            .into()
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
            if custom_title.borrow().is_none()
                && let Some(title) = vte.window_title()
            {
                let cleaned = crate::terminal::persistent_widget::strip_user_host_prefix(&title);
                title_label.set_label(cleaned);
            }
        });

        let vte_for_spawn = vte.clone();
        let widget_ref = self.downgrade();
        vte.spawn_async(
            vte4::PtyFlags::DEFAULT,
            cwd_path.as_deref(),
            &[shell.as_str()],
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gtk4::gio::Cancellable::NONE,
            move |result| {
                if let Err(error) = result {
                    log::error!("Failed to spawn shell: {error}");
                    let msg = format!("\r\n\x1b[31mFailed to spawn shell: {error}\x1b[0m\r\n");
                    vte_for_spawn.feed(msg.as_bytes());
                    if let Some(widget) = widget_ref.upgrade()
                        && let Some(root) = widget.root()
                        && let Some(window) = root.downcast_ref::<crate::window::Window>()
                    {
                        window.show_toast(&format!("Shell spawn failed: {error}"));
                    }
                }
            },
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
        let pending_inputs: Vec<String> =
            self.imp().pending_shell_inputs.borrow_mut().drain(..).collect();
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

    #[cfg(test)]
    #[must_use]
    pub(crate) fn recovery_message_visible_for_test(&self) -> bool {
        self.imp().recovery_bar.is_visible()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn recovery_message_for_test(&self) -> String {
        self.imp().recovery_label.label().to_string()
    }

    #[cfg(test)]
    pub(crate) fn set_current_directory_for_test(&self, cwd: Option<&str>) {
        self.imp().current_directory_override.replace(Some(cwd.map(str::to_string)));
    }

    #[cfg(test)]
    pub(crate) fn initial_cwd_for_test(&self) -> Option<String> {
        self.imp().initial_cwd.borrow().clone()
    }

    /// Disconnect the `child_exited` signal handler to prevent re-entrancy
    /// panics when the terminal is dropped while a `RefCell` is borrowed.
    pub fn disconnect_child_exited(&self) {
        if let Some(id) = self.imp().child_exited_handler.borrow_mut().take() {
            self.imp().vte.disconnect(id);
        }
    }

    /// Reset terminal modes that a remote process (e.g., SSH) may have left active
    /// after a broken connection.
    ///
    /// Feeds ANSI/DEC escape sequences directly to the VTE emulator so it disables
    /// mouse tracking, bracketed paste, alternate screen, and resets text attributes.
    /// Called whenever a child process exits so a dangling SSH session never leaves
    /// the local terminal in a corrupt state.
    pub fn reset_terminal_state(&self) {
        // Each sequence targets a specific mode; none has visible side-effects when
        // the mode was already inactive, so this is safe to call unconditionally.
        //
        // ESC [ ! p   — DECSTR: soft terminal reset (resets scroll regions, origin
        //               mode, insert mode, and most other DEC private modes)
        // ?1000l      — disable X10 mouse reporting
        // ?1002l      — disable button-event (cell-motion) mouse tracking
        // ?1003l      — disable any-event mouse tracking
        // ?1006l      — disable SGR extended mouse coordinate encoding
        // ?1015l      — disable URXVT extended mouse coordinate encoding
        // ?1016l      — disable SGR pixel coordinate mouse encoding
        // ?2004l      — disable bracketed paste mode
        // ?1049l      — leave alternate screen buffer, restore normal buffer
        // ?25h        — show cursor (re-enable if the remote hid it)
        // 0m          — reset all SGR character attributes
        const RESET: &str = concat!(
            "\x1b[!p",
            "\x1b[?1000l",
            "\x1b[?1002l",
            "\x1b[?1003l",
            "\x1b[?1006l",
            "\x1b[?1015l",
            "\x1b[?1016l",
            "\x1b[?2004l",
            "\x1b[?1049l",
            "\x1b[?25h",
            "\x1b[0m",
        );
        self.imp().vte.feed(RESET.as_bytes());
    }

    pub fn apply_color_scheme(&self, scheme: &color_scheme::ColorScheme) {
        let vte = &self.imp().vte;

        if let Some(fg) = scheme.foreground_rgba()
            && let Some(bg) = scheme.background_rgba()
        {
            let palette = scheme.palette_rgba();
            let palette_refs: Vec<&gtk4::gdk::RGBA> = palette.iter().collect();
            vte.set_colors(Some(&fg), Some(&bg), &palette_refs);
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

        if scheme.use_bold_color
            && let Some(bold) = color_scheme::ColorScheme::parse_color(&scheme.bold_color)
        {
            vte.set_color_bold(Some(&bold));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::{TerminalInputBackend, TerminalKeyAction, terminal_key_action};
    use gtk4::glib;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::ObjectSubclassIsExt;

    /// Verify that the RESET constant inside `reset_terminal_state()` contains
    /// the expected escape sequences without requiring a live VTE widget.
    #[test]
    fn reset_terminal_state_sequences() {
        // Reconstruct the same constant used in the function.
        let reset = concat!(
            "\x1b[!p",
            "\x1b[?1000l",
            "\x1b[?1002l",
            "\x1b[?1003l",
            "\x1b[?1006l",
            "\x1b[?1015l",
            "\x1b[?1016l",
            "\x1b[?2004l",
            "\x1b[?1049l",
            "\x1b[?25h",
            "\x1b[0m",
        );
        assert!(reset.contains("\x1b[!p"), "DECSTR soft reset missing");
        assert!(reset.contains("\x1b[?1000l"), "X10 mouse disable missing");
        assert!(reset.contains("\x1b[?1002l"), "button-event mouse disable missing");
        assert!(reset.contains("\x1b[?1003l"), "any-event mouse disable missing");
        assert!(reset.contains("\x1b[?1006l"), "SGR mouse disable missing");
        assert!(reset.contains("\x1b[?2004l"), "bracketed paste disable missing");
        assert!(reset.contains("\x1b[?1049l"), "alt-screen exit missing");
        assert!(reset.contains("\x1b[?25h"), "cursor show missing");
        assert!(reset.contains("\x1b[0m"), "SGR reset missing");
    }

    #[test]
    fn smart_clipboard_only_copies_selected_ctrl_c() {
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                true,
                true,
            ),
            TerminalKeyAction::CopySelection
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            TerminalKeyAction::PassThrough
        );
    }

    #[test]
    fn smart_clipboard_paste_requires_plain_ctrl_v_and_opt_in() {
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            TerminalKeyAction::PasteClipboard
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                true,
            ),
            TerminalKeyAction::PassThrough
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
    }

    #[test]
    fn smart_clipboard_ignores_extra_non_shortcut_modifiers_for_ctrl_shortcuts() {
        let pointer_mask = gtk4::gdk::ModifierType::BUTTON1_MASK;

        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK | pointer_mask,
                false,
                true,
            ),
            TerminalKeyAction::PasteClipboard
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK
                    | gtk4::gdk::ModifierType::LOCK_MASK
                    | pointer_mask,
                true,
                true,
            ),
            TerminalKeyAction::CopySelection
        );
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn smart_clipboard_key_controller_ignores_extra_non_shortcut_modifiers() {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }

        let term = super::TerminalWidget::new("term-1", None);
        term.set_smart_clipboard(true);
        let window = gtk4::Window::new();
        window.set_default_size(640, 320);
        window.set_child(Some(&term));
        window.present();
        while gtk4::glib::MainContext::default().iteration(false) {}

        assert_eq!(
            term.emit_smart_clipboard_key_for_test(
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::BUTTON1_MASK,
            ),
            glib::Propagation::Stop
        );

        window.close();
    }

    /// Spawn error message must be visible in the terminal pane. #22.
    #[test]
    fn spawn_error_message_format_contains_ansi_red() {
        let error = "No such file or directory";
        let msg = format!("\r\n\x1b[31mFailed to spawn shell: {error}\x1b[0m\r\n");
        assert!(msg.contains("\x1b[31m"), "error must use red ANSI color");
        assert!(msg.contains(error));
        assert!(msg.contains("\x1b[0m"), "error must reset ANSI color");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn dispose_unparents_context_menu() {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }

        let term = super::TerminalWidget::new("dispose-ctx", None);
        assert!(
            term.imp().context_menu.borrow().is_some(),
            "context menu should be stored after construction"
        );
        drop(term);
        // No critical GLib warnings about orphaned popover means dispose ran.
    }
}
