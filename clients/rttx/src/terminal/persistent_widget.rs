//! GTK widget for daemon-backed persistent terminal panes.
//!
//! Uses a `vte4::Terminal` in feed mode — no local PTY. Terminal output
//! arrives as `Delta` messages from `rttx-server` and is fed into VTE for
//! rendering. Keyboard input is captured and sent back to the daemon.

use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use vte4::prelude::*;

use crate::color_scheme;
use crate::runtime::{ConnectionPresentation, ConnectionStatus};
use crate::terminal::TerminalInputBackend;
use crate::terminal::TerminalKeyAction;
use crate::terminal::links;
use crate::terminal::terminal_key_action;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Debug)]
    pub struct PersistentPaneView {
        pub uuid: RefCell<String>,
        pub session_id: RefCell<String>,
        pub custom_title: RefCell<Option<String>>,
        pub current_directory: RefCell<Option<String>>,
        pub smart_clipboard: Rc<Cell<bool>>,
        pub visual_bell: Cell<bool>,
        pub connected: Cell<bool>,
        pub accepts_input: Cell<bool>,
        pub input_key_controller: RefCell<Option<gtk4::EventControllerKey>>,
        pub vte: vte4::Terminal,
        pub terminal_scroller: gtk4::ScrolledWindow,
        pub header: gtk4::Box,
        pub connection_banner: gtk4::Box,
        pub connection_title_label: gtk4::Label,
        pub connection_body_label: gtk4::Label,
        pub title_label: gtk4::Label,
        pub close_button: gtk4::Button,
        pub split_h_button: gtk4::Button,
        pub split_v_button: gtk4::Button,
        pub status_label: gtk4::Label,
        pub search_bar: gtk4::SearchBar,
        pub search_entry: gtk4::SearchEntry,
    }

    impl Default for PersistentPaneView {
        fn default() -> Self {
            Self {
                uuid: RefCell::default(),
                session_id: RefCell::default(),
                custom_title: RefCell::default(),
                current_directory: RefCell::default(),
                smart_clipboard: Rc::new(Cell::new(false)),
                visual_bell: Cell::default(),
                connected: Cell::default(),
                accepts_input: Cell::default(),
                input_key_controller: RefCell::default(),
                vte: vte4::Terminal::new(),
                terminal_scroller: gtk4::ScrolledWindow::new(),
                header: gtk4::Box::default(),
                connection_banner: gtk4::Box::default(),
                connection_title_label: gtk4::Label::default(),
                connection_body_label: gtk4::Label::default(),
                title_label: gtk4::Label::default(),
                close_button: gtk4::Button::default(),
                split_h_button: gtk4::Button::default(),
                split_v_button: gtk4::Button::default(),
                status_label: gtk4::Label::default(),
                search_bar: gtk4::SearchBar::default(),
                search_entry: gtk4::SearchEntry::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PersistentPaneView {
        const NAME: &'static str = "RttxPersistentPaneView";
        type Type = super::PersistentPaneView;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for PersistentPaneView {
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

            // Header bar — same structure as TerminalWidget.
            self.header.set_orientation(gtk4::Orientation::Horizontal);
            self.header.set_spacing(4);
            self.header.add_css_class("terminal-header");

            self.title_label.set_hexpand(true);
            self.title_label.set_xalign(0.0);
            self.title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            self.title_label.set_label("Terminal (persistent)");

            self.status_label.set_xalign(1.0);
            self.status_label.add_css_class("dim-label");
            self.status_label.set_label("⏻");
            self.status_label
                .set_tooltip_text(Some("Persistent workspace — daemon-backed runtime"));

            self.split_h_button.set_icon_name("object-flip-horizontal-symbolic");
            self.split_h_button.add_css_class("flat");
            self.split_h_button.set_tooltip_text(Some("Split horizontally"));

            self.split_v_button.set_icon_name("object-flip-vertical-symbolic");
            self.split_v_button.add_css_class("flat");
            self.split_v_button.set_tooltip_text(Some("Split vertically"));

            self.close_button.set_icon_name("window-close-symbolic");
            self.close_button.add_css_class("flat");
            self.close_button.set_tooltip_text(Some("Close pane"));

            self.header.append(&self.title_label);
            self.header.append(&self.status_label);
            self.header.append(&self.split_h_button);
            self.header.append(&self.split_v_button);
            self.header.append(&self.close_button);

            // VTE in feed mode — no PTY spawned.
            self.vte.set_hexpand(true);
            self.vte.set_vexpand(true);
            self.vte.set_scroll_on_output(false);
            self.vte.set_scroll_on_keystroke(true);
            self.vte.set_scrollback_lines(10000);
            self.vte.set_input_enabled(false);
            links::configure_openable_matches(&self.vte);
            let link_target = obj.downgrade();
            links::install_openable_link_controllers(&self.vte, move || {
                link_target.upgrade().and_then(|pane| pane.current_directory())
            });

            self.search_entry.set_hexpand(true);
            self.search_bar.set_child(Some(&self.search_entry));
            self.search_bar.set_show_close_button(true);

            self.terminal_scroller.set_hscrollbar_policy(gtk4::PolicyType::Never);
            self.terminal_scroller.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
            self.terminal_scroller.set_hexpand(true);
            self.terminal_scroller.set_vexpand(true);
            self.terminal_scroller.add_css_class("terminal-scroller");
            self.terminal_scroller.set_child(Some(&self.vte));

            obj.append(&self.header);
            self.connection_banner.set_orientation(gtk4::Orientation::Vertical);
            self.connection_banner.set_spacing(6);
            self.connection_banner.set_margin_start(8);
            self.connection_banner.set_margin_end(8);
            self.connection_banner.set_margin_top(6);
            self.connection_banner.set_margin_bottom(6);
            self.connection_banner.add_css_class("toolbar");
            self.connection_banner.set_visible(false);

            self.connection_title_label.set_xalign(0.0);
            self.connection_title_label.add_css_class("heading");

            self.connection_body_label.set_xalign(0.0);
            self.connection_body_label.set_wrap(true);
            self.connection_body_label.add_css_class("dim-label");

            self.connection_banner.append(&self.connection_title_label);
            self.connection_banner.append(&self.connection_body_label);

            obj.append(&self.connection_banner);
            obj.append(&self.search_bar);
            obj.append(&self.terminal_scroller);

            // Focus VTE on header click.
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
        }
    }

    impl WidgetImpl for PersistentPaneView {}
    impl BoxImpl for PersistentPaneView {}
}

glib::wrapper! {
    /// A terminal pane backed by the `rttx-server` daemon.
    ///
    /// Unlike `TerminalWidget`, this widget does not own a PTY. Terminal
    /// output is fed via `feed_output()` from daemon Delta messages, and
    /// keyboard input is captured via `connect_input()` for sending back
    /// to the daemon.
    pub struct PersistentPaneView(ObjectSubclass<imp::PersistentPaneView>)
        @extends gtk4::Box, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl PersistentPaneView {
    /// Create a new persistent pane view.
    #[must_use]
    pub fn new(uuid: &str, session_id: &str) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().uuid.replace(uuid.to_string());
        obj.imp().session_id.replace(session_id.to_string());
        obj.connect_search();
        obj
    }

    /// The pane's UUID.
    #[must_use]
    pub fn uuid(&self) -> String {
        self.imp().uuid.borrow().clone()
    }

    /// Update the pane UUID used by the GTK-side session model.
    pub fn set_uuid(&self, uuid: &str) {
        self.imp().uuid.replace(uuid.to_string());
    }

    /// The daemon session UUID this pane belongs to.
    #[must_use]
    pub fn session_id(&self) -> String {
        self.imp().session_id.borrow().clone()
    }

    /// Update the runtime UUID this pane currently belongs to.
    pub fn set_session_id(&self, session_id: &str) {
        self.imp().session_id.replace(session_id.to_string());
    }

    /// The underlying VTE terminal widget.
    #[must_use]
    pub fn vte(&self) -> &vte4::Terminal {
        &self.imp().vte
    }

    /// The title label in the header.
    #[must_use]
    pub fn title_label(&self) -> &gtk4::Label {
        &self.imp().title_label
    }

    /// The close/detach button.
    #[must_use]
    pub fn close_button(&self) -> &gtk4::Button {
        &self.imp().close_button
    }

    /// The horizontal split button.
    #[must_use]
    pub fn split_h_button(&self) -> &gtk4::Button {
        &self.imp().split_h_button
    }

    /// The vertical split button.
    #[must_use]
    pub fn split_v_button(&self) -> &gtk4::Button {
        &self.imp().split_v_button
    }

    /// The header box (for drag source).
    #[must_use]
    pub fn header(&self) -> &gtk4::Box {
        &self.imp().header
    }

    /// Set the pane title.
    pub fn set_title(&self, title: &str) {
        self.imp().title_label.set_label(title);
    }

    /// Set a custom title that overrides the daemon-reported title.
    pub fn set_custom_title(&self, title: Option<&str>) {
        self.imp().custom_title.replace(title.map(str::to_string));
        if let Some(title) = title {
            self.set_title(title);
        }
    }

    /// Get the custom title, if set.
    #[must_use]
    pub fn custom_title(&self) -> Option<String> {
        self.imp().custom_title.borrow().clone()
    }

    /// Toggle the inline search UI.
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

    /// Current working directory reported by the runtime.
    #[must_use]
    pub fn current_directory(&self) -> Option<String> {
        self.imp().current_directory.borrow().clone()
    }

    /// Update the current working directory reported by the runtime.
    pub fn set_current_directory(&self, cwd: Option<&str>) {
        self.imp().current_directory.replace(cwd.map(str::to_string));
    }

    /// Feed raw terminal output bytes into VTE for rendering.
    ///
    /// Called when a `Delta` message arrives from the daemon.
    pub fn feed_output(&self, data: &[u8]) {
        self.imp().vte.feed(data);
    }

    /// Feed a snapshot's scrollback bytes into VTE to restore state on attach.
    /// Bell characters are stripped to prevent historical bells from ringing.
    pub fn feed_snapshot(&self, scrollback: &[u8]) {
        if scrollback.is_empty() {
            return;
        }
        if scrollback.contains(&0x07) {
            let filtered: Vec<u8> = scrollback.iter().copied().filter(|&b| b != 0x07).collect();
            self.imp().vte.feed(&filtered);
        } else {
            self.imp().vte.feed(scrollback);
        }
    }

    /// Update the connection status indicator.
    pub fn set_connected(&self, connected: bool) {
        self.imp().connected.set(connected);
        let label = if connected { "Connected" } else { "Disconnected" };
        self.imp().status_label.set_label(label);
        let tooltip = if connected { "Connected to daemon" } else { "Disconnected from daemon" };
        self.imp().status_label.set_tooltip_text(Some(tooltip));
    }

    /// Render a richer connection state in the pane header.
    pub fn set_connection_status(&self, status: &ConnectionStatus) {
        self.imp()
            .connected
            .set(matches!(status, ConnectionStatus::Connected | ConnectionStatus::Recovered));
        let label = status.short_label();
        self.imp().status_label.set_label(&label);
        self.imp().status_label.set_tooltip_text(Some(&status.label()));
    }

    /// Render the inline connection banner and update input availability.
    pub fn set_connection_presentation(
        &self,
        status: &ConnectionStatus,
        presentation: &ConnectionPresentation,
    ) {
        self.imp()
            .connected
            .set(matches!(status, ConnectionStatus::Connected | ConnectionStatus::Recovered));
        self.imp().status_label.set_label(&presentation.header_label);
        self.imp().status_label.set_tooltip_text(Some(&status.label()));
        self.imp().connection_title_label.set_label(&presentation.banner_title);
        self.imp().connection_body_label.set_label(&presentation.banner_body);
        self.imp().connection_banner.set_visible(presentation.banner_visible);
        self.imp().accepts_input.set(presentation.input_enabled);
    }

    /// Mark this pane as active (focused).
    pub fn set_active(&self, active: bool) {
        if active {
            self.add_css_class("terminal-pane-active");
        } else {
            self.remove_css_class("terminal-pane-active");
        }
    }

    /// Enable or disable smart clipboard behavior.
    pub fn set_smart_clipboard(&self, enabled: bool) {
        self.imp().smart_clipboard.set(enabled);
    }

    /// Enable or disable visual bell.
    pub fn set_visual_bell(&self, enabled: bool) {
        self.imp().visual_bell.set(enabled);
    }

    /// Read clipboard text and deliver it through the managed input path.
    ///
    /// Managed panes use VTE only as a renderer. Their real writable backend
    /// is the daemon `Input` message, so VTE's local `paste_clipboard()`
    /// handler does not reach the remote PTY.
    pub fn request_clipboard_paste<F: Fn(Vec<u8>) + 'static>(&self, f: F) {
        if !self.imp().accepts_input.get() {
            return;
        }

        let pane_weak = self.downgrade();
        self.clipboard().read_text_async(None::<&gtk4::gio::Cancellable>, move |result| {
            let Some(pane) = pane_weak.upgrade() else {
                return;
            };
            if !pane.imp().accepts_input.get() {
                return;
            }

            match result {
                Ok(Some(text)) if !text.is_empty() => f(text.as_bytes().to_vec()),
                Ok(Some(_) | None) => {}
                Err(error) => {
                    log::warn!("Failed to read clipboard text for managed paste: {error}");
                }
            }
        });
    }

    /// Flash the header on bell.
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

    /// Apply a color scheme to the VTE terminal.
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

    /// Get the terminal's column and row count.
    #[must_use]
    pub fn terminal_size(&self) -> (u16, u16) {
        let vte = &self.imp().vte;
        (vte.column_count() as u16, vte.row_count() as u16)
    }

    /// Connect a callback for keyboard input.
    ///
    /// The callback receives the raw terminal bytes to send to the daemon.
    pub fn connect_input<F: Fn(&[u8]) + 'static>(&self, f: F) {
        let forward_input = std::rc::Rc::new(f);
        let pane_weak = self.downgrade();
        let key_controller = gtk4::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        key_controller.connect_key_pressed(move |_, key, _keycode, state| {
            let Some(pane) = pane_weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match terminal_key_action(
                TerminalInputBackend::Managed,
                key,
                state,
                pane.vte().has_selection(),
                pane.imp().smart_clipboard.get(),
            ) {
                TerminalKeyAction::CopySelection => {
                    pane.vte().copy_clipboard_format(vte4::Format::Text);
                    pane.vte().unselect_all();
                    glib::Propagation::Stop
                }
                TerminalKeyAction::PasteClipboard => {
                    let forward_input = std::rc::Rc::clone(&forward_input);
                    pane.request_clipboard_paste(move |bytes| {
                        forward_input(&bytes);
                    });
                    glib::Propagation::Stop
                }
                TerminalKeyAction::ForwardToPty(bytes) => {
                    if pane.imp().accepts_input.get() {
                        forward_input(&bytes);
                    }
                    glib::Propagation::Stop
                }
                TerminalKeyAction::PassThrough => glib::Propagation::Proceed,
            }
        });
        self.imp().input_key_controller.replace(Some(key_controller.clone()));
        self.imp().vte.add_controller(key_controller);
    }

    /// Connect a callback for terminal resize events.
    ///
    /// The callback receives `(cols, rows)`. The caller is responsible for
    /// sending a `Resize` message to the daemon.
    pub fn connect_resize<F: Fn(u16, u16) + 'static>(&self, f: F) {
        use std::cell::Cell;
        use std::rc::Rc;

        let vte = self.imp().vte.clone();
        let last_cols = Rc::new(Cell::new(0u16));
        let last_rows = Rc::new(Cell::new(0u16));
        let f = Rc::new(f);

        let emit_size = Rc::new({
            let last_cols = Rc::clone(&last_cols);
            let last_rows = Rc::clone(&last_rows);
            let f = Rc::clone(&f);
            move |vte: &vte4::Terminal| {
                if vte.width() <= 0 || vte.height() <= 0 {
                    return;
                }
                let cols = vte.column_count() as u16;
                let rows = vte.row_count() as u16;
                if cols > 0 && rows > 0 && (cols != last_cols.get() || rows != last_rows.get()) {
                    last_cols.set(cols);
                    last_rows.set(rows);
                    f(cols, rows);
                }
            }
        });

        {
            let emit_size = Rc::clone(&emit_size);
            let resize_vte = vte.clone();
            vte.connect_char_size_changed(move |_, _, _| {
                emit_size(&resize_vte);
            });
        }

        let pane_weak = self.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let Some(pane) = pane_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            emit_size(pane.vte());
            glib::ControlFlow::Continue
        });
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn connection_banner_visible_for_test(&self) -> bool {
        self.imp().connection_banner.is_visible()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn input_enabled_for_test(&self) -> bool {
        self.imp().accepts_input.get()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn status_label_text_for_test(&self) -> String {
        self.imp().status_label.label().to_string()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn connection_title_for_test(&self) -> String {
        self.imp().connection_title_label.label().to_string()
    }

    #[cfg(test)]
    pub(crate) fn emit_input_key_for_test(
        &self,
        key: gtk4::gdk::Key,
        modifiers: gtk4::gdk::ModifierType,
    ) -> glib::Propagation {
        self.imp()
            .input_key_controller
            .borrow()
            .as_ref()
            .expect("input controller should be connected before test input is emitted")
            .emit_by_name::<bool>("key-pressed", &[&key, &0u32, &modifiers])
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ConnectionProblem, RuntimeEndpoint, present_connection_status};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    macro_rules! require_display {
        () => {
            if !crate::test_helpers::ensure_gtk() {
                eprintln!("SKIPPED: no display available");
                return;
            }
        };
    }

    fn pump_events(max_ms: u64) {
        let ctx = glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_millis(max_ms);
        while Instant::now() < deadline {
            if !ctx.iteration(false) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn wait_until(max_ms: u64, condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_millis(max_ms);
        while Instant::now() < deadline {
            pump_events(20);
            if condition() {
                return true;
            }
        }
        condition()
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_presentation_controls_banner_and_input_state() {
        require_display!();

        let pane = PersistentPaneView::new("pane-1", "runtime-1");

        let reconnecting = present_connection_status(
            &RuntimeEndpoint::Local,
            &ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
        );
        pane.set_connection_presentation(
            &ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
            &reconnecting,
        );

        assert!(pane.connection_banner_visible_for_test());
        assert_eq!(pane.status_label_text_for_test(), "Retry 4s");
        assert_eq!(pane.connection_title_for_test(), "Reconnecting in 4s");
        assert!(!pane.input_enabled_for_test());

        let blocked = present_connection_status(
            &RuntimeEndpoint::Remote { host: "builder.example".into() },
            &ConnectionStatus::Blocked(ConnectionProblem::PermissionDenied),
        );
        pane.set_connection_presentation(
            &ConnectionStatus::Blocked(ConnectionProblem::PermissionDenied),
            &blocked,
        );

        let connected =
            present_connection_status(&RuntimeEndpoint::Local, &ConnectionStatus::Connected);
        pane.set_connection_presentation(&ConnectionStatus::Connected, &connected);
        assert!(!pane.connection_banner_visible_for_test());
        assert!(pane.input_enabled_for_test());

        let recovered =
            present_connection_status(&RuntimeEndpoint::Local, &ConnectionStatus::Recovered);
        pane.set_connection_presentation(&ConnectionStatus::Recovered, &recovered);
        assert!(!pane.connection_banner_visible_for_test());
        assert_eq!(pane.status_label_text_for_test(), "Connected");
    }

    #[test]
    fn managed_input_action_preserves_clipboard_shortcuts() {
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                true,
                true,
            ),
            TerminalKeyAction::CopySelection
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            TerminalKeyAction::PasteClipboard
        );
        // Ctrl+Shift+C without selection passes through (nothing to copy).
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::C,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
        // Ctrl+Shift+V always pastes in managed terminals.
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::V,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PasteClipboard
        );
    }

    #[test]
    fn managed_input_action_ignores_extra_non_shortcut_modifiers_for_clipboard_shortcuts() {
        let pointer_mask = gtk4::gdk::ModifierType::BUTTON1_MASK;

        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK | pointer_mask,
                false,
                true,
            ),
            TerminalKeyAction::PasteClipboard
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
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
    fn managed_input_action_preserves_window_accelerators() {
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::T,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::T,
                gtk4::gdk::ModifierType::CONTROL_MASK
                    | gtk4::gdk::ModifierType::SHIFT_MASK
                    | gtk4::gdk::ModifierType::ALT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::ISO_Left_Tab,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::F,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
    }

    #[test]
    fn managed_input_action_keeps_shell_control_sequences_when_no_shortcut_applies() {
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            TerminalKeyAction::ForwardToPty(vec![0x03])
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                false,
            ),
            TerminalKeyAction::ForwardToPty(vec![0x16])
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::X,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::ForwardToPty(vec![0x18])
        );
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn input_controller_preserves_clipboard_shortcuts_before_forwarding_shell_input() {
        require_display!();

        let pane = PersistentPaneView::new("pane-1", "runtime-1");
        pane.set_smart_clipboard(true);
        let window = gtk4::Window::new();
        window.set_default_size(640, 320);
        window.set_child(Some(&pane));
        window.present();
        pump_events(50);
        let display =
            gtk4::gdk::Display::default().expect("display should be available for GTK tests");
        display.clipboard().set_text("managed pasted text");
        pane.feed_output(b"managed copied text\r\n");
        let connected =
            present_connection_status(&RuntimeEndpoint::Local, &ConnectionStatus::Connected);
        pane.set_connection_presentation(&ConnectionStatus::Connected, &connected);

        let forwarded = Rc::new(RefCell::new(Vec::new()));
        let forwarded_clone = Rc::clone(&forwarded);
        pane.connect_input(move |bytes| {
            forwarded_clone.borrow_mut().push(bytes.to_vec());
        });

        assert_eq!(
            pane.emit_input_key_for_test(
                gtk4::gdk::Key::v,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::BUTTON1_MASK,
            ),
            glib::Propagation::Stop
        );
        assert!(
            wait_until(1_000, || { forwarded.borrow().contains(&b"managed pasted text".to_vec()) }),
            "managed Ctrl+V should forward clipboard text through the daemon input path"
        );

        pane.vte().select_all();
        assert_eq!(
            pane.emit_input_key_for_test(gtk4::gdk::Key::c, gtk4::gdk::ModifierType::CONTROL_MASK,),
            glib::Propagation::Stop
        );
        assert_eq!(
            pane.emit_input_key_for_test(
                gtk4::gdk::Key::C,
                gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
            ),
            glib::Propagation::Proceed
        );
        assert_eq!(
            forwarded.borrow().as_slice(),
            &[b"managed pasted text".to_vec()],
            "copy shortcuts must not leak shell input when a selection is present"
        );

        pane.vte().unselect_all();
        assert_eq!(
            pane.emit_input_key_for_test(gtk4::gdk::Key::c, gtk4::gdk::ModifierType::CONTROL_MASK,),
            glib::Propagation::Stop
        );
        assert_eq!(forwarded.borrow().as_slice(), &[b"managed pasted text".to_vec(), vec![0x03]]);
        window.close();
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn request_clipboard_paste_delivers_text_when_connected() {
        require_display!();

        let pane = PersistentPaneView::new("pane-1", "runtime-1");
        let window = gtk4::Window::new();
        window.set_default_size(640, 320);
        window.set_child(Some(&pane));
        window.present();
        pump_events(50);

        let display =
            gtk4::gdk::Display::default().expect("display should be available for GTK tests");
        display.clipboard().set_text("window action paste");

        let connected =
            present_connection_status(&RuntimeEndpoint::Local, &ConnectionStatus::Connected);
        pane.set_connection_presentation(&ConnectionStatus::Connected, &connected);

        let forwarded = Rc::new(RefCell::new(Vec::new()));
        let forwarded_clone = Rc::clone(&forwarded);
        pane.request_clipboard_paste(move |bytes| {
            forwarded_clone.borrow_mut().push(bytes);
        });

        assert!(
            wait_until(1_000, || { forwarded.borrow().contains(&b"window action paste".to_vec()) }),
            "managed clipboard paste helper should deliver clipboard bytes to daemon input"
        );

        window.close();
    }

    #[test]
    fn bell_bytes_are_stripped_from_snapshot_data() {
        let input = b"\x07prompt$ \x07cmd\r\n\x07prompt$ ";
        let filtered: Vec<u8> = input.iter().copied().filter(|&b| b != 0x07).collect();
        assert_eq!(filtered, b"prompt$ cmd\r\nprompt$ ");
        assert!(!filtered.contains(&0x07));
    }

    #[test]
    fn snapshot_without_bells_passes_through_unchanged() {
        let input = b"prompt$ cmd\r\nprompt$ ";
        assert!(!input.contains(&0x07));
    }
}
