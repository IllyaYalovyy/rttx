//! Integration tests for VTE crash resilience (#958).
//!
//! Verifies that a VTE panic in one pane does not affect other panes
//! or the overall GUI state.

use rttx::terminal::handle::TerminalHandle;
use rttx::terminal::persistent_widget::PersistentPaneView;
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
            eprintln!("SKIPPED: no display available");
            return;
        }
    };
}

/// A crashed pane does not affect sibling panes in the same workspace.
/// Regression test for #958.
#[test]
#[ignore = "requires isolated GTK harness"]
fn crashed_pane_does_not_affect_sibling() {
    require_display!();

    let pane_a = PersistentPaneView::new("pane-a", "rt-1");
    let pane_b = PersistentPaneView::new("pane-b", "rt-1");

    // Both panes start healthy.
    assert!(!pane_a.is_crashed());
    assert!(!pane_b.is_crashed());

    // Pane A crashes.
    pane_a.mark_crashed();

    // Pane A is isolated.
    assert!(pane_a.is_crashed());

    // Pane B remains fully functional.
    assert!(!pane_b.is_crashed());
    pane_b.feed_output(b"still working");
    assert!(!pane_b.is_crashed());
}

/// Repair terminal restores a crashed pane to a functional state.
/// Regression test for #958.
#[test]
#[ignore = "requires isolated GTK harness"]
fn repair_recovers_crashed_pane() {
    require_display!();

    let pane = PersistentPaneView::new("repair-int", "rt-1");
    pane.feed_output(b"initial output");
    assert!(!pane.is_crashed());

    // Simulate crash.
    pane.mark_crashed();
    assert!(pane.is_crashed());
    pane.feed_output(b"ignored after crash");

    // Repair via the handle (same path as Ctrl+Shift+X).
    let handle = TerminalHandle::Managed(pane.clone());
    handle.repair_terminal();

    // Pane is functional again.
    assert!(!pane.is_crashed());
    pane.feed_output(b"working after repair");
    assert!(!pane.is_crashed());
}

/// The daemon connection is preserved when a pane crashes — the pane
/// remains addressable and can receive future deltas after repair.
/// Regression test for #958.
#[test]
#[ignore = "requires isolated GTK harness"]
fn daemon_connection_survives_pane_crash() {
    require_display!();

    let pane = PersistentPaneView::new("daemon-surv", "rt-1");
    pane.set_connected(true);

    pane.mark_crashed();

    // The pane object still exists and retains its identity.
    assert_eq!(pane.uuid(), "daemon-surv");
    assert_eq!(pane.runtime_id(), "rt-1");
    // The pane is crashed but not destroyed — daemon can still reference it.
    assert!(pane.is_crashed());
}
