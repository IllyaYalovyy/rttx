//! GTK test: a daemon OSC 52 clipboard write takes the system clipboard, and
//! a read-only pane never does (#46).
//!
//! VTE ignores OSC 52 entirely, so the daemon decodes the sequence and pushes
//! a `ClipboardWrite`; this is the client half of that path. A control write
//! runs first, exactly as the probe in #46 did: without proof that the
//! clipboard is genuinely ownable in this environment, a silent clipboard
//! would make every assertion below pass for the wrong reason.

use gtk4::prelude::*;
use rttx::runtime::{ConnectionProblem, ConnectionStatus, present_connection_status};
use rttx::terminal::persistent_widget::PersistentPaneView;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Once;

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

/// Read the clipboard back, pumping the main loop until the async read lands.
fn read_clipboard() -> Option<String> {
    let clipboard = gtk4::gdk::Display::default()?.clipboard();
    let text: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let done = Rc::new(Cell::new(false));
    let text_handle = Rc::clone(&text);
    let done_handle = Rc::clone(&done);
    clipboard.read_text_async(gtk4::gio::Cancellable::NONE, move |result| {
        *text_handle.borrow_mut() = result.ok().flatten().map(|s| s.to_string());
        done_handle.set(true);
    });

    let ctx = gtk4::glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !done.get() && std::time::Instant::now() < deadline {
        if !ctx.iteration(false) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    text.borrow().clone()
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn osc52_write_takes_the_clipboard_only_for_the_pane_this_client_drives() {
    require_display!();

    let pane = PersistentPaneView::new("pane-1", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&pane));
    window.present();
    pump_events(200);

    rttx::terminal::set_clipboard_text("SENTINEL-BEFORE");
    pump_events(100);
    if read_clipboard().as_deref() != Some("SENTINEL-BEFORE") {
        eprintln!("SKIPPED: the clipboard is not ownable under this display server");
        window.close();
        return;
    }

    let connected = present_connection_status(&ConnectionStatus::Connected);
    pane.set_connection_presentation(&ConnectionStatus::Connected, &connected);

    let copied = "copied by the app ✂";
    assert!(
        pane.apply_clipboard_write(copied.as_bytes(), true),
        "the pane this client drives applies the write"
    );
    pump_events(100);
    assert_eq!(read_clipboard().as_deref(), Some(copied));

    // A take-over leaves this client a read-only mirror. It still receives the
    // pane's events, and must not hijack the clipboard with them.
    let taken_over = ConnectionStatus::Blocked(ConnectionProblem::TakenOver);
    pane.set_connection_presentation(&taken_over, &present_connection_status(&taken_over));
    assert!(
        !pane.apply_clipboard_write(b"from a read-only mirror", true),
        "a read-only pane must refuse the write"
    );
    pump_events(100);
    assert_eq!(read_clipboard().as_deref(), Some(copied), "clipboard must be untouched");

    // With the preference off, not even the lease holder acts on it.
    pane.set_connection_presentation(&ConnectionStatus::Connected, &connected);
    assert!(!pane.apply_clipboard_write(b"disabled by preference", false));
    pump_events(100);
    assert_eq!(read_clipboard().as_deref(), Some(copied), "clipboard must be untouched");

    window.close();
}
