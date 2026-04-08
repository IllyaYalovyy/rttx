use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use std::time::Duration;

#[cfg(test)]
const ACTIVITY_IDLE_DELAY_MS: u64 = 30;
#[cfg(not(test))]
const ACTIVITY_IDLE_DELAY_MS: u64 = 1_200;

/// Visual state for the session activity indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    /// No unread activity.
    None,
    /// Terminal is actively producing output.
    Active,
    /// Output was produced but has since stopped.
    Idle,
}

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Debug)]
    pub struct SessionRow {
        pub uuid: RefCell<String>,
        pub name: RefCell<String>,
        pub color_dot: gtk4::Image,
        pub position_label: gtk4::Label,
        pub connection_icon: gtk4::Image,
        pub activity_dot: gtk4::Image,
        pub close_button: gtk4::Button,
        pub activity_state: Cell<ActivityState>,
        pub idle_transition_source: RefCell<Option<glib::SourceId>>,
    }

    impl Default for SessionRow {
        fn default() -> Self {
            let color_dot = gtk4::Image::from_icon_name("circle-filled-symbolic");
            color_dot.set_pixel_size(10);

            let connection_icon = gtk4::Image::new();
            connection_icon.set_pixel_size(16);
            connection_icon.set_visible(false);

            let activity_dot = gtk4::Image::from_icon_name("media-record-symbolic");
            activity_dot.set_pixel_size(8);
            activity_dot.set_visible(false);

            let position_label = gtk4::Label::new(None);
            position_label.add_css_class("dim-label");
            position_label.add_css_class("caption");

            Self {
                uuid: RefCell::new(String::new()),
                name: RefCell::new(String::new()),
                color_dot,
                position_label,
                connection_icon,
                activity_dot,
                close_button: gtk4::Button::from_icon_name("window-close-symbolic"),
                activity_state: Cell::new(ActivityState::None),
                idle_transition_source: RefCell::new(None),
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

            self.close_button.add_css_class("flat");
            self.close_button.add_css_class("circular");

            obj.add_prefix(&self.color_dot);
            obj.add_prefix(&self.position_label);
            obj.add_suffix(&self.connection_icon);
            obj.add_suffix(&self.activity_dot);
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
    pub fn new(uuid: &str, name: &str) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().uuid.replace(uuid.to_string());
        obj.imp().name.replace(name.to_string());
        obj.set_title(name);
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

    pub fn set_color(&self, color: crate::session::SessionColor) {
        let dot = &self.imp().color_dot;
        for cls in crate::session::SessionColor::ALL {
            dot.remove_css_class(cls.css_class());
        }
        dot.add_css_class(color.css_class());
    }

    pub fn set_connection_icon(&self, icon: Option<&crate::runtime::ConnectionIcon>) {
        const ICON_CSS_CLASSES: &[&str] = &["accent", "dim-label", "warning", "error"];
        let widget = &self.imp().connection_icon;
        for cls in ICON_CSS_CLASSES {
            widget.remove_css_class(cls);
        }
        match icon {
            Some(icon) => {
                widget.set_icon_name(Some(icon.icon_name));
                widget.add_css_class(icon.css_class);
                widget.set_visible(true);
            }
            None => widget.set_visible(false),
        }
    }

    fn set_activity_state_internal(&self, state: ActivityState) {
        let imp = self.imp();
        imp.activity_state.set(state);
        match state {
            ActivityState::None => {
                imp.activity_dot.remove_css_class("accent");
                imp.activity_dot.remove_css_class("session-activity-idle");
                imp.activity_dot.set_tooltip_text(None);
                imp.activity_dot.set_visible(false);
            }
            ActivityState::Active => {
                imp.activity_dot.remove_css_class("session-activity-idle");
                imp.activity_dot.add_css_class("accent");
                imp.activity_dot.set_tooltip_text(Some("Background activity is ongoing"));
                imp.activity_dot.set_visible(true);
            }
            ActivityState::Idle => {
                imp.activity_dot.remove_css_class("accent");
                imp.activity_dot.add_css_class("session-activity-idle");
                imp.activity_dot
                    .set_tooltip_text(Some("Unread activity is waiting in this workspace"));
                imp.activity_dot.set_visible(true);
            }
        }
    }

    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.activity_state() != ActivityState::None
    }

    #[must_use]
    pub fn activity_state(&self) -> ActivityState {
        self.imp().activity_state.get()
    }

    fn clear_pending_idle_transition(&self) {
        if let Some(source_id) = self.imp().idle_transition_source.borrow_mut().take() {
            source_id.remove();
        }
    }

    pub fn mark_activity(&self) {
        self.clear_pending_idle_transition();
        self.set_activity_state_internal(ActivityState::Active);

        let row_weak = self.downgrade();
        let source_id =
            glib::timeout_add_local(Duration::from_millis(ACTIVITY_IDLE_DELAY_MS), move || {
                let Some(row) = row_weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                row.imp().idle_transition_source.borrow_mut().take();
                row.set_activity_state_internal(ActivityState::Idle);
                glib::ControlFlow::Break
            });
        self.imp().idle_transition_source.replace(Some(source_id));
    }

    pub fn clear_activity(&self) {
        self.clear_pending_idle_transition();
        self.set_activity_state_internal(ActivityState::None);
    }

    pub fn set_position(&self, position: usize) {
        if position < 9 {
            self.imp().position_label.set_label(&format!("{}", position + 1));
            self.imp().position_label.set_visible(true);
        } else {
            self.imp().position_label.set_visible(false);
        }
    }

    #[must_use]
    pub fn position_label_text(&self) -> String {
        self.imp().position_label.label().to_string()
    }

    #[must_use]
    pub fn close_button(&self) -> &gtk4::Button {
        &self.imp().close_button
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn session_row_is_an_action_row() {
        require_display!();

        let row = SessionRow::new("session-1", "Session 1");

        assert!(row.is::<adw::ActionRow>());
        assert!(row.is::<gtk4::ListBoxRow>());
        assert_eq!(row.title().as_str(), "Session 1");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn session_row_updates_title() {
        require_display!();

        let row = SessionRow::new("session-1", "Session 1");
        row.set_session_name("Renamed");

        assert_eq!(row.session_name(), "Renamed");
        assert_eq!(row.title().as_str(), "Renamed");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn session_row_subtitle_shows_pane_info() {
        require_display!();

        let row = SessionRow::new("session-1", "Session 1");
        row.set_subtitle("Local · vim main.rs");
        assert_eq!(row.subtitle().unwrap().as_str(), "Local · vim main.rs");

        row.set_subtitle("Local");
        assert_eq!(row.subtitle().unwrap().as_str(), "Local");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_icon_hidden_by_default() {
        require_display!();
        let row = SessionRow::new("s1", "Session");
        assert!(!row.imp().connection_icon.is_visible());
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_icon_shows_and_hides() {
        require_display!();
        let row = SessionRow::new("s1", "Session");

        let icon = crate::runtime::ConnectionIcon {
            icon_name: "network-server-symbolic",
            css_class: "accent",
        };
        row.set_connection_icon(Some(&icon));
        assert!(row.imp().connection_icon.is_visible());
        assert_eq!(row.imp().connection_icon.icon_name().unwrap(), "network-server-symbolic");
        assert!(row.imp().connection_icon.has_css_class("accent"));

        row.set_connection_icon(None);
        assert!(!row.imp().connection_icon.is_visible());
        assert!(!row.imp().connection_icon.has_css_class("accent"));
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_icon_switches_css_class() {
        require_display!();
        let row = SessionRow::new("s1", "Session");

        let connected = crate::runtime::ConnectionIcon {
            icon_name: "network-server-symbolic",
            css_class: "accent",
        };
        row.set_connection_icon(Some(&connected));
        assert!(row.imp().connection_icon.has_css_class("accent"));

        let disconnected = crate::runtime::ConnectionIcon {
            icon_name: "network-offline-symbolic",
            css_class: "warning",
        };
        row.set_connection_icon(Some(&disconnected));
        assert!(!row.imp().connection_icon.has_css_class("accent"));
        assert!(row.imp().connection_icon.has_css_class("warning"));
        assert_eq!(row.imp().connection_icon.icon_name().unwrap(), "network-offline-symbolic");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn activity_indicator_toggles() {
        require_display!();

        let row = SessionRow::new("s1", "Session");
        assert_eq!(row.activity_state(), ActivityState::None);
        assert!(!row.has_activity(), "activity should be off initially");

        row.mark_activity();
        assert!(row.has_activity());
        assert_eq!(row.activity_state(), ActivityState::Active);

        assert!(
            wait_until(250, || row.activity_state() == ActivityState::Idle),
            "activity should settle to idle after output stops"
        );
        assert!(row.has_activity());

        row.clear_activity();
        assert!(!row.has_activity());
        assert_eq!(row.activity_state(), ActivityState::None);
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn repeated_activity_refreshes_idle_timer() {
        require_display!();

        let row = SessionRow::new("s1", "Session");

        row.mark_activity();
        pump_events(ACTIVITY_IDLE_DELAY_MS / 2);

        row.mark_activity();
        assert_eq!(row.activity_state(), ActivityState::Active);

        pump_events((ACTIVITY_IDLE_DELAY_MS / 2) + 10);
        assert_eq!(
            row.activity_state(),
            ActivityState::Active,
            "a second activity event should refresh the idle timer"
        );

        assert!(
            wait_until(250, || row.activity_state() == ActivityState::Idle),
            "refreshed activity should eventually settle to idle"
        );
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn clear_activity_cancels_pending_idle_transition() {
        require_display!();

        let row = SessionRow::new("s1", "Session");

        row.mark_activity();
        assert_eq!(row.activity_state(), ActivityState::Active);

        row.clear_activity();
        pump_events(100);

        assert_eq!(
            row.activity_state(),
            ActivityState::None,
            "clearing activity should prevent a later idle transition"
        );
        assert!(!row.has_activity());
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn position_label_shows_number() {
        require_display!();

        let row = SessionRow::new("s1", "Session");
        row.set_position(0);
        assert_eq!(row.position_label_text(), "1");

        row.set_position(8);
        assert_eq!(row.position_label_text(), "9");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn position_label_hidden_beyond_nine() {
        require_display!();

        let row = SessionRow::new("s1", "Session");
        row.set_position(0);
        assert!(row.imp().position_label.is_visible());

        row.set_position(9);
        assert!(!row.imp().position_label.is_visible(), "positions >= 9 should hide the label");
    }
}
