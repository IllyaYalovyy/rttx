//! Integration test: VTE CPR responses must not leak into daemon input.
//!
//! Regression for #633. When VTE processes snapshot data containing DSR
//! queries, it generates CPR responses via the `commit` signal. These
//! must be filtered before forwarding to the daemon.

#![allow(clippy::doc_markdown)]

use gtk4::prelude::*;
use std::cell::RefCell;
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

/// VTE CPR response emitted via `commit` must be filtered out before
/// reaching the daemon input callback. Regression for #633.
#[test]
#[ignore = "requires isolated GTK harness"]
fn cpr_response_filtered_from_persistent_pane_input() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&pane));
    window.present();
    pump_events(50);

    let connected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Connected);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Connected, &connected);

    let forwarded = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
    let forwarded_clone = Rc::clone(&forwarded);
    pane.connect_input(move |bytes| {
        forwarded_clone.borrow_mut().push(bytes.to_vec());
    });

    // CPR response must be silently dropped.
    let cpr = "\x1b[1;6R";
    pane.vte().emit_by_name::<()>("commit", &[&cpr, &(cpr.len() as u32)]);
    pump_events(50);
    assert!(forwarded.borrow().is_empty(), "CPR response must not be forwarded to daemon input");

    // DA1 response must also be dropped.
    let da1 = "\x1b[?64;1;2;6;22c";
    pane.vte().emit_by_name::<()>("commit", &[&da1, &(da1.len() as u32)]);
    pump_events(50);
    assert!(forwarded.borrow().is_empty(), "DA1 response must not be forwarded to daemon input");

    // Mouse sequences must still pass through.
    let sgr_click = "\x1b[<0;5;10M";
    pane.vte().emit_by_name::<()>("commit", &[&sgr_click, &(sgr_click.len() as u32)]);
    pump_events(50);
    assert!(
        forwarded.borrow().contains(&sgr_click.as_bytes().to_vec()),
        "mouse escape sequences must still be forwarded"
    );

    window.close();
}
