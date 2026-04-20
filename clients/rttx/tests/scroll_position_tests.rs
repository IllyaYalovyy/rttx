//! Integration tests for VTE scroll position preservation across
//! reparenting operations (split, rebuild, close). Regression for #686.

use gtk4::prelude::*;
use rttx::terminal::handle::TerminalHandle;
use rttx::terminal::persistent_widget::PersistentPaneView;
use std::sync::Once;
use std::time::{Duration, Instant};

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

fn pump_events(max_ms: u64) {
    let ctx = gtk4::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    while Instant::now() < deadline {
        if !ctx.iteration(false) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn feed_scrollback(pane: &PersistentPaneView, line_count: usize) {
    let mut data = Vec::new();
    for i in 0..line_count {
        data.extend_from_slice(format!("line {i}\r\n").as_bytes());
    }
    pane.feed_snapshot(&data);
}

/// Scroll position is preserved when a persistent pane is reparented
/// (unparent + re-add), simulating what happens during split or rebuild.
/// Regression for #686.
#[test]
#[ignore = "requires isolated GTK harness"]
fn scroll_position_preserved_across_reparent() {
    require_display!();

    let pane = PersistentPaneView::new("reparent-1", "runtime-1");
    let handle = TerminalHandle::Managed(pane.clone());

    let window = gtk4::Window::new();
    window.set_default_size(640, 480);
    window.set_child(Some(&pane));
    window.present();
    pump_events(100);

    feed_scrollback(&pane, 200);
    pump_events(50);

    // Scroll to a mid-point.
    let adj = pane.vte().vadjustment().expect("vadjustment should exist");
    let mid = (adj.upper() - adj.page_size()) / 2.0;
    adj.set_value(mid);
    pump_events(20);

    let saved = handle.scroll_position().expect("scroll position should be available");
    assert!(
        (saved - mid).abs() < 1.0,
        "saved position should be near mid-point {mid}, got {saved}"
    );

    // Reparent: remove from window, re-add.
    window.set_child(None::<&gtk4::Widget>);
    pump_events(20);
    window.set_child(Some(&pane));
    pump_events(20);

    // Restore via TerminalHandle.
    handle.restore_scroll_position(saved);
    pump_events(50);

    let restored = handle.scroll_position().expect("scroll position should be available");
    assert!(
        (restored - saved).abs() < 1.0,
        "restored position {restored} should be near saved {saved}"
    );

    window.close();
}

/// Scroll position restore clamps to valid range when the scrollback
/// shrinks (e.g., terminal resized to show more rows). Regression for #686.
#[test]
#[ignore = "requires isolated GTK harness"]
fn scroll_position_restore_clamps_to_valid_range() {
    require_display!();

    let pane = PersistentPaneView::new("clamp-1", "runtime-1");
    let handle = TerminalHandle::Managed(pane.clone());

    let window = gtk4::Window::new();
    window.set_default_size(640, 480);
    window.set_child(Some(&pane));
    window.present();
    pump_events(100);

    feed_scrollback(&pane, 200);
    pump_events(50);

    // Save a position near the bottom.
    let adj = pane.vte().vadjustment().expect("vadjustment should exist");
    let near_bottom = adj.upper() - adj.page_size() - 5.0;
    adj.set_value(near_bottom);
    pump_events(20);

    // Attempt to restore an out-of-range value (larger than upper - page_size).
    // The restore should clamp without panicking.
    handle.restore_scroll_position(999_999.0);
    pump_events(50);

    let restored = handle.scroll_position().expect("scroll position should be available");
    let max_valid = adj.upper() - adj.page_size();
    assert!(
        restored <= max_valid + 1.0,
        "restored position {restored} should be clamped to max {max_valid}"
    );

    window.close();
}

/// After reconnect, `feed_snapshot` must scroll the viewport to the bottom
/// so the user sees the most recent output. The scroll is deferred to the
/// next main-loop iteration because VTE updates its layout asynchronously.
/// Regression for #707.
#[test]
#[ignore = "requires isolated GTK harness"]
fn feed_snapshot_scrolls_to_bottom_after_reconnect() {
    require_display!();

    let pane = PersistentPaneView::new("reconnect-scroll", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 480);
    window.set_child(Some(&pane));
    window.present();
    pump_events(100);

    // Simulate reconnect with scrollback replay.
    feed_scrollback(&pane, 300);
    pump_events(100);

    let adj = pane.vte().vadjustment().expect("vadjustment should exist");
    let bottom = adj.upper() - adj.page_size();
    assert!(
        bottom > 0.0,
        "scrollback must exceed visible area; upper={} page_size={}",
        adj.upper(),
        adj.page_size()
    );
    assert!(
        (adj.value() - bottom).abs() < 1.0,
        "viewport must be at bottom after reconnect snapshot; got {} expected ~{bottom}",
        adj.value()
    );

    window.close();
}
