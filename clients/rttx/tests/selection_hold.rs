//! Behaviour tests for holding managed-pane output during a selection drag.
//!
//! Regression for #1108. VTE clears an in-progress mouse selection whenever
//! it processes output mid-drag; its own terminals avoid that by pausing PTY
//! reads, but managed panes render with `vte.feed()`, so a program that keeps
//! drawing (Codex, Claude Code) made selection impossible. The pane now holds
//! output while button 1 drags and feeds it on release. These tests drive the
//! hold entry points on a real VTE and observe its text.

#![allow(clippy::doc_markdown)]

use gtk4::prelude::*;
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
        let text = screen_text(pane);
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

fn screen_text(pane: &PersistentPaneView) -> String {
    pane.vte().text_format(vte4::Format::Text).map(|text| text.to_string()).unwrap_or_default()
}

/// One frame of the #1108 emitter: braille dots redrawn in a synchronized
/// update, one shade off a light background, plus a visible marker line.
fn emitter_frame(index: usize) -> Vec<u8> {
    format!(
        "\x1b[?2026h\x1b[20;1H\x1b[38;2;201;201;197;48;2;240;237;230m\u{2801}\
         \x1b[21;15H\x1b[38;2;207;206;202;48;2;240;237;230m\u{2804}\x1b[m\x1b[?2026l\
         \x1b[{};1Hheld frame {index:02}",
        index + 2
    )
    .into_bytes()
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn output_is_held_during_drag_and_fed_in_order_on_release() {
    require_display!();
    let (_window, pane) = presented_pane();

    pane.feed_output(b"\x1b[2J\x1b[Hconversation line 00: select me\r\n");
    let before = wait_for_text(&pane, "conversation line 00");
    assert!(before.contains("conversation line 00"), "baseline output rendered: {before:?}");

    assert!(pane.begin_selection_hold(false), "a plain drag holds output");
    for index in 0..5 {
        pane.feed_output(&emitter_frame(index));
    }
    pump_events(150);
    assert!(pane.is_holding_output());
    assert_eq!(screen_text(&pane), before, "held output must not reach VTE mid-drag");

    pane.end_selection_hold();
    assert!(!pane.is_holding_output());
    let after = wait_for_text(&pane, "held frame 04");
    let positions: Vec<usize> = (0..5)
        .map(|index| {
            after
                .find(&format!("held frame {index:02}"))
                .unwrap_or_else(|| panic!("frame {index} missing after release: {after:?}"))
        })
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "frames fed in order: {after:?}");

    // Output after the drag flows straight through again.
    pane.feed_output(b"\x1b[10;1Hlive again");
    assert!(wait_for_text(&pane, "live again").contains("live again"));
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn output_is_not_held_while_app_tracks_the_mouse() {
    require_display!();
    let (_window, pane) = presented_pane();

    pane.feed_output(b"\x1b[2J\x1b[H\x1b[?1002h\x1b[?1006hmouse app\r\n");
    wait_for_text(&pane, "mouse app");
    assert!(pane.has_mouse_tracking());

    assert!(!pane.begin_selection_hold(false), "the drag belongs to the application");
    pane.feed_output(b"\x1b[5;1Hdrawn during app drag");
    assert!(wait_for_text(&pane, "drawn during app drag").contains("drawn during app drag"));
    pane.end_selection_hold();

    assert!(pane.begin_selection_hold(true), "Shift+drag still selects");
    pane.feed_output(b"\x1b[6;1Hheld during shift drag");
    pump_events(150);
    assert!(!screen_text(&pane).contains("held during shift drag"));
    pane.end_selection_hold();
    let text = wait_for_text(&pane, "held during shift drag");
    assert!(text.contains("held during shift drag"), "released after Shift+drag: {text:?}");

    // Tracking turned off by the app: plain drags hold again.
    pane.feed_output(b"\x1b[?1002l");
    assert!(!pane.has_mouse_tracking());
    assert!(pane.begin_selection_hold(false));
    pane.end_selection_hold();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn snapshot_modes_seed_mouse_tracking() {
    require_display!();
    let (_window, pane) = presented_pane();

    let modes = rttx_proto::v3::TerminalModeState {
        mouse_mode: rttx_proto::v3::MouseMode::Any as i32,
        ..Default::default()
    };
    pane.restore_interaction_modes(&modes);
    assert!(pane.has_mouse_tracking(), "armed tracking from the snapshot is honoured");
    assert!(!pane.begin_selection_hold(false));

    pane.reset_tracked_modes();
    assert!(!pane.has_mouse_tracking());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn replay_discards_output_held_before_the_restore() {
    require_display!();
    let (_window, pane) = presented_pane();

    assert!(pane.begin_selection_hold(false));
    pane.feed_output(b"\x1b[3;1Hstale pre-restore output");
    pane.vte().reset(true, true);
    pane.begin_replay();
    pane.feed_snapshot(b"\x1b[2J\x1b[Hrestored snapshot\r\n");
    pane.end_replay();
    let restored = wait_for_text(&pane, "restored snapshot");
    assert!(restored.contains("restored snapshot"), "snapshot bypasses the hold: {restored:?}");

    pane.end_selection_hold();
    pump_events(150);
    let text = screen_text(&pane);
    assert!(!text.contains("stale pre-restore output"), "superseded output not replayed: {text:?}");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn held_output_is_bounded() {
    require_display!();
    let (_window, pane) = presented_pane();

    assert!(pane.begin_selection_hold(false));
    let chunk = vec![b'x'; 1024 * 1024];
    for _ in 0..8 {
        pane.feed_output(&chunk);
    }
    assert!(!pane.is_holding_output(), "a sustained flood overflows the hold and flushes");
}

/// Call a method on Mutter's RemoteDesktop service over `bus`.
fn remote_desktop_call(
    bus: &gtk4::gio::DBusConnection,
    path: &str,
    method: &str,
    params: Option<&gtk4::glib::Variant>,
) -> gtk4::glib::Variant {
    let interface = if path == "/org/gnome/Mutter/RemoteDesktop" {
        "org.gnome.Mutter.RemoteDesktop"
    } else {
        "org.gnome.Mutter.RemoteDesktop.Session"
    };
    bus.call_sync(
        Some("org.gnome.Mutter.RemoteDesktop"),
        path,
        interface,
        method,
        params,
        None,
        gtk4::gio::DBusCallFlags::NONE,
        5000,
        None::<&gtk4::gio::Cancellable>,
    )
    .unwrap_or_else(|err| panic!("RemoteDesktop {method} failed: {err}"))
}

/// End-to-end reproduction of #1108 with real pointer events. It needs a
/// compositor that injects input, so it is opt-in. Run it under a headless
/// Mutter on a private session bus:
///
/// ```bash
/// dbus-run-session -- bash -c '
///   mutter --headless --wayland --no-x11 --wayland-display wl-test \
///          --virtual-monitor 1280x720 & sleep 2
///   WAYLAND_DISPLAY=wl-test GDK_BACKEND=wayland RTTX_SELECTION_DRAG_E2E=1 \
///     cargo test -p rttx --test selection_hold \
///     pointer_drag_selects_while_program_keeps_drawing -- --ignored --exact'
/// ```
///
/// The pane receives the #1108 emitter's frames through `feed_output` every
/// 150 ms, exactly as daemon deltas arrive, while a button-1 drag is injected
/// through `org.gnome.Mutter.RemoteDesktop`. Without the hold, the first frame
/// processed mid-drag clears the selection.
#[test]
#[ignore = "requires isolated GTK harness"]
fn pointer_drag_selects_while_program_keeps_drawing() {
    use std::cell::Cell;
    use std::fmt::Write as _;
    use std::rc::Rc;

    const BTN_LEFT: i32 = 0x110;

    if std::env::var_os("RTTX_SELECTION_DRAG_E2E").is_none() {
        eprintln!("SKIPPED: set RTTX_SELECTION_DRAG_E2E=1 under a headless Mutter (see test docs)");
        return;
    }
    require_display!();

    let bus = gtk4::gio::bus_get_sync(gtk4::gio::BusType::Session, None::<&gtk4::gio::Cancellable>)
        .expect("session bus");
    let created =
        remote_desktop_call(&bus, "/org/gnome/Mutter/RemoteDesktop", "CreateSession", None);
    let session = created.child_value(0).str().expect("session object path").to_owned();
    remote_desktop_call(&bus, &session, "Start", None);
    let move_pointer = |dx: f64, dy: f64| {
        remote_desktop_call(
            &bus,
            &session,
            "NotifyPointerMotionRelative",
            Some(&(dx, dy).to_variant()),
        );
    };
    let button = |pressed: bool| {
        remote_desktop_call(
            &bus,
            &session,
            "NotifyPointerButton",
            Some(&(BTN_LEFT, pressed).to_variant()),
        );
    };

    let pane = PersistentPaneView::new("pane-e2e", "runtime-e2e");
    let window = gtk4::Window::new();
    window.set_child(Some(&pane));
    window.fullscreen();
    window.present();
    pump_events(1000);

    let mut lines = String::from("\x1b[?25l");
    for index in 0..30 {
        let _ =
            write!(lines, "conversation line {index:02}: select me while the animation runs\r\n");
    }
    pane.feed_output(lines.as_bytes());
    wait_for_text(&pane, "conversation line 29");

    // The emitter: braille dots redrawn on the bottom rows in synchronized
    // updates, one shade off a light background.
    let frames = Rc::new(Cell::new(0_u64));
    let emitter_pane = pane.clone();
    let emitter_frames = Rc::clone(&frames);
    let emitter = gtk4::glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
        let tick = emitter_frames.get();
        emitter_frames.set(tick + 1);
        let rows = emitter_pane.vte().row_count();
        let cols = emitter_pane.vte().column_count().max(1);
        let mut frame = String::from("\x1b[?2026h");
        for row in rows - 2..=rows {
            for dot in 0..12_i64 {
                let seed = tick as i64 * 31 + row * 7 + dot * 13;
                let shade = 200 + seed.rem_euclid(36);
                let glyph = ['⠁', '⠂', '⠄', '⠈', '⠐', '⠠', '⡀', '⢀'][seed.rem_euclid(8) as usize];
                let _ = write!(
                    frame,
                    "\x1b[{row};{}H\x1b[38;2;{shade};{};{};48;2;240;237;230m{glyph}",
                    seed.rem_euclid(cols) + 1,
                    shade - 2,
                    shade - 6
                );
            }
        }
        frame.push_str("\x1b[m\x1b[?2026l");
        emitter_pane.feed_output(frame.as_bytes());
        gtk4::glib::ControlFlow::Continue
    });

    // Steer the pointer with relative motion, closing the loop on where GTK
    // reports it, so compositor acceleration does not matter.
    let pointer = Rc::new(Cell::new(None::<(f64, f64)>));
    let motion = gtk4::EventControllerMotion::new();
    let motion_pointer = Rc::clone(&pointer);
    motion.connect_motion(move |_, x, y| motion_pointer.set(Some((x, y))));
    pane.vte().add_controller(motion);
    let cell_w = pane.vte().char_width() as f64;
    let cell_h = pane.vte().char_height() as f64;
    let steer_to = |target: (f64, f64)| {
        for _ in 0..300 {
            pump_events(15);
            match pointer.get() {
                None => move_pointer(3.0, 3.0),
                Some((x, y)) => {
                    let (dx, dy) = (target.0 - x, target.1 - y);
                    if dx.abs() < cell_w / 3.0 && dy.abs() < cell_h / 3.0 {
                        return;
                    }
                    move_pointer((dx * 0.6).clamp(-80.0, 80.0), (dy * 0.6).clamp(-80.0, 80.0));
                }
            }
        }
        panic!("pointer did not reach {target:?}; last seen at {:?}", pointer.get());
    };

    steer_to((cell_w * 0.2, cell_h * 2.5));
    button(true);
    pump_events(100);
    steer_to((cell_w * 30.5, cell_h * 5.5));
    // Keep the drag going while several frames land.
    let frames_before = frames.get();
    pump_events(800);
    assert!(frames.get() >= frames_before + 3, "emitter kept drawing during the drag");
    assert!(pane.is_holding_output(), "a plain drag holds the pane's output");
    assert!(pane.vte().has_selection(), "selection is still present mid-drag");
    button(false);
    pump_events(100);
    assert!(!pane.is_holding_output(), "release feeds held output");

    // The selection survives release and the frames that follow it.
    pump_events(800);
    emitter.remove();
    remote_desktop_call(&bus, &session, "Stop", None);
    assert!(pane.vte().has_selection(), "selection survives release while the program draws");
    let selected = pane.vte().text_selected(vte4::Format::Text).map(|t| t.to_string());
    let selected = selected.unwrap_or_default();
    for line in ["conversation line 03", "conversation line 04"] {
        assert!(selected.contains(line), "selection covers the dragged lines: {selected:?}");
    }
}
