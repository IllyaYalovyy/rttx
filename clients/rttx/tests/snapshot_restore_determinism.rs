//! Integration tests for #809: snapshot restore must be deterministic
//! regardless of what the scrollback tail left in VTE.

use rttx::terminal::persistent_widget::PersistentPaneView;
use std::sync::Once;
use vte4::prelude::*;

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

/// Two panes with identical snapshots but different scrollback histories
/// must converge to the same tracked mode state after restore. #809.
#[test]
#[ignore = "requires isolated GTK harness"]
fn restore_converges_regardless_of_scrollback_history() {
    require_display!();

    let modes_off = rttx_proto::v3::TerminalModeState::default();

    // Pane A: scrollback enabled cursor-keys + mouse, then restore says off.
    let pane_a = PersistentPaneView::new("converge-a", "runtime-1");
    pane_a.vte().feed(b"\x1b[?1h\x1b[?1003h\x1b[?25l");
    pane_a.restore_interaction_modes(&modes_off);

    // Pane B: clean scrollback, same restore.
    let pane_b = PersistentPaneView::new("converge-b", "runtime-1");
    pane_b.restore_interaction_modes(&modes_off);

    // Tracked state must be identical.
    assert_eq!(pane_a.terminal_modes(), pane_b.terminal_modes());
}

/// Full snapshot-then-restore cycle: feed scrollback with stale modes,
/// then restore with the correct snapshot state. The tracked modes must
/// reflect the snapshot, not the scrollback. #809.
#[test]
#[ignore = "requires isolated GTK harness"]
fn full_snapshot_restore_cycle_overrides_scrollback_modes() {
    require_display!();

    let pane = PersistentPaneView::new("full-cycle-1", "runtime-1");

    // Simulate scrollback that enabled several modes.
    pane.feed_snapshot(b"\x1b[?1h\x1b[?1003h\x1b[?1006h\x1b[?25l");

    // Snapshot says: only application_cursor_keys should be on.
    let snapshot_modes = rttx_proto::v3::TerminalModeState {
        application_cursor_keys: true,
        ..Default::default()
    };
    pane.restore_interaction_modes(&snapshot_modes);

    let modes = pane.terminal_modes();
    assert!(modes.application_cursor_keys, "snapshot says cursor keys on");
    assert!(!modes.application_keypad, "snapshot says keypad off");
}
