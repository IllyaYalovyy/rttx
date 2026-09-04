//! Behavioural coverage for next/previous pane cycling (#1095).
//!
//! Spatial navigation dead-ends at layout edges, which reads as "switching
//! panes does not work" when a pane is zoomed and the layout is invisible.
//! Cycling must always land on another pane, carry the zoom with it, and keep
//! the `n/total` pane counter in step.
//!
//! These tests need a display and a private GTK process — run with:
//!
//!   `xvfb-run cargo test --test pane_cycling -- --ignored`

use gtk4::gio::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use std::sync::Once;

static GTK_INIT: Once = Once::new();
static GTK_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn ensure_gtk_init() -> bool {
    GTK_INIT.call_once(|| {
        // SAFETY: GTK init runs once before any threads spawn; no concurrent env readers.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("GTK_A11Y", "none");
        };
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gtk4::init().is_ok()))
            .unwrap_or(false);
        if ok && let Some(display) = gtk4::gdk::Display::default() {
            std::mem::forget(display);
        }
        GTK_AVAILABLE.store(ok, std::sync::atomic::Ordering::Relaxed);
    });
    GTK_AVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
}

macro_rules! require_display {
    () => {
        if !ensure_gtk_init() {
            eprintln!("SKIPPED: no display available (run with GDK_BACKEND=broadway or xvfb-run)");
            return;
        }
    };
}

#[allow(unsafe_code)]
fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    unsafe { std::env::set_var(key, value) }
}

#[allow(unsafe_code)]
fn remove_env(key: &str) {
    unsafe { std::env::remove_var(key) }
}

fn pump_events(iterations: usize) {
    let context = gtk4::glib::MainContext::default();
    for _ in 0..iterations {
        while context.iteration(false) {}
    }
}

struct Harness {
    _tmp: tempfile::TempDir,
    app: adw::Application,
    window: rttx::window::Window,
}

impl Harness {
    fn new(app_id: &str) -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        set_env("XDG_CONFIG_HOME", tmp.path());
        set_env("XDG_STATE_HOME", tmp.path().join("state"));
        set_env("XDG_CACHE_HOME", tmp.path().join("cache"));
        set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder().application_id(app_id).build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = rttx::window::Window::new(&app);
        window.set_default_size(1000, 700);
        window.present();
        pump_events(100);

        Self { _tmp: tmp, app, window }
    }

    fn pane_uuids(&self) -> Vec<String> {
        self.window.imp().state.borrow().workspaces[0].layout.terminal_uuids()
    }

    fn zoomed_pane(&self) -> Option<String> {
        self.window.imp().state.borrow().workspaces[0].zoomed_terminal_uuid.clone()
    }

    fn focus_pane(&self, uuid: &str) {
        *self.window.imp().focused_terminal_uuid.borrow_mut() = Some(uuid.to_string());
    }

    fn focused_pane(&self) -> Option<String> {
        self.window.imp().focused_terminal_uuid.borrow().clone()
    }

    fn activate(&self, action: &str) {
        gtk4::gio::prelude::ActionGroupExt::activate_action(&self.window, action, None);
        pump_events(50);
    }

    /// Split until the workspace holds `count` panes, keeping the layout a
    /// left-to-right chain so layout order is predictable.
    fn split_to(&self, count: usize) {
        for _ in 0..count {
            if self.pane_uuids().len() >= count {
                break;
            }
            let last = self.pane_uuids().last().unwrap().clone();
            self.focus_pane(&last);
            self.activate("split-horizontal");
        }
        assert_eq!(self.pane_uuids().len(), count, "failed to reach {count} panes");
    }

    fn pane_counter_text(&self, uuid: &str) -> Option<String> {
        let terminals = self.window.imp().terminals.borrow();
        let term = terminals.get(uuid)?;
        let label = term.pane_count_label();
        label.is_visible().then(|| label.label().to_string())
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.window.close();
        pump_events(20);
        remove_env("RTTX_DISABLE_SHELL_SPAWN");
        remove_env("XDG_CONFIG_HOME");
        remove_env("XDG_STATE_HOME");
        remove_env("XDG_CACHE_HOME");
    }
}

/// Cycling while zoomed must move the zoom to every pane in turn and wrap,
/// including from the last pane where spatial navigation is a silent no-op.
#[test]
#[ignore = "requires isolated GTK harness"]
fn next_pane_while_zoomed_moves_zoom_through_all_panes_and_wraps() {
    require_display!();
    let h = Harness::new("com.illya.rttx.next-pane-zoomed-tests");

    h.split_to(3);
    let panes = h.pane_uuids();
    assert_eq!(panes.len(), 3, "test needs three panes");

    h.focus_pane(&panes[0]);
    h.activate("toggle-pane-zoom");
    assert_eq!(h.zoomed_pane().as_deref(), Some(panes[0].as_str()), "first pane should be zoomed");

    h.activate("next-pane");
    assert_eq!(h.zoomed_pane().as_deref(), Some(panes[1].as_str()), "next should zoom pane 2");

    h.activate("next-pane");
    assert_eq!(h.zoomed_pane().as_deref(), Some(panes[2].as_str()), "next should zoom pane 3");

    // The last pane has no rightward neighbour — cycling must still wrap.
    h.activate("next-pane");
    assert_eq!(
        h.zoomed_pane().as_deref(),
        Some(panes[0].as_str()),
        "next from the last pane should wrap to the first"
    );
}

/// Previous pane cycles the zoom in the opposite direction and wraps backwards.
#[test]
#[ignore = "requires isolated GTK harness"]
fn prev_pane_while_zoomed_moves_zoom_backwards_and_wraps() {
    require_display!();
    let h = Harness::new("com.illya.rttx.prev-pane-zoomed-tests");

    h.split_to(3);
    let panes = h.pane_uuids();

    h.focus_pane(&panes[0]);
    h.activate("toggle-pane-zoom");

    h.activate("prev-pane");
    assert_eq!(
        h.zoomed_pane().as_deref(),
        Some(panes[2].as_str()),
        "previous from the first pane should wrap to the last"
    );

    h.activate("prev-pane");
    assert_eq!(h.zoomed_pane().as_deref(), Some(panes[1].as_str()), "previous should zoom pane 2");
}

/// The `n/total` counter shown while zoomed must track the cycled pane, so the
/// user can tell where they are without seeing the layout.
#[test]
#[ignore = "requires isolated GTK harness"]
fn cycling_while_zoomed_updates_the_pane_counter() {
    require_display!();
    let h = Harness::new("com.illya.rttx.pane-counter-cycle-tests");

    h.split_to(3);
    let panes = h.pane_uuids();

    h.focus_pane(&panes[0]);
    h.activate("toggle-pane-zoom");
    assert_eq!(h.pane_counter_text(&panes[0]).as_deref(), Some("1/3"));

    h.activate("next-pane");
    assert_eq!(h.pane_counter_text(&panes[1]).as_deref(), Some("2/3"));

    h.activate("next-pane");
    assert_eq!(h.pane_counter_text(&panes[2]).as_deref(), Some("3/3"));
}

/// Without zoom, cycling moves only the focus — it must not zoom anything.
#[test]
#[ignore = "requires isolated GTK harness"]
fn cycling_without_zoom_moves_focus_only() {
    require_display!();
    let h = Harness::new("com.illya.rttx.pane-cycle-unzoomed-tests");

    h.split_to(2);
    let panes = h.pane_uuids();

    h.focus_pane(&panes[0]);
    h.activate("next-pane");
    pump_events(50);

    assert_eq!(h.focused_pane().as_deref(), Some(panes[1].as_str()), "focus should move to pane 2");
    assert!(h.zoomed_pane().is_none(), "cycling must not zoom a pane");
}

/// A single-pane workspace has nowhere to cycle to; the action must be inert.
#[test]
#[ignore = "requires isolated GTK harness"]
fn cycling_in_a_single_pane_workspace_is_a_no_op() {
    require_display!();
    let h = Harness::new("com.illya.rttx.pane-cycle-single-tests");

    let panes = h.pane_uuids();
    assert_eq!(panes.len(), 1, "a fresh workspace starts with one pane");

    h.focus_pane(&panes[0]);
    h.activate("next-pane");
    h.activate("prev-pane");

    assert_eq!(h.pane_uuids(), panes, "cycling must not change the layout");
    assert_eq!(h.focused_pane().as_deref(), Some(panes[0].as_str()), "focus must stay put");
    assert!(h.zoomed_pane().is_none(), "cycling must not zoom the only pane");
}

/// The cycling actions must be reachable from the keyboard out of the box.
#[test]
#[ignore = "requires isolated GTK harness"]
fn cycling_actions_register_their_default_accelerators() {
    require_display!();
    let h = Harness::new("com.illya.rttx.pane-cycle-accel-tests");

    for (action, accel) in [("next-pane", "<Alt>bracketright"), ("prev-pane", "<Alt>bracketleft")] {
        let accels = h.app.accels_for_action(&format!("win.{action}"));
        assert!(
            accels.iter().any(|a| a == accel),
            "action '{action}' should register accelerator '{accel}', got: {accels:?}"
        );
    }
}
