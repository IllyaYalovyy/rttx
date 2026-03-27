//! GTK widget for daemon-backed persistent terminal panes.
//!
//! Uses a `vte4::Terminal` in feed mode — no local PTY. Terminal output
//! arrives as `Delta` messages from rttxd and is fed into VTE for
//! rendering. Keyboard input is captured and sent back to the daemon.

use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use vte4::prelude::*;

use crate::color_scheme;
use crate::runtime::ConnectionStatus;

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
        pub vte: vte4::Terminal,
        pub terminal_scroller: gtk4::ScrolledWindow,
        pub header: gtk4::Box,
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
                vte: vte4::Terminal::new(),
                terminal_scroller: gtk4::ScrolledWindow::new(),
                header: gtk4::Box::default(),
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
            self.status_label.set_tooltip_text(Some("Persistent session — daemon-backed"));

            self.split_h_button.set_icon_name("object-flip-horizontal-symbolic");
            self.split_h_button.add_css_class("flat");
            self.split_h_button.set_tooltip_text(Some("Split horizontally"));

            self.split_v_button.set_icon_name("object-flip-vertical-symbolic");
            self.split_v_button.add_css_class("flat");
            self.split_v_button.set_tooltip_text(Some("Split vertically"));

            self.close_button.set_icon_name("window-close-symbolic");
            self.close_button.add_css_class("flat");
            self.close_button.set_tooltip_text(Some("Detach terminal"));

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
            obj.append(&self.search_bar);
            obj.append(&self.terminal_scroller);

            // Smart clipboard: Ctrl+C copies if selection exists, Ctrl+V pastes.
            let smart_clipboard_controller = gtk4::ShortcutController::new();
            smart_clipboard_controller.set_name(Some("smart-clipboard"));
            smart_clipboard_controller.set_scope(gtk4::ShortcutScope::Local);
            smart_clipboard_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

            let copy_vte = self.vte.clone();
            let copy_flag = std::rc::Rc::clone(&self.smart_clipboard);
            smart_clipboard_controller.add_shortcut(gtk4::Shortcut::new(
                Some(gtk4::KeyvalTrigger::new(
                    gtk4::gdk::Key::c,
                    gtk4::gdk::ModifierType::CONTROL_MASK,
                )),
                Some(gtk4::CallbackAction::new(move |_, _| {
                    if copy_flag.get() && copy_vte.has_selection() {
                        copy_vte.copy_clipboard_format(vte4::Format::Text);
                        copy_vte.unselect_all();
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                })),
            ));

            let paste_vte = self.vte.clone();
            let paste_flag = std::rc::Rc::clone(&self.smart_clipboard);
            smart_clipboard_controller.add_shortcut(gtk4::Shortcut::new(
                Some(gtk4::KeyvalTrigger::new(
                    gtk4::gdk::Key::v,
                    gtk4::gdk::ModifierType::CONTROL_MASK,
                )),
                Some(gtk4::CallbackAction::new(move |_, _| {
                    if paste_flag.get() {
                        paste_vte.paste_clipboard();
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                })),
            ));

            self.vte.add_controller(smart_clipboard_controller);

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
    /// A terminal pane backed by the rttxd daemon.
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
        }
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
    pub fn feed_snapshot(&self, scrollback: &[u8]) {
        if !scrollback.is_empty() {
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
        let label = match status {
            ConnectionStatus::Starting => "Starting".to_string(),
            ConnectionStatus::Connecting => "Connecting".to_string(),
            ConnectionStatus::Connected => "Connected".to_string(),
            ConnectionStatus::Reconnecting { attempt } => format!("Retry {attempt}"),
            ConnectionStatus::Blocked(problem) => format!("Blocked: {}", problem.label()),
            ConnectionStatus::Disconnected => "Disconnected".to_string(),
            ConnectionStatus::Recovered => "Recovered".to_string(),
        };
        self.imp().status_label.set_label(&label);
        self.imp().status_label.set_tooltip_text(Some(&status.label()));
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
    /// The callback receives the text typed by the user. The caller is
    /// responsible for sending it to the daemon as an `Input` message.
    pub fn connect_input<F: Fn(&str) + 'static>(&self, f: F) {
        self.imp().vte.connect_commit(move |_, text, _| {
            f(text);
        });
    }

    /// Connect a callback for terminal resize events.
    ///
    /// The callback receives `(cols, rows)`. The caller is responsible for
    /// sending a `Resize` message to the daemon.
    pub fn connect_resize<F: Fn(u16, u16) + 'static>(&self, f: F) {
        use std::cell::Cell;
        let last_cols = Cell::new(0u16);
        let last_rows = Cell::new(0u16);
        self.imp().vte.connect_char_size_changed(move |vte, _, _| {
            let cols = vte.column_count() as u16;
            let rows = vte.row_count() as u16;
            if cols > 0 && rows > 0 && (cols != last_cols.get() || rows != last_rows.get()) {
                last_cols.set(cols);
                last_rows.set(rows);
                f(cols, rows);
            }
        });
    }
}
