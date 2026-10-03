//! Behaviour tests for the hint a copy shows when it comes up empty because
//! the pane's application owns the mouse.
//!
//! Regression for #1114. Claude Code, Codex, htop and vim with `mouse=a` arm
//! mouse tracking, and VTE then hands a plain drag to the application instead
//! of selecting, so Ctrl+Shift+C copied an empty selection and nothing at all
//! happened. The pane now decides, once, that such a copy deserves a word
//! about Shift+drag. These tests drive a real VTE with real escape sequences
//! and real selections, so what the pane sees is what the terminal sees.

#![allow(clippy::doc_markdown)]

use gtk4::prelude::*;
use rttx::terminal::handle::TerminalHandle;
use rttx::terminal::persistent_widget::PersistentPaneView;
use std::sync::Once;
use vte4::prelude::*;

static GTK_INIT: Once = Once::new();
static GTK_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn ensure_gtk_init() -> bool {
    GTK_INIT.call_once(|| {
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
    while std::time::Instant::now() < deadline {
        if !ctx.iteration(false) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// Poll VTE's text until it contains `needle`. VTE parses fed bytes on a
/// timer, so positive checks wait rather than pump a fixed interval.
fn wait_for_text(pane: &PersistentPaneView, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        pump_events(20);
        let text =
            pane.vte().text_format(vte4::Format::Text).map(|t| t.to_string()).unwrap_or_default();
        if text.contains(needle) || std::time::Instant::now() >= deadline {
            return text;
        }
    }
}

fn presented_pane() -> (gtk4::Window, PersistentPaneView) {
    let pane = PersistentPaneView::new("pane-1", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 480);
    window.set_child(Some(&pane));
    window.present();
    pump_events(50);
    (window, pane)
}

/// What a Claude Code pane persists: tracking armed, SGR encoding, alternate
/// screen. A copy there finds nothing selected, and that is worth explaining.
#[test]
#[ignore = "requires isolated GTK harness"]
fn empty_copy_hints_once_while_the_app_tracks_the_mouse() {
    require_display!();
    let (_window, pane) = presented_pane();

    pane.feed_output(b"\x1b[2J\x1b[H\x1b[?1049h\x1b[?1003h\x1b[?1006hmouse app\r\n");
    wait_for_text(&pane, "mouse app");
    assert!(pane.has_mouse_tracking(), "the app armed tracking");
    assert!(!pane.vte().has_selection(), "a plain drag belongs to the app, so nothing is selected");
    assert!(!pane.shift_select_hint_shown(), "nothing has been explained yet");

    let handle = TerminalHandle::Managed(pane.clone());
    assert!(handle.take_shift_select_hint(), "the first empty copy explains Shift+drag");
    assert!(pane.shift_select_hint_shown());

    for _ in 0..5 {
        assert!(!handle.take_shift_select_hint(), "not said again on every keypress");
    }
}

/// An empty selection in an ordinary pane is just an empty selection.
#[test]
#[ignore = "requires isolated GTK harness"]
fn empty_copy_stays_silent_without_mouse_tracking() {
    require_display!();
    let (_window, pane) = presented_pane();

    pane.feed_output(b"\x1b[2J\x1b[Hplain shell\r\n");
    wait_for_text(&pane, "plain shell");
    assert!(!pane.has_mouse_tracking());

    let handle = TerminalHandle::Managed(pane.clone());
    assert!(!handle.take_shift_select_hint(), "no tracking, nothing to say");
    assert!(!pane.shift_select_hint_shown(), "the pane's one-shot hint is not spent");

    // Tracking armed later still gets its one explanation.
    pane.feed_output(b"\x1b[?1003h");
    pump_events(50);
    assert!(pane.has_mouse_tracking());
    assert!(handle.take_shift_select_hint());
}

/// Shift+drag works, so a copy with a selection has nothing to explain — and
/// it must not spend the pane's one-shot hint either.
#[test]
#[ignore = "requires isolated GTK harness"]
fn copy_with_a_selection_says_nothing_even_while_tracking() {
    require_display!();
    let (_window, pane) = presented_pane();

    pane.feed_output(b"\x1b[2J\x1b[H\x1b[?1003h\x1b[?1006hselect me with shift\r\n");
    wait_for_text(&pane, "select me with shift");
    assert!(pane.has_mouse_tracking());
    pane.vte().select_all();
    pump_events(50);
    assert!(pane.vte().has_selection(), "Shift+drag's equivalent: a real selection");

    let handle = TerminalHandle::Managed(pane.clone());
    assert!(!handle.take_shift_select_hint(), "the copy had something to copy");
    assert!(!pane.shift_select_hint_shown(), "hint still available for a later empty copy");

    pane.vte().unselect_all();
    pump_events(50);
    assert!(handle.take_shift_select_hint(), "the next empty copy explains Shift+drag");
}

/// Tracking seeded from the daemon's snapshot counts too: a pane re-attached
/// into a running Claude Code session knows the app owns the mouse before any
/// output arrives.
#[test]
#[ignore = "requires isolated GTK harness"]
fn snapshot_seeded_tracking_also_hints() {
    require_display!();
    let (_window, pane) = presented_pane();

    let modes = rttx_proto::v3::TerminalModeState {
        mouse_mode: rttx_proto::v3::MouseMode::Any as i32,
        ..Default::default()
    };
    pane.restore_interaction_modes(&modes);
    assert!(pane.has_mouse_tracking(), "armed tracking from the snapshot is honoured");

    let handle = TerminalHandle::Managed(pane);
    assert!(handle.take_shift_select_hint());
    assert!(!handle.take_shift_select_hint());
}

/// Direct terminals render their own PTY and the client never learns which
/// mouse modes the application armed, so they stay silent.
#[test]
#[ignore = "requires isolated GTK harness"]
fn direct_terminals_never_hint() {
    require_display!();

    let direct = rttx::terminal::widget::TerminalWidget::new("direct-1", None);
    let window = gtk4::Window::new();
    window.set_child(Some(&direct));
    window.present();
    pump_events(50);

    let handle = TerminalHandle::Direct(direct);
    assert!(!handle.take_shift_select_hint());

    window.close();
}
