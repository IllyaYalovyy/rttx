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
    pub struct WorkspaceRow {
        pub uuid: RefCell<String>,
        pub name: RefCell<String>,
        pub position_label: gtk4::Label,
        pub connection_icon: gtk4::Image,
        pub close_button: gtk4::Button,
        pub activity_state: Cell<ActivityState>,
        pub idle_transition_source: RefCell<Option<glib::SourceId>>,
    }

    impl Default for WorkspaceRow {
        fn default() -> Self {
            let connection_icon = gtk4::Image::new();
            connection_icon.set_pixel_size(16);

            let position_label = gtk4::Label::new(None);
            position_label.add_css_class("dim-label");
            position_label.add_css_class("caption");

            Self {
                uuid: RefCell::new(String::new()),
                name: RefCell::new(String::new()),
                position_label,
                connection_icon,
                close_button: gtk4::Button::from_icon_name("window-close-symbolic"),
                activity_state: Cell::new(ActivityState::None),
                idle_transition_source: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WorkspaceRow {
        const NAME: &'static str = "RttxWorkspaceRow";
        type Type = super::WorkspaceRow;
        type ParentType = adw::ActionRow;
    }

    impl ObjectImpl for WorkspaceRow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_activatable(true);
            obj.set_selectable(true);
            obj.set_title_lines(1);

            self.close_button.add_css_class("flat");
            self.close_button.add_css_class("circular");

            obj.add_prefix(&self.connection_icon);
            obj.add_prefix(&self.position_label);
            obj.add_suffix(&self.close_button);

            obj.set_subtitle_lines(1);
            obj.add_css_class("session-row");
        }
    }

    impl WidgetImpl for WorkspaceRow {}
    impl ListBoxRowImpl for WorkspaceRow {}
    impl PreferencesRowImpl for WorkspaceRow {}
    impl ActionRowImpl for WorkspaceRow {}
}

glib::wrapper! {
    pub struct WorkspaceRow(ObjectSubclass<imp::WorkspaceRow>)
        @extends adw::ActionRow, adw::PreferencesRow, gtk4::ListBoxRow, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Actionable;
}

impl WorkspaceRow {
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
    pub fn workspace_name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub fn set_workspace_name(&self, name: &str) {
        self.imp().name.replace(name.to_string());
        self.set_title(name);
    }

    pub fn set_connection_icon(&self, icon: &crate::runtime::ConnectionIcon) {
        const ICON_CSS_CLASSES: &[&str] = &["accent", "dim-label", "warning", "error"];
        let widget = &self.imp().connection_icon;
        for cls in ICON_CSS_CLASSES {
            widget.remove_css_class(cls);
        }
        widget.set_icon_name(Some(icon.icon_name));
        widget.add_css_class(icon.css_class);
        widget.set_tooltip_text(Some(icon.tooltip));
    }

    fn set_activity_state_internal(&self, state: ActivityState) {
        let imp = self.imp();
        imp.activity_state.set(state);
        let obj = imp.obj();
        obj.remove_css_class("session-activity-active");
        obj.remove_css_class("session-activity-idle");
        match state {
            ActivityState::None => {
                obj.set_tooltip_text(None);
            }
            ActivityState::Active => {
                obj.add_css_class("session-activity-active");
                obj.set_tooltip_text(Some("Background activity is ongoing"));
            }
            ActivityState::Idle => {
                obj.add_css_class("session-activity-idle");
                obj.set_tooltip_text(Some("Unread activity in this workspace"));
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

    /// Switch the close button to a menu icon for managed workspaces.
    pub fn set_managed_actions_style(&self) {
        self.imp().close_button.set_icon_name("view-more-symbolic");
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

        let row = WorkspaceRow::new("session-1", "Session 1");

        assert!(row.is::<adw::ActionRow>());
        assert!(row.is::<gtk4::ListBoxRow>());
        assert_eq!(row.title().as_str(), "Session 1");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn session_row_updates_title() {
        require_display!();

        let row = WorkspaceRow::new("session-1", "Session 1");
        row.set_workspace_name("Renamed");

        assert_eq!(row.workspace_name(), "Renamed");
        assert_eq!(row.title().as_str(), "Renamed");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn session_row_subtitle_shows_pane_info() {
        require_display!();

        let row = WorkspaceRow::new("session-1", "Session 1");
        row.set_subtitle("vim main.rs");
        assert_eq!(row.subtitle().unwrap().as_str(), "vim main.rs");

        row.set_subtitle("");
        assert_eq!(row.subtitle().unwrap().as_str(), "");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_icon_visible_by_default() {
        require_display!();
        let row = WorkspaceRow::new("s1", "Session");
        assert!(row.imp().connection_icon.is_visible());
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_icon_shows_and_updates() {
        require_display!();
        let row = WorkspaceRow::new("s1", "Session");

        let icon = crate::runtime::ConnectionIcon {
            icon_name: "network-server-symbolic",
            css_class: "accent",
            tooltip: "Connected to remote host",
        };
        row.set_connection_icon(&icon);
        assert!(row.imp().connection_icon.is_visible());
        assert_eq!(row.imp().connection_icon.icon_name().unwrap(), "network-server-symbolic");
        assert!(row.imp().connection_icon.has_css_class("accent"));

        let local = crate::runtime::ConnectionIcon {
            icon_name: "computer-symbolic",
            css_class: "accent",
            tooltip: "Connected to local runtime",
        };
        row.set_connection_icon(&local);
        assert!(row.imp().connection_icon.is_visible());
        assert_eq!(row.imp().connection_icon.icon_name().unwrap(), "computer-symbolic");
        assert!(row.imp().connection_icon.has_css_class("accent"));
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_icon_switches_css_class() {
        require_display!();
        let row = WorkspaceRow::new("s1", "Session");

        let connected = crate::runtime::ConnectionIcon {
            icon_name: "network-server-symbolic",
            css_class: "accent",
            tooltip: "Connected to remote host",
        };
        row.set_connection_icon(&connected);
        assert!(row.imp().connection_icon.has_css_class("accent"));

        let disconnected = crate::runtime::ConnectionIcon {
            icon_name: "network-offline-symbolic",
            css_class: "warning",
            tooltip: "Disconnected from runtime",
        };
        row.set_connection_icon(&disconnected);
        assert!(!row.imp().connection_icon.has_css_class("accent"));
        assert!(row.imp().connection_icon.has_css_class("warning"));
        assert_eq!(row.imp().connection_icon.icon_name().unwrap(), "network-offline-symbolic");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn activity_indicator_toggles() {
        require_display!();

        let row = WorkspaceRow::new("s1", "Session");
        assert_eq!(row.activity_state(), ActivityState::None);
        assert!(!row.has_activity(), "activity should be off initially");
        assert!(!row.has_css_class("session-activity-active"));
        assert!(!row.has_css_class("session-activity-idle"));

        row.mark_activity();
        assert!(row.has_activity());
        assert_eq!(row.activity_state(), ActivityState::Active);
        assert!(row.has_css_class("session-activity-active"));

        assert!(
            wait_until(250, || row.activity_state() == ActivityState::Idle),
            "activity should settle to idle after output stops"
        );
        assert!(row.has_activity());
        assert!(row.has_css_class("session-activity-idle"));
        assert!(!row.has_css_class("session-activity-active"));

        row.clear_activity();
        assert!(!row.has_activity());
        assert_eq!(row.activity_state(), ActivityState::None);
        assert!(!row.has_css_class("session-activity-active"));
        assert!(!row.has_css_class("session-activity-idle"));
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn repeated_activity_refreshes_idle_timer() {
        require_display!();

        let row = WorkspaceRow::new("s1", "Session");

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

        let row = WorkspaceRow::new("s1", "Session");

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

        let row = WorkspaceRow::new("s1", "Session");
        row.set_position(0);
        assert_eq!(row.position_label_text(), "1");

        row.set_position(8);
        assert_eq!(row.position_label_text(), "9");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn position_label_hidden_beyond_nine() {
        require_display!();

        let row = WorkspaceRow::new("s1", "Session");
        row.set_position(0);
        assert!(row.imp().position_label.is_visible());

        row.set_position(9);
        assert!(!row.imp().position_label.is_visible(), "positions >= 9 should hide the label");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn connection_icon_tooltip_updates() {
        require_display!();
        let row = WorkspaceRow::new("s1", "Session");

        let icon = crate::runtime::ConnectionIcon {
            icon_name: "network-server-symbolic",
            css_class: "accent",
            tooltip: "Connected to remote host",
        };
        row.set_connection_icon(&icon);
        assert_eq!(
            row.imp().connection_icon.tooltip_text().unwrap().as_str(),
            "Connected to remote host"
        );

        let local = crate::runtime::ConnectionIcon {
            icon_name: "computer-symbolic",
            css_class: "dim-label",
            tooltip: "Local workspace",
        };
        row.set_connection_icon(&local);
        assert_eq!(row.imp().connection_icon.tooltip_text().unwrap().as_str(), "Local workspace");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn managed_actions_style_changes_button_icon() {
        require_display!();
        let row = WorkspaceRow::new("s1", "Session");
        assert_eq!(row.close_button().icon_name().unwrap(), "window-close-symbolic");

        row.set_managed_actions_style();
        assert_eq!(row.close_button().icon_name().unwrap(), "view-more-symbolic");
    }

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn session_row_has_css_class_and_subtitle_truncation() {
        require_display!();
        let row = WorkspaceRow::new("s1", "Session");
        assert!(row.has_css_class("session-row"));
        assert_eq!(row.subtitle_lines(), 2);
    }

    // ═══════════════════════════════════════════════════════════════
    // C5 — GLib source leaks
    //
    // A glib::timeout_add_local source that fires after the owning
    // widget is destroyed accesses freed memory → crash. The
    // mark_activity timer uses downgrade() so the callback returns
    // Break when the row is gone, but the source itself must also be
    // cancelled on destruction to avoid a dangling GLib source.
    // ═══════════════════════════════════════════════════════════════

    /// C5 regression: dropping a `WorkspaceRow` with an active idle timer
    /// must not cause the timer callback to mutate the row's state.
    /// The weak-ref pattern in `mark_activity` returns `Break` when the
    /// row is gone — this test proves that contract holds.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn c5_dropped_session_row_idle_timer_does_not_fire() {
        require_display!();

        let row = WorkspaceRow::new("s1", "Session");
        row.mark_activity();
        assert_eq!(row.activity_state(), ActivityState::Active);

        // Verify the idle transition source is registered.
        assert!(
            row.imp().idle_transition_source.borrow().is_some(),
            "idle transition source should be registered after mark_activity"
        );

        // Drop the row — the weak ref in the timer callback should
        // prevent any state mutation.
        let weak = row.downgrade();
        drop(row);

        // Pump the main loop past the idle delay so the timer fires.
        pump_events(ACTIVITY_IDLE_DELAY_MS + 100);

        // The weak ref should not upgrade — the row is gone.
        assert!(weak.upgrade().is_none(), "WorkspaceRow should be finalized after drop");
    }

    /// C5 regression: `clear_activity` must cancel the pending `GLib` source
    /// so it never fires after the row transitions to None state.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn c5_clear_activity_removes_glib_source() {
        require_display!();

        let row = WorkspaceRow::new("s1", "Session");
        row.mark_activity();
        assert!(
            row.imp().idle_transition_source.borrow().is_some(),
            "source should exist after mark_activity"
        );

        row.clear_activity();
        assert!(
            row.imp().idle_transition_source.borrow().is_none(),
            "source should be removed after clear_activity"
        );

        // Pump past the delay — state must remain None.
        pump_events(ACTIVITY_IDLE_DELAY_MS + 100);
        assert_eq!(
            row.activity_state(),
            ActivityState::None,
            "state must stay None — the cancelled source must not fire"
        );
    }

    /// C5 regression: calling `mark_activity` replaces the previous `GLib`
    /// source. The old source must be removed so only one is active.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn c5_mark_activity_replaces_previous_source() {
        require_display!();

        let row = WorkspaceRow::new("s1", "Session");
        row.mark_activity();
        assert!(row.imp().idle_transition_source.borrow().is_some());

        // Capture the debug representation of the first source.
        let first_debug = format!("{:?}", *row.imp().idle_transition_source.borrow());

        row.mark_activity();
        assert!(row.imp().idle_transition_source.borrow().is_some());

        let second_debug = format!("{:?}", *row.imp().idle_transition_source.borrow());

        // The sources should be different — the first was cancelled.
        assert_ne!(
            first_debug, second_debug,
            "second mark_activity should replace the GLib source, not stack on it"
        );
    }
}
