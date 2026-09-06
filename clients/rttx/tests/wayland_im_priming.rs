//! Regression coverage for the GTK Wayland input-method startup
//! use-after-free (rttx #1099, formerly #808).
//!
//! `priming_is_idempotent_and_backend_aware` runs on any display backend
//! (Broadway in CI) and checks the workaround's contract.
//!
//! `focused_widget_torn_down_before_text_input_roundtrip` is the
//! deterministic crash reproduction.  It needs a real Wayland compositor
//! that implements `zwp_text_input_v3` **and** hands the new window keyboard
//! focus, so it is opt-in.  Run it under a headless Mutter:
//!
//! ```bash
//! mutter --headless --wayland --no-x11 --wayland-display wl-test \
//!        --virtual-monitor 1280x720 &
//! WAYLAND_DISPLAY=wl-test GDK_BACKEND=wayland RTTX_WAYLAND_IM_REPRO=1 \
//!   cargo test -p rttx --test wayland_im_priming -- --ignored
//! ```
//!
//! A standalone C version of the same crash (no rttx involved, for the
//! upstream GTK report) lives at `tests/fixtures/gtk_im_uaf_upstream.c`.
//!
//! Add `RTTX_WAYLAND_IM_REPRO_UNPRIMED=1` to skip the workaround: the test
//! process then dies with SIGSEGV in `notify_im_change` on GTK 4.20.4,
//! which is the bug this file guards against.

use gtk4::prelude::*;
use rttx::wayland_im::{PrimingOutcome, backend_needs_priming, is_primed, prime_text_input};
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

fn pump(duration: Duration) {
    let ctx = gtk4::glib::MainContext::default();
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        while ctx.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn priming_is_idempotent_and_backend_aware() {
    require_display!();
    let display = gtk4::gdk::Display::default().expect("display after init");
    let wayland = backend_needs_priming(display.type_().name());

    assert!(!is_primed(), "fresh process must start unprimed");
    let first = prime_text_input();

    if wayland {
        assert_eq!(first, PrimingOutcome::Primed);
        assert!(is_primed());
        assert_eq!(prime_text_input(), PrimingOutcome::AlreadyPrimed);
    } else {
        assert_eq!(first, PrimingOutcome::NotWayland);
        assert!(!is_primed(), "non-Wayland backends must not keep a primer alive");
        assert_eq!(prime_text_input(), PrimingOutcome::NotWayland);
    }

    // Whatever the backend, a real text widget must still be able to take
    // focus and route IM focus afterwards without complaint.
    let window = gtk4::Window::new();
    let text = gtk4::Text::new();
    window.set_child(Some(&text));
    window.present();
    assert!(text.grab_focus());
    pump(Duration::from_millis(100));
    window.close();
    pump(Duration::from_millis(50));
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn focused_widget_torn_down_before_text_input_roundtrip() {
    if std::env::var_os("RTTX_WAYLAND_IM_REPRO").is_none() {
        eprintln!("SKIPPED: set RTTX_WAYLAND_IM_REPRO=1 under a headless Mutter (see file docs)");
        return;
    }
    require_display!();
    let display = gtk4::gdk::Display::default().expect("display after init");
    assert!(
        backend_needs_priming(display.type_().name()),
        "this reproduction needs GDK_BACKEND=wayland, got {}",
        display.type_().name()
    );

    let unprimed = std::env::var_os("RTTX_WAYLAND_IM_REPRO_UNPRIMED").is_some();
    if !unprimed {
        assert_eq!(prime_text_input(), PrimingOutcome::Primed);
    }

    // The exact sequence rttx performs at startup, compressed into one
    // dispatch: a text widget takes focus (GtkIMContextWayland focus_in ->
    // global created, registry request queued, global->current set), then
    // the widget is unrealized before the registry reply is read
    // (GtkText -> set_client_widget(NULL) -> delegate finalized).
    let window = gtk4::Window::new();
    window.set_title(Some("rttx wayland-im reproduction"));
    let text = gtk4::Text::new();
    window.set_child(Some(&text));
    window.present();
    assert!(text.grab_focus());
    window.set_child(Some(&gtk4::Label::new(Some("focused widget torn down"))));
    drop(text);

    // The compositor now binds text_input (registry reply) and, once it
    // gives the window keyboard focus, sends zwp_text_input_v3.enter.  On an
    // unprimed GTK 4.20.4 that event dereferences the freed context and the
    // process dies here with SIGSEGV.
    pump(Duration::from_secs(2));

    // Still alive.  Prove the danger was real: the compositor must actually
    // have entered text input on this window.  A fresh IM context that
    // takes focus while `global->focused` is set is enabled immediately,
    // and GTK's enable path emits `retrieve-surrounding`, which
    // GtkIMMulticontext relays to us.
    let probe_anchor = gtk4::Label::new(Some("probe"));
    window.set_child(Some(&probe_anchor));
    let probe = gtk4::IMMulticontext::new();
    probe.set_client_widget(Some(&probe_anchor));
    let entered = std::rc::Rc::new(std::cell::Cell::new(false));
    let entered_flag = std::rc::Rc::clone(&entered);
    probe.connect_retrieve_surrounding(move |_| {
        entered_flag.set(true);
        true
    });
    probe.focus_in();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !entered.get() && Instant::now() < deadline {
        pump(Duration::from_millis(20));
    }
    assert!(
        entered.get(),
        "compositor never entered text input on the window; run under a headless Mutter (see file docs)"
    );
    probe.focus_out();
    probe.set_client_widget(None::<&gtk4::Widget>);

    assert_eq!(is_primed(), !unprimed);
    window.close();
    pump(Duration::from_millis(50));
}
