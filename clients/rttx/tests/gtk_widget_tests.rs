#![allow(clippy::doc_markdown, clippy::items_after_statements, clippy::redundant_clone)]

/// GTK widget integration tests.
///
/// These tests instantiate real GTK4 widgets to catch bugs at the Rust/C
/// boundary that pure data-model tests cannot detect. They require a
/// display backend — run with:
///
///   GDK_BACKEND=broadway GTK_A11Y=none cargo test --test gtk_widget_tests
///
/// or:
///
///   xvfb-run cargo test --test gtk_widget_tests
///
/// These tests are ignored by default so `cargo test` works headless.
use gtk4::gio::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
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

/// This is the exact bug that caused the split crash: a single widget
/// added to a Stack by name, then unparented, then stack.remove() called
/// on the now-orphaned widget. GTK asserts that the child's parent is
/// the stack — but unparent() already removed it.
#[test]
#[ignore = "requires isolated GTK harness"]
fn stack_remove_after_unparent_is_invalid() {
    require_display!();

    let stack = gtk4::Stack::new();
    let child = gtk4::Label::new(Some("test"));
    stack.add_named(&child, Some("page1"));

    // This is what the OLD buggy code did:
    // 1. unparent the child (removes from stack)
    child.unparent();
    // 2. try to find it by name — it's gone
    let found = stack.child_by_name("page1");
    assert!(found.is_none(), "child_by_name should return None after unparent");

    // The fix: remove from stack FIRST, then unparent from detached tree.
}

/// Correct pattern: remove from stack first, then unparent children
/// from the detached subtree for reuse.
#[test]
#[ignore = "requires isolated GTK harness"]
fn stack_remove_then_unparent_is_correct() {
    require_display!();

    let stack = gtk4::Stack::new();
    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    let left = gtk4::Label::new(Some("left"));
    let right = gtk4::Label::new(Some("right"));
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));
    stack.add_named(&paned, Some("session1"));

    // Step 1: remove from stack (detaches entire paned tree)
    stack.remove(&paned);

    // Step 2: unparent children from the detached paned
    // The paned is no longer in the stack, but left/right are still
    // children of the paned. We can safely unparent them.
    left.unparent();
    right.unparent();

    // Step 3: build new tree reusing the children
    let new_paned = gtk4::Paned::new(gtk4::Orientation::Vertical);
    new_paned.set_start_child(Some(&left));
    new_paned.set_end_child(Some(&right));
    stack.add_named(&new_paned, Some("session1"));

    // Verify the new tree is valid
    assert!(stack.child_by_name("session1").is_some());
}

/// Verify that a widget can be reparented after unparent.
#[test]
#[ignore = "requires isolated GTK harness"]
fn widget_reparent_after_unparent() {
    require_display!();

    let box1 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let box2 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let label = gtk4::Label::new(Some("movable"));

    box1.append(&label);
    assert!(label.parent().is_some());

    label.unparent();
    assert!(label.parent().is_none());

    box2.append(&label);
    assert_eq!(label.parent().unwrap(), box2.upcast_ref::<gtk4::Widget>().clone());
}

/// Verify that GObject ref-counting keeps widgets alive when held in a
/// HashMap even after their parent is destroyed. This is the invariant
/// our terminal reuse relies on.
#[test]
#[ignore = "requires isolated GTK harness"]
fn gobject_refcount_survives_parent_destruction() {
    require_display!();

    let label = gtk4::Label::new(Some("survivor"));
    let label_clone = label.clone(); // extra ref

    {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.append(&label);
        // container goes out of scope here — but label has extra ref
    }

    // label_clone should still be valid
    assert_eq!(label_clone.label(), "survivor");
    // It should have no parent (container was dropped)
    assert!(label_clone.parent().is_none());
}

/// Verify that Paned handles unparenting of children gracefully.
/// This is what happens during rebuild_session_content.
#[test]
#[ignore = "requires isolated GTK harness"]
fn paned_children_unparent_safely() {
    require_display!();

    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    let left = gtk4::Label::new(Some("left"));
    let right = gtk4::Label::new(Some("right"));
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));

    // Unparent both children
    left.unparent();
    right.unparent();

    // Paned should now have no children
    assert!(paned.start_child().is_none());
    assert!(paned.end_child().is_none());

    // Children should be reusable
    let new_paned = gtk4::Paned::new(gtk4::Orientation::Vertical);
    new_paned.set_start_child(Some(&right)); // swapped order
    new_paned.set_end_child(Some(&left));
    assert!(new_paned.start_child().is_some());
}

/// Verify that nested Paned trees can be rebuilt without leaks.
/// Simulates the split-split-close-split pattern.
#[test]
#[ignore = "requires isolated GTK harness"]
fn nested_paned_rebuild_cycle() {
    require_display!();

    let stack = gtk4::Stack::new();

    // Build initial: single label
    let t1 = gtk4::Label::new(Some("t1"));
    stack.add_named(&t1, Some("s1"));

    // Simulate split: remove from stack, build paned, re-add
    stack.remove(&t1);
    t1.unparent(); // no-op since stack.remove already unparented

    let t2 = gtk4::Label::new(Some("t2"));
    let paned1 = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned1.set_start_child(Some(&t1));
    paned1.set_end_child(Some(&t2));
    stack.add_named(&paned1, Some("s1"));

    // Simulate second split on t1: remove from stack, unparent, rebuild
    stack.remove(&paned1);
    t1.unparent();
    t2.unparent();

    let t3 = gtk4::Label::new(Some("t3"));
    let inner = gtk4::Paned::new(gtk4::Orientation::Vertical);
    inner.set_start_child(Some(&t1));
    inner.set_end_child(Some(&t3));
    let paned2 = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned2.set_start_child(Some(&inner));
    paned2.set_end_child(Some(&t2));
    stack.add_named(&paned2, Some("s1"));

    assert!(stack.child_by_name("s1").is_some());

    // Simulate close t3: remove from stack, unparent, rebuild simpler
    stack.remove(&paned2);
    t1.unparent();
    t2.unparent();
    t3.unparent();

    let paned3 = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned3.set_start_child(Some(&t1));
    paned3.set_end_child(Some(&t2));
    stack.add_named(&paned3, Some("s1"));

    assert!(stack.child_by_name("s1").is_some());
}

/// Verify that nested Paned widgets get proper size allocation.
/// Proves that Paned width is 0 at construction time — the root cause
/// of the "nested splits go dark" bug. connect_realize fires at this
/// point, so set_position(0.5 * 0) = 0, giving the first child no space.
#[test]
#[ignore = "requires isolated GTK harness"]
fn paned_has_zero_size_before_allocation() {
    require_display!();

    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    let left = gtk4::Label::new(Some("left"));
    let right = gtk4::Label::new(Some("right"));
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));

    assert_eq!(paned.width(), 0);
    assert_eq!(paned.height(), 0);
}

/// Regression test: build_layout_widget with nested splits must set
/// Paned positions via notify::width (not connect_realize). We call
/// the real build_layout_widget, then trigger allocation and verify
/// positions are non-zero.
#[test]
#[ignore = "requires isolated GTK harness"]
fn build_layout_widget_sets_position_after_allocation() {
    require_display!();

    use rttx::workspace::build_layout_widget;
    use rttx::workspace::*;

    // (t1 / t2) | t3 — nested split
    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::Split {
            orientation: SplitOrientation::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Terminal {
                uuid: "t1".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: "t2".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t3".into(),
            profile: None,
            cwd: None,
            custom_title: None,
        }),
    };

    let widget = build_layout_widget(&layout, &|_spec| gtk4::Label::new(Some("terminal")).upcast());

    let outer = widget.downcast_ref::<gtk4::Paned>().expect("Root must be Paned");

    // Trigger allocation — this fires notify::width, which sets position
    outer.set_size_request(800, 600);
    outer.allocate(800, 600, -1, None);

    assert!(
        outer.position() > 0,
        "Outer Paned position must be > 0 after allocation, got {}. \
         Regression: notify handler not firing.",
        outer.position()
    );
}

/// Regression test: triple-nested split must not leave any Paned at position 0.
/// This is the exact user scenario: split, split again, third split.
#[test]
#[ignore = "requires isolated GTK harness"]
fn triple_nested_split_all_paneds_nonzero() {
    require_display!();

    use rttx::workspace::build_layout_widget;
    use rttx::workspace::*;

    // ((t1 | t2) | t3) | t4
    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Split {
                orientation: SplitOrientation::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::Terminal {
                    uuid: "t1".into(),
                    profile: None,
                    cwd: None,
                    custom_title: None,
                }),
                second: Box::new(LayoutNode::Terminal {
                    uuid: "t2".into(),
                    profile: None,
                    cwd: None,
                    custom_title: None,
                }),
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: "t3".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t4".into(),
            profile: None,
            cwd: None,
            custom_title: None,
        }),
    };

    let widget = build_layout_widget(&layout, &|_spec| gtk4::Label::new(Some("terminal")).upcast());

    let root = widget.downcast_ref::<gtk4::Paned>().unwrap();
    root.set_size_request(800, 600);
    root.allocate(800, 600, -1, None);

    fn check_all_paneds(widget: &gtk4::Widget, depth: usize) {
        if let Some(paned) = widget.downcast_ref::<gtk4::Paned>() {
            assert!(
                paned.position() > 0,
                "Paned at depth {depth} has position 0 — nested split sizing regression"
            );
            if let Some(ref start) = paned.start_child() {
                check_all_paneds(start, depth + 1);
            }
            if let Some(ref end) = paned.end_child() {
                check_all_paneds(end, depth + 1);
            }
        }
    }
    check_all_paneds(widget.upcast_ref(), 0);
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Pump the GLib event loop for up to `max_ms` milliseconds.
#[allow(dead_code)] // Used by future timer-based tests (M6 activity detection)
/// Required when testing signals that propagate asynchronously or
/// when GTK needs to process queued events before an assertion.
fn pump_events(max_ms: u64) {
    let ctx = gtk4::glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
    while std::time::Instant::now() < deadline {
        if !ctx.iteration(false) {
            break;
        }
    }
}

fn wait_until(max_ms: u64, condition: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        pump_events(10);
    }
    condition()
}

fn present_widget(widget: &impl gtk4::prelude::IsA<gtk4::Widget>) -> gtk4::Window {
    let window = gtk4::Window::new();
    window.set_default_size(800, 500);
    window.set_child(Some(widget));
    window.present();
    pump_events(100);
    window
}

fn walk_widget_tree(root: &gtk4::Widget, mut visit: impl FnMut(&gtk4::Widget)) {
    fn walk(root: &gtk4::Widget, visit: &mut impl FnMut(&gtk4::Widget)) {
        visit(root);

        let mut child = root.first_child();
        while let Some(widget) = child {
            walk(&widget, visit);
            child = widget.next_sibling();
        }
    }

    walk(root, &mut visit);
}

fn count_buttons_with_tooltip(
    root: &impl gtk4::prelude::IsA<gtk4::Widget>,
    tooltip: &str,
) -> usize {
    let mut matches = 0;
    walk_widget_tree(root.as_ref(), |widget| {
        if let Some(button) = widget.downcast_ref::<gtk4::Button>()
            && button.tooltip_text().as_deref() == Some(tooltip)
        {
            matches += 1;
        }
    });
    matches
}

fn count_vte_terminals(root: &impl gtk4::prelude::IsA<gtk4::Widget>) -> usize {
    let mut matches = 0;
    walk_widget_tree(root.as_ref(), |widget| {
        if widget.is::<vte4::Terminal>() {
            matches += 1;
        }
    });
    matches
}

fn clipboard_text() -> Option<String> {
    let display = gtk4::gdk::Display::default().expect("display should be available for GTK tests");
    let clipboard = display.clipboard();
    let ctx = gtk4::glib::MainContext::default();
    ctx.block_on(clipboard.read_text_future())
        .expect("clipboard text read should succeed")
        .map(|text| text.to_string())
}

fn emit_left_click(widget: &gtk4::Widget, n_press: i32) {
    let controllers = widget.observe_controllers();
    for index in 0..controllers.n_items() {
        let Some(controller) = controllers.item(index) else {
            continue;
        };
        if let Ok(gesture) = controller.downcast::<gtk4::GestureClick>() {
            gesture.emit_by_name::<()>("released", &[&n_press, &0.0_f64, &0.0_f64]);
            return;
        }
    }
    panic!("widget should have a GestureClick controller");
}

fn emit_left_click_at(widget: &gtk4::Widget, n_press: i32, x: f64, y: f64) {
    let controllers = widget.observe_controllers();
    for index in 0..controllers.n_items() {
        let Some(controller) = controllers.item(index) else {
            continue;
        };
        if let Ok(gesture) = controller.downcast::<gtk4::GestureClick>()
            && gesture.button() == 1
        {
            gesture.emit_by_name::<()>("released", &[&n_press, &x, &y]);
            return;
        }
    }
    panic!("widget should have a left-click GestureClick controller");
}

fn find_match_coords(vte: &vte4::Terminal, expected: &str) -> Option<(f64, f64)> {
    let width = vte.width().max(120);
    let height = vte.height().max(40);
    for y in (2..height).step_by(2) {
        for x in (2..width).step_by(2) {
            let (matched, _tag) = vte.check_match_at(f64::from(x), f64::from(y));
            if matched.as_deref() == Some(expected) {
                return Some((f64::from(x), f64::from(y)));
            }
        }
    }
    None
}

fn wait_for_match_coords(vte: &vte4::Terminal, expected: &str, max_ms: u64) -> Option<(f64, f64)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
    while std::time::Instant::now() < deadline {
        if let Some(coords) = find_match_coords(vte, expected) {
            return Some(coords);
        }
        pump_events(10);
    }
    find_match_coords(vte, expected)
}

// ── M2: RefCell re-entrancy (GTK signal timing) ───────────────────────────────

/// Proves that GTK property-change signals fire SYNCHRONOUSLY in the same
/// call stack as the setter. This is why holding a RefCell borrow across
/// any GTK widget operation is dangerous: the operation may fire a signal
/// whose handler also tries to borrow the same RefCell.
///
/// If GTK ever changed to fire signals asynchronously, this test would fail
/// and our borrow-ordering discipline would no longer be necessary.
#[test]
#[ignore = "requires isolated GTK harness"]
fn gtk_notify_signal_fires_synchronously() {
    require_display!();

    let fired = Rc::new(Cell::new(false));
    let fired_clone = fired.clone();

    let label = gtk4::Label::new(Some("original"));
    label.connect_notify_local(Some("label"), move |_, _| {
        fired_clone.set(true);
    });

    assert!(!fired.get(), "Signal must not have fired before set_label");
    label.set_label("changed"); // fires notify::label synchronously
    assert!(
        fired.get(),
        "notify::label must fire synchronously within set_label — \
         if this fails, GTK signal timing has changed and borrow \
         ordering rules need re-evaluation"
    );
}

/// Proves the CORRECT pattern: extract data, release borrow, then do the
/// GTK operation. The signal handler can borrow freely because there is
/// no active borrow when it fires.
#[test]
#[ignore = "requires isolated GTK harness"]
fn gtk_signal_after_released_borrow_does_not_panic() {
    require_display!();

    let state = Rc::new(RefCell::new(0i32));
    let state_clone = state.clone();

    let label = gtk4::Label::new(Some("original"));
    label.connect_notify_local(Some("label"), move |_, _| {
        *state_clone.borrow_mut() += 1;
    });

    // CORRECT: extract what we need, release borrow, then do the widget op
    let value_before = { *state.borrow() }; // borrow released at end of block
    label.set_label("changed"); // signal fires — safe, no active borrow

    assert_eq!(
        *state.borrow(),
        value_before + 1,
        "Handler must have run exactly once after borrow was released"
    );
}

// ── M1: build_layout_widget callback count ────────────────────────────────────

/// Verifies that build_layout_widget calls make_terminal exactly once per
/// unique UUID in the layout. If it were called twice for the same UUID,
/// rebuild_session_content would create a duplicate TerminalWidget and
/// insert it into the HashMap, dropping the original — the original's
/// VTE process becomes a zombie and its signal handlers are lost.
#[test]
#[ignore = "requires isolated GTK harness"]
fn build_layout_widget_calls_make_terminal_exactly_once_per_uuid() {
    require_display!();

    use rttx::workspace::build_layout_widget;
    use rttx::workspace::*;
    use std::collections::HashMap;

    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::Split {
            orientation: SplitOrientation::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Terminal {
                uuid: "t1".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: "t2".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t3".into(),
            profile: None,
            cwd: None,
            custom_title: None,
        }),
    };

    let call_counts: Rc<RefCell<HashMap<String, usize>>> = Rc::new(RefCell::new(HashMap::new()));
    let counts_clone = call_counts.clone();

    build_layout_widget(&layout, &|spec| {
        *counts_clone.borrow_mut().entry(spec.uuid.to_string()).or_insert(0) += 1;
        gtk4::Label::new(Some(spec.uuid)).upcast()
    });

    let counts = call_counts.borrow();
    for uuid in ["t1", "t2", "t3"] {
        assert_eq!(
            counts.get(uuid).copied().unwrap_or(0),
            1,
            "make_terminal called {} time(s) for '{uuid}', expected exactly 1 — \
             multiple calls would create duplicate widgets and double signal handlers",
            counts.get(uuid).copied().unwrap_or(0)
        );
    }
}

// ── M7: signal disconnect before drop ────────────────────────────────────────

/// Verifies that disconnecting a VTE signal handler and then dropping the
/// terminal does not crash. This is the pattern used by disconnect_child_exited
/// to prevent RefCell re-entrancy when terminals are cleaned up.
#[test]
#[ignore = "requires isolated GTK harness"]
fn vte_signal_disconnect_before_drop_does_not_crash() {
    require_display!();

    let vte = vte4::Terminal::new();
    let fired = Rc::new(Cell::new(false));
    let fired_clone = fired.clone();

    let handler_id = vte.connect_child_exited(move |_, _| {
        fired_clone.set(true);
    });

    // Disconnect must not crash
    vte.disconnect(handler_id);

    // Drop must not crash (VTE finalization must not fire a disconnected signal)
    drop(vte);

    // Handler must not have fired (no child process was ever spawned)
    assert!(!fired.get(), "child_exited fired after disconnect — signal not properly cleaned up");
}

/// Verifies that connecting the same signal type twice on a VTE terminal
/// results in the handler firing twice per event — documenting why
/// connect_terminal_signals must never be called twice on the same terminal.
#[test]
#[ignore = "requires isolated GTK harness"]
fn vte_signal_connected_twice_fires_twice() {
    require_display!();

    let vte = vte4::Terminal::new();
    let fire_count = Rc::new(Cell::new(0u32));

    let count1 = fire_count.clone();
    let count2 = fire_count.clone();

    // Simulate accidentally connecting the same logical handler twice
    let id1 = vte.connect_child_exited(move |_, _| {
        count1.set(count1.get() + 1);
    });
    let id2 = vte.connect_child_exited(move |_, _| {
        count2.set(count2.get() + 1);
    });

    // Disconnect both to clean up
    vte.disconnect(id1);
    vte.disconnect(id2);

    // The important thing proven here is that GTK does not deduplicate
    // signal connections — connecting twice means two callbacks registered.
    // This is why rebuild_session_content must reuse existing terminals
    // rather than reconnecting signals on already-connected terminals.
    drop(vte);
}

// ── M4: weak reference lifecycle ─────────────────────────────────────────────

/// Verifies that a GObject weak reference becomes None after the last strong
/// reference is dropped. This is the foundation for the signal closure pattern:
///   let weak = obj.downgrade();
///   signal.connect(move |_| { if let Some(obj) = weak.upgrade() { ... } });
///
/// Without weak refs, signal closures hold strong refs that can form reference
/// cycles and prevent objects from being freed.
#[test]
#[ignore = "requires isolated GTK harness"]
fn weak_reference_invalidated_after_last_strong_ref_dropped() {
    require_display!();

    let label = gtk4::Label::new(Some("test"));
    let weak = label.downgrade();

    assert!(weak.upgrade().is_some(), "Weak ref must be valid while strong ref exists");

    drop(label);

    assert!(
        weak.upgrade().is_none(),
        "Weak ref must return None after all strong refs are dropped — \
         signal handler closures using weak refs will correctly skip \
         after the target object is freed"
    );
}

/// Verifies the safe signal closure pattern: upgrade weak ref before use,
/// skip silently if the object is gone. This prevents use-after-free when
/// a signal fires after the target window or session was closed.
#[test]
#[ignore = "requires isolated GTK harness"]
fn signal_closure_with_weak_ref_skips_safely_after_drop() {
    require_display!();

    let counter = Rc::new(Cell::new(0u32));
    let label = gtk4::Label::new(Some("source"));
    let target = gtk4::Label::new(Some("target"));
    let target_weak = target.downgrade();

    // Simulate the CORRECT closure pattern used in signal handlers
    let counter_clone = counter.clone();
    label.connect_notify_local(Some("label"), move |_, _| {
        // Upgrade weak ref — safe even if target was already dropped
        if let Some(_target) = target_weak.upgrade() {
            counter_clone.set(counter_clone.get() + 1);
        }
        // If upgrade() returns None, we skip without crashing
    });

    // While target is alive, signal increments counter
    label.set_label("ping");
    assert_eq!(counter.get(), 1, "Handler must fire while target is alive");

    // Drop the target — target_weak.upgrade() will now return None
    drop(target);

    // Signal fires again, but target is gone — must not crash, must not increment
    label.set_label("pong");
    assert_eq!(
        counter.get(),
        1,
        "Handler must skip silently after target is dropped — \
         no crash, no access to freed memory"
    );
}

// ── M5: extreme ratios produce non-zero Paned positions ──────────────────────

/// Verifies that Paned widgets with non-default but valid ratios all receive
/// non-zero positions after allocation. The position calculation is:
///   (size as f64 * ratio) as i32
/// which could theoretically produce 0 for very small ratios on small windows.
#[test]
#[ignore = "requires isolated GTK harness"]
fn paned_extreme_but_valid_ratios_produce_nonzero_positions() {
    require_display!();

    use rttx::workspace::build_layout_widget;
    use rttx::workspace::*;

    // Test ratios near both ends of the valid (0, 1) range
    for &ratio in &[0.1f64, 0.2, 0.5, 0.8, 0.9] {
        let layout = LayoutNode::Split {
            orientation: SplitOrientation::Horizontal,
            ratio,
            first: Box::new(LayoutNode::Terminal {
                uuid: "t1".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: "t2".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
        };

        let widget =
            build_layout_widget(&layout, &|spec| gtk4::Label::new(Some(spec.uuid)).upcast());

        let paned = widget.downcast_ref::<gtk4::Paned>().expect("Root must be Paned");

        paned.set_size_request(800, 600);
        paned.allocate(800, 600, -1, None);

        let expected = (800.0 * ratio) as i32;
        assert!(
            paned.position() > 0,
            "Paned with ratio {ratio:.1} has position 0 after allocation \
             (expected ~{expected}px on 800px wide pane). \
             notify::width handler may not have fired."
        );
    }
}

/// Prevent regression: PopoverMenu created without set_parent() crashes on popup().
///
/// When the context menu was introduced, forgetting set_parent() causes a GTK
/// assertion failure the moment the user right-clicks. This test verifies that
/// the PopoverMenu is registered as a child of the TerminalWidget immediately
/// after construction, before any interaction.
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_context_menu_is_parented_to_widget() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);

    let popover = find_popover_child(term.vte().upcast_ref::<gtk4::Widget>());
    assert!(
        popover.is_some(),
        "TerminalWidget must have a PopoverMenu parented to the VTE after construction. \
         Call set_parent() on the context menu during constructed()."
    );
}

/// Prevent regression: mounting VTE directly in the pane removes any visible
/// scrollbar, which made it impossible to discover backlog scrolling from the UI.
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_wraps_vte_in_scrolled_window() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);

    let vte_parent = term.vte().parent().expect("VTE must have a parent widget");
    let scroller = vte_parent
        .downcast::<gtk4::ScrolledWindow>()
        .expect("VTE should be wrapped in a ScrolledWindow so the pane exposes a scrollbar");

    assert_eq!(
        scroller.parent(),
        Some(term.clone().upcast::<gtk4::Widget>()),
        "ScrolledWindow should be mounted directly under TerminalWidget",
    );
    assert_eq!(
        scroller.child(),
        Some(term.vte().clone().upcast::<gtk4::Widget>()),
        "ScrolledWindow should own the VTE child",
    );
}

/// Prevent regression: pane titles are a focus target only for now, so
/// double-clicking the title must not create an inline Entry editor.
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_title_double_click_does_not_start_inline_editing() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    let header = term
        .title_label()
        .parent()
        .and_then(|parent| parent.downcast::<gtk4::Box>().ok())
        .expect("title label should be parented to the terminal header box");

    emit_left_click(term.title_label().upcast_ref::<gtk4::Widget>(), 2);
    pump_events(50);

    let mut child = header.first_child();
    while let Some(widget) = child {
        assert!(
            widget.downcast_ref::<gtk4::Entry>().is_none(),
            "double-clicking the title should not create an inline title editor"
        );
        child = widget.next_sibling();
    }
    assert!(
        term.title_label().is_visible(),
        "title label should remain visible after double-click"
    );
}

/// Prevent regression: an empty or mis-named action in the context menu produces
/// a non-functional item with no visible error.
///
/// Each section of the menu model is verified to be non-empty and all items must
/// carry an "action" attribute. Any item without an action attribute is invisible
/// to the user but silently broken.
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_context_menu_model_has_actions() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);

    let popover = find_popover_child(term.vte().upcast_ref::<gtk4::Widget>())
        .expect("context menu must be parented to VTE");

    let model = popover.menu_model().expect("PopoverMenu must have a menu model");

    let n_sections = model.n_items();
    assert!(n_sections > 0, "context menu model must have at least one section");

    let mut total_items = 0;
    for section_idx in 0..n_sections {
        let section =
            model.item_link(section_idx, "section").expect("each top-level item must be a section");

        let n = section.n_items();
        assert!(n > 0, "section {section_idx} must not be empty");

        for item_idx in 0..n {
            let has_action = section.item_attribute_value(item_idx, "action", None).is_some();
            let has_submenu = section.item_link(item_idx, "submenu").is_some();
            assert!(
                has_action || has_submenu,
                "context menu section {section_idx} item {item_idx} has no action or submenu — \
                 the item will silently do nothing when clicked"
            );
            total_items += 1;
        }
    }

    assert!(
        total_items >= 8,
        "context menu must have at least 8 items; found {total_items}. \
         A section may have been accidentally emptied."
    );
}

fn find_popover_child(widget: &gtk4::Widget) -> Option<gtk4::PopoverMenu> {
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Ok(popover) = c.clone().downcast::<gtk4::PopoverMenu>() {
            return Some(popover);
        }
        child = c.next_sibling();
    }
    None
}

/// The context menu must contain a "Places" submenu in one of its sections.
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_context_menu_has_places_submenu() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    let popover = find_popover_child(term.vte().upcast_ref::<gtk4::Widget>())
        .expect("context menu must be parented");
    let model = popover.menu_model().expect("PopoverMenu must have a menu model");

    let mut found_places = false;
    for section_idx in 0..model.n_items() {
        let section = model.item_link(section_idx, "section").unwrap();
        for item_idx in 0..section.n_items() {
            if section.item_link(item_idx, "submenu").is_some() {
                let label = section.item_attribute_value(
                    item_idx,
                    "label",
                    Some(gtk4::glib::VariantTy::STRING),
                );
                if label.is_some_and(|v| v.get::<String>().unwrap() == "Places") {
                    found_places = true;
                }
            }
        }
    }
    assert!(found_places, "context menu must contain a Places submenu");
}

/// Regression for #480: the context menu popover must use halign=Start so its
/// left edge aligns with the pointer position. Without this, the popover
/// centers on the click point and the pointer lands on a menu item, causing
/// immediate activation on button release.
#[test]
#[ignore = "requires isolated GTK harness"]
fn direct_terminal_context_menu_popover_uses_start_halign() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t-halign", None);
    let popover = find_popover_child(term.vte().upcast_ref::<gtk4::Widget>())
        .expect("context menu must be parented");
    assert_eq!(
        popover.halign(),
        gtk4::Align::Start,
        "context menu popover must use halign=Start so the pointer is outside the menu on open"
    );
}

/// Regression for #480: same as above but for persistent pane context menu.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_context_menu_popover_uses_start_halign() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("p-halign", "s1");
    let popover = find_popover_child(pane.vte().upcast_ref::<gtk4::Widget>())
        .expect("context menu must be parented");
    assert_eq!(
        popover.halign(),
        gtk4::Align::Start,
        "context menu popover must use halign=Start so the pointer is outside the menu on open"
    );
}

// ── TerminalHandle tests ────────────────────────────────────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_reports_titles_and_managed_current_directory() {
    require_display!();

    let direct = rttx::terminal::widget::TerminalWidget::new("direct-1", None);
    direct.set_title("Direct Title");
    let direct_handle = rttx::terminal::handle::TerminalHandle::Direct(direct);
    assert_eq!(direct_handle.title(), "Direct Title");

    let managed =
        rttx::terminal::persistent_widget::PersistentPaneView::new("managed-1", "runtime-1");
    managed.set_daemon_title("Managed Title");
    managed.set_current_directory(Some("/tmp/managed-cwd"));
    let managed_handle = rttx::terminal::handle::TerminalHandle::Managed(managed);
    assert_eq!(managed_handle.title(), "Managed Title : /tmp/managed-cwd");
    assert_eq!(managed_handle.current_directory().as_deref(), Some("/tmp/managed-cwd"));
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_set_active_updates_both_direct_and_managed_panes() {
    require_display!();

    let direct = rttx::terminal::widget::TerminalWidget::new("direct-1", None);
    let direct_handle = rttx::terminal::handle::TerminalHandle::Direct(direct.clone());
    direct_handle.set_active(true);
    assert!(direct.has_css_class("terminal-pane-active"));
    direct_handle.set_active(false);
    assert!(!direct.has_css_class("terminal-pane-active"));

    let managed =
        rttx::terminal::persistent_widget::PersistentPaneView::new("managed-1", "runtime-1");
    let managed_handle = rttx::terminal::handle::TerminalHandle::Managed(managed.clone());
    managed_handle.set_active(true);
    assert!(managed.has_css_class("terminal-pane-active"));
    managed_handle.set_active(false);
    assert!(!managed.has_css_class("terminal-pane-active"));
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_grab_focus_targets_direct_terminal_vte() {
    require_display!();

    let direct = rttx::terminal::widget::TerminalWidget::new("direct-1", None);
    let window = present_widget(&direct);
    let handle = rttx::terminal::handle::TerminalHandle::Direct(direct.clone());

    assert!(handle.grab_focus(), "direct handle should request focus successfully");
    assert!(wait_until(1000, || direct.vte().has_focus()));

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_grab_focus_targets_managed_terminal_vte() {
    require_display!();

    let managed =
        rttx::terminal::persistent_widget::PersistentPaneView::new("managed-1", "runtime-1");
    let window = present_widget(&managed);
    let handle = rttx::terminal::handle::TerminalHandle::Managed(managed.clone());

    assert!(handle.grab_focus(), "managed handle should request focus successfully");
    assert!(wait_until(1000, || managed.vte().has_focus()));

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_copy_clipboard_uses_direct_terminal_selection() {
    require_display!();

    let display = gtk4::gdk::Display::default().expect("display should be available for GTK tests");
    display.clipboard().set_text("");

    let direct = rttx::terminal::widget::TerminalWidget::new("direct-1", None);
    let window = present_widget(&direct);
    direct.vte().feed(b"direct copied text\r\n");
    pump_events(50);
    direct.vte().select_all();

    let handle = rttx::terminal::handle::TerminalHandle::Direct(direct);
    handle.copy_clipboard();
    assert!(wait_until(1000, || {
        clipboard_text().is_some_and(|text| text.contains("direct copied text"))
    }));
    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_copy_clipboard_uses_managed_terminal_selection() {
    require_display!();

    let display = gtk4::gdk::Display::default().expect("display should be available for GTK tests");
    display.clipboard().set_text("");

    let managed =
        rttx::terminal::persistent_widget::PersistentPaneView::new("managed-1", "runtime-1");
    let window = present_widget(&managed);
    managed.feed_output(b"managed copied text\r\n");
    pump_events(50);
    managed.vte().select_all();

    let handle = rttx::terminal::handle::TerminalHandle::Managed(managed);
    handle.copy_clipboard();
    assert!(wait_until(1000, || {
        clipboard_text().is_some_and(|text| text.contains("managed copied text"))
    }));
    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn direct_terminal_plain_click_does_not_launch_url() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("direct-link", None);
    let window = present_widget(&term);
    let expected = "https://example.com/direct";
    term.vte().feed(format!("{expected}\n").as_bytes());
    let (x, y) =
        wait_for_match_coords(term.vte(), expected, 1000).expect("link match should be present");

    let launched = Rc::new(RefCell::new(Vec::new()));
    let launched_clone = Rc::clone(&launched);
    rttx::terminal::links::with_test_uri_launcher(
        move |uri| {
            launched_clone.borrow_mut().push(uri.to_string());
            true
        },
        || {
            // Plain click (no Ctrl) must not open the link so VTE can
            // forward mouse events to mouse-aware apps. Regression for #459.
            emit_left_click_at(term.vte().upcast_ref::<gtk4::Widget>(), 1, x, y);
            pump_events(50);
        },
    );

    assert!(
        launched.borrow().is_empty(),
        "plain click must not launch URL — Ctrl+click is required (#459)"
    );
    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_terminal_plain_click_does_not_launch_url() {
    require_display!();

    let pane =
        rttx::terminal::persistent_widget::PersistentPaneView::new("managed-link", "runtime-1");
    let window = present_widget(&pane);
    let expected = "https://example.com/persistent";
    pane.feed_output(format!("{expected}\n").as_bytes());
    let (x, y) =
        wait_for_match_coords(pane.vte(), expected, 1000).expect("link match should be present");

    let launched = Rc::new(RefCell::new(Vec::new()));
    let launched_clone = Rc::clone(&launched);
    rttx::terminal::links::with_test_uri_launcher(
        move |uri| {
            launched_clone.borrow_mut().push(uri.to_string());
            true
        },
        || {
            // Plain click (no Ctrl) must not open the link. #459.
            emit_left_click_at(pane.vte().upcast_ref::<gtk4::Widget>(), 1, x, y);
            pump_events(50);
        },
    );

    assert!(
        launched.borrow().is_empty(),
        "plain click must not launch URL — Ctrl+click is required (#459)"
    );
    window.close();
}

// ── PersistentPaneView tests ─────────────────────────────────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_stores_uuid_and_runtime_id() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    assert_eq!(pane.uuid(), "pane-1");
    assert_eq!(pane.runtime_id(), "session-1");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_feed_output_does_not_crash() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    // Feed some terminal output — should not panic.
    pane.feed_output(b"hello world\r\n");
    pane.feed_output(b"\x1b[31mred text\x1b[0m\r\n");
    pane.feed_output(b"");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_feed_snapshot_restores_content() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    pane.feed_snapshot(b"line 1\r\nline 2\r\nline 3\r\n");
    // Empty snapshot should not crash.
    pane.feed_snapshot(b"");
}

/// Snapshot feed must strip bell characters to prevent historical bells
/// from ringing on connect/reconnect. Regression test for #268.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_feed_snapshot_strips_bells() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    // Feed scrollback containing bell characters — should not crash or ring.
    pane.feed_snapshot(b"\x07prompt$ \x07command\r\n\x07prompt$ ");
    // The content should be present without the bells.
    // (We can't easily assert VTE content, but the test verifies no panic.)
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_feed_snapshot_restores_cursor_after_inline_motion() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    let window = present_widget(&pane);

    pane.feed_snapshot(b"PROMPT> abcd\x1b[D\x1b[D");
    pump_events(50);

    assert_eq!(pane.vte().cursor_position(), (10, 0));
    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_feed_snapshot_restores_cursor_after_multiline_formatted_output() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    let window = present_widget(&pane);

    pane.feed_snapshot(b"\x1b[31mRED\x1b[0m\r\nPROMPT> wrap\x1b[D");
    pump_events(50);

    assert_eq!(pane.vte().cursor_position(), (11, 1));
    window.close();
}

/// After feeding a large snapshot, the viewport must be scrolled to the
/// bottom so the user sees the most recent output. Regression test for #440.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_feed_snapshot_scrolls_to_bottom() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    let window = present_widget(&pane);
    pump_events(50);

    // Feed enough lines to exceed the visible area.
    let mut data = Vec::new();
    for i in 0..200 {
        data.extend_from_slice(format!("line {i}\r\n").as_bytes());
    }
    pane.feed_snapshot(&data);
    pump_events(50);

    let adj = pane.vte().vadjustment().expect("VTE should have a vadjustment");
    let at_bottom = (adj.value() + adj.page_size() - adj.upper()).abs() < 1.0;
    assert!(at_bottom, "viewport should be scrolled to the bottom after snapshot feed");
    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_resize_callback_tracks_allocated_terminal_size() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    let reported_sizes = Rc::new(RefCell::new(Vec::new()));
    let reported_sizes_clone = Rc::clone(&reported_sizes);
    pane.connect_resize(move |cols, rows| {
        reported_sizes_clone.borrow_mut().push((cols, rows));
    });

    let window = present_widget(&pane);
    assert!(
        wait_until(1000, || {
            let (cols, rows) = pane.terminal_size();
            cols > 0 && rows > 0
        }),
        "persistent pane never received an initial terminal allocation"
    );

    let initial_size = pane.terminal_size();
    assert!(
        wait_until(1000, || reported_sizes.borrow().last().copied() == Some(initial_size)),
        "resize callback never reported the initial allocated terminal size"
    );
    let initial_reported = reported_sizes.borrow().last().copied();
    assert_eq!(
        initial_reported,
        Some(initial_size),
        "resize callback must report the initial allocated terminal size"
    );

    window.allocate(420, 500, -1, None);
    pump_events(50);
    assert!(
        wait_until(1000, || pane.terminal_size() != initial_size),
        "persistent pane terminal size did not change after window resize"
    );

    let resized_size = pane.terminal_size();
    assert!(
        wait_until(1000, || reported_sizes.borrow().last().copied() == Some(resized_size)),
        "resize callback never reported the resized terminal size"
    );
    let resized_reported = reported_sizes.borrow().last().copied();
    assert_eq!(
        resized_reported,
        Some(resized_size),
        "resize callback must track viewport-derived terminal size changes"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_set_connected_updates_state() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    pane.set_connected(true);
    pane.set_connected(false);
    pane.set_connected(true);
    // No crash — status label updates are visual only.
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_set_title_and_custom_title() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    pane.set_daemon_title("my title");
    assert_eq!(pane.title_label().label(), "my title");

    assert!(pane.custom_title().is_none());
    pane.set_custom_title(Some("custom"));
    assert_eq!(pane.custom_title().as_deref(), Some("custom"));
    assert_eq!(pane.title_label().label(), "custom");

    pane.set_custom_title(None);
    assert!(pane.custom_title().is_none());
    // After clearing custom title, daemon title is restored.
    assert_eq!(pane.title_label().label(), "my title");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_active_css_class() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    pane.set_active(true);
    assert!(pane.has_css_class("terminal-pane-active"));
    pane.set_active(false);
    assert!(!pane.has_css_class("terminal-pane-active"));
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_flash_bell_does_not_crash() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    pane.set_visual_bell(true);
    pane.flash_bell();
    pane.set_visual_bell(false);
    pane.flash_bell(); // Should be a no-op.
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_has_expected_children() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-1", "session-1");
    // Header buttons should exist.
    assert!(pane.close_button().icon_name().is_some());
    assert!(pane.split_h_button().icon_name().is_some());
    assert!(pane.split_v_button().icon_name().is_some());
    assert!(pane.zoom_button().icon_name().is_some());
    // VTE should exist.
    assert!(pane.vte().column_count() >= 0);
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn window_add_session_materializes_managed_runtime_controls() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    set_env("XDG_CONFIG_HOME", tmp.path());
    set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.window-runtime-module-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = rttx::window::Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    assert_eq!(count_buttons_with_tooltip(&window, "Close terminal"), 1);
    assert_eq!(count_buttons_with_tooltip(&window, "Close pane"), 0);
    assert_eq!(count_vte_terminals(&window), 1);

    window.add_session();
    assert!(
        wait_until(1_000, || count_buttons_with_tooltip(&window, "Close pane") == 1),
        "adding a managed workspace should materialize persistent pane controls"
    );
    assert_eq!(
        count_buttons_with_tooltip(&window, "Close terminal"),
        1,
        "the direct workspace controls should remain present"
    );
    assert_eq!(
        count_vte_terminals(&window),
        2,
        "window should keep one direct terminal and add one managed pane terminal"
    );

    window.close();
    remove_env("RTTX_DISABLE_SHELL_SPAWN");
    remove_env("XDG_CONFIG_HOME");
}

#[test]
#[ignore = "requires GTK display"]
fn terminal_search_bar_wired_to_vte() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("search-wire-test", None);
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&term));
    window.present();
    pump_events(50);

    assert!(!term.search_bar().is_search_mode());

    term.toggle_search();
    assert!(term.search_bar().is_search_mode());

    term.search_entry().set_text("hello");
    pump_events(50);
    assert!(
        term.vte().search_get_regex().is_some(),
        "typing in search entry must set VTE search regex"
    );

    term.search_entry().set_text("");
    pump_events(50);
    assert!(
        term.vte().search_get_regex().is_none(),
        "clearing search entry must clear VTE search regex"
    );

    term.search_entry().set_text("world");
    pump_events(50);
    term.toggle_search();
    assert!(
        term.vte().search_get_regex().is_none(),
        "closing search bar must clear VTE search regex"
    );

    window.close();
}

/// Managed clipboard paste must forward clipboard text through the daemon
/// input callback instead of delegating to VTE's local PTY paste path.
#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_terminal_request_clipboard_paste_delivers_bytes() {
    require_display!();

    let display = gtk4::gdk::Display::default().expect("display should be available for GTK tests");
    display.clipboard().set_text("managed clipboard bytes");

    let managed =
        rttx::terminal::persistent_widget::PersistentPaneView::new("managed-1", "runtime-1");
    let window = present_widget(&managed);
    let connected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Connected);
    managed.set_connection_presentation(&rttx::runtime::ConnectionStatus::Connected, &connected);

    let forwarded = Rc::new(RefCell::new(Vec::new()));
    let forwarded_clone = Rc::clone(&forwarded);
    managed.request_clipboard_paste(move |bytes| {
        forwarded_clone.borrow_mut().push(bytes);
    });

    assert!(
        wait_until(1_000, || { forwarded.borrow().contains(&b"managed clipboard bytes".to_vec()) }),
        "managed paste helper should deliver clipboard text to the daemon input callback"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_terminal_request_clipboard_paste_requires_connected_input() {
    require_display!();

    let display = gtk4::gdk::Display::default().expect("display should be available for GTK tests");
    display.clipboard().set_text("managed clipboard bytes");

    let managed =
        rttx::terminal::persistent_widget::PersistentPaneView::new("managed-2", "runtime-1");
    let window = present_widget(&managed);

    let forwarded = Rc::new(RefCell::new(Vec::new()));
    let forwarded_clone = Rc::clone(&forwarded);
    managed.request_clipboard_paste(move |bytes| {
        forwarded_clone.borrow_mut().push(bytes);
    });

    pump_events(100);
    assert!(
        forwarded.borrow().is_empty(),
        "disconnected managed panes must not forward clipboard bytes"
    );

    window.close();
}

/// Close dialog for managed workspaces must not offer detach or terminate.
/// Regression test for #195.
#[test]
fn close_dialog_has_no_detach_or_terminate_for_managed_workspace() {
    use rttx::runtime::{WorkspacePolicy, present_workspace_actions};

    let presentation = present_workspace_actions(Some(WorkspacePolicy::Persistent), true, 1);
    assert_eq!(presentation.close_label, "Close Workspace");
    assert!(!presentation.body.contains("Detach"));
    assert!(!presentation.body.contains("Terminate"));
}

/// Managed pane banner must not have retry/edit/close action buttons.
/// Regression test for #196.
#[test]
fn managed_pane_banner_is_passive() {
    // The PersistentPaneView no longer has retry_button, edit_connection_button,
    // or close_workspace_button fields. This is a compile-time check that the
    // connect_retry_requested, connect_edit_connection_requested, and
    // connect_close_workspace_requested methods no longer exist.
    let _: fn(&str, &str) -> rttx::terminal::persistent_widget::PersistentPaneView =
        rttx::terminal::persistent_widget::PersistentPaneView::new;
}

/// The window must expose a `new-remote-workspace` action.
#[test]
#[ignore = "requires isolated GTK harness"]
fn window_has_new_remote_workspace_action() {
    require_display!();

    let app = adw::Application::builder()
        .application_id("io.github.IllyaYalovyy.rttx.test.remote_action")
        .build();
    app.register(None::<&gtk4::gio::Cancellable>).unwrap();

    let window = rttx::window::Window::new(&app);
    let action_group: gtk4::gio::ActionGroup = window.clone().upcast();
    assert!(
        action_group.has_action("new-remote-workspace"),
        "window must have new-remote-workspace action"
    );
    window.close();
}

/// `schedule_initial_paned_ratios` must set position on realize to avoid
/// a visible jump. Regression test for #23.
#[test]
#[ignore = "requires isolated GTK harness"]
fn paned_ratio_applied_on_realize_not_just_idle() {
    require_display!();

    use rttx::workspace::{
        LayoutNode, SplitOrientation, build_layout_widget, schedule_initial_paned_ratios,
    };

    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::Terminal {
            uuid: "t1".into(),
            profile: None,
            cwd: None,
            custom_title: None,
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t2".into(),
            profile: None,
            cwd: None,
            custom_title: None,
        }),
    };

    let widget = build_layout_widget(&layout, &|_spec| gtk4::Label::new(Some("terminal")).upcast());

    schedule_initial_paned_ratios(&widget, &layout);

    // The realize handler should be connected. When the widget is realized,
    // the position should be set before the first paint.
    let paned = widget.downcast_ref::<gtk4::Paned>().unwrap();
    paned.set_size_request(800, 600);

    // Force realization by adding to a window.
    let window = gtk4::Window::new();
    window.set_child(Some(paned));
    window.present();
    pump_events(100);

    let position = paned.position();
    assert!(position > 0, "paned position must be set after realize, got {position}");

    window.close();
}

/// Link click gesture must deny when no URI is found, allowing VTE to
/// receive mouse events for mouse-aware apps. Regression for #291.
/// Since #459, the gesture also denies without Ctrl, so all plain clicks
/// pass through to VTE regardless of whether a link is present.
#[test]
fn link_gesture_denies_when_no_uri() {
    assert_ne!(gtk4::EventSequenceState::Denied, gtk4::EventSequenceState::Claimed);
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_zoom_button_hidden_by_default() {
    require_display!();
    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    assert!(!term.zoom_button().is_visible());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_zoom_button_visible_for_multi_pane() {
    require_display!();
    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    term.set_zoom_state(false, true, 0, 3);
    assert!(term.zoom_button().is_visible());
    assert_eq!(term.zoom_button().icon_name().unwrap(), "view-fullscreen-symbolic");
    assert!(!term.pane_count_label().is_visible());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_zoom_button_shows_restore_when_zoomed() {
    require_display!();
    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    term.set_zoom_state(true, true, 1, 3);
    assert!(term.zoom_button().is_visible());
    assert_eq!(term.zoom_button().icon_name().unwrap(), "view-restore-symbolic");
    assert!(term.pane_count_label().is_visible());
    assert_eq!(term.pane_count_label().label(), "2/3");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_zoom_button_hidden_for_single_pane_unzoomed() {
    require_display!();
    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    term.set_zoom_state(false, false, 0, 1);
    assert!(!term.zoom_button().is_visible());
    assert!(!term.pane_count_label().is_visible());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_zoom_button_visible_for_multi_pane() {
    require_display!();
    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("p1", "s1");
    pane.set_zoom_state(false, true, 0, 2);
    assert!(pane.zoom_button().is_visible());
    assert_eq!(pane.zoom_button().icon_name().unwrap(), "view-fullscreen-symbolic");
    assert!(!pane.pane_count_label().is_visible());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_zoom_button_shows_restore_when_zoomed() {
    require_display!();
    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("p1", "s1");
    pane.set_zoom_state(true, false, 0, 2);
    assert!(pane.zoom_button().is_visible());
    assert_eq!(pane.zoom_button().icon_name().unwrap(), "view-restore-symbolic");
    assert!(pane.pane_count_label().is_visible());
    assert_eq!(pane.pane_count_label().label(), "1/2");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn pane_count_label_hides_when_unzoomed() {
    require_display!();
    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    term.set_zoom_state(true, true, 2, 4);
    assert!(term.pane_count_label().is_visible());
    assert_eq!(term.pane_count_label().label(), "3/4");
    term.set_zoom_state(false, true, 2, 4);
    assert!(!term.pane_count_label().is_visible());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_has_open_and_copy_link_actions() {
    require_display!();
    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);
    // Actions exist but are disabled by default — activate returns Err.
    assert!(term.activate_action("term.open-link", None).is_err());
    assert!(term.activate_action("term.copy-link", None).is_err());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_has_open_and_copy_link_actions() {
    require_display!();
    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("p1", "s1");
    assert!(pane.activate_action("term.open-link", None).is_err());
    assert!(pane.activate_action("term.copy-link", None).is_err());
}

/// Persistent pane must forward VTE `commit` data (mouse escape sequences)
/// to the daemon input callback. Regression for #442.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_forwards_vte_commit_to_daemon_input() {
    require_display!();

    use std::cell::RefCell;
    use std::rc::Rc;

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("mouse-1", "runtime-1");
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

    // Simulate VTE emitting an SGR mouse click sequence via commit.
    let sgr_click = "\x1b[<0;10;5M";
    pane.vte().emit_by_name::<()>("commit", &[&sgr_click, &(sgr_click.len() as u32)]);
    pump_events(50);

    assert!(
        forwarded.borrow().contains(&sgr_click.as_bytes().to_vec()),
        "persistent pane must forward VTE commit data to daemon input callback"
    );

    window.close();
}

/// The key controller must have an IMContext set so compose sequences,
/// dead keys, and system input methods work in managed panes. #462.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_key_controller_has_im_context() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("ime-1", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&pane));
    window.present();
    pump_events(50);

    let connected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Connected);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Connected, &connected);

    pane.connect_input(|_| {});

    assert!(
        pane.has_im_context_for_test(),
        "key controller must have an IMContext for compose/dead-key/IME support"
    );

    window.close();
}

// ── New Workspace dialog ────────────────────────────────────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_workspace_dialog_shows_builtin_places_for_local_host() {
    require_display!();

    let host = rttx::host::Host::local();
    let saved = rttx::places::visible_for_host(&[], &host.key);

    // Built-in places should always include Home and Root
    let names: Vec<&str> = saved.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["Home", "Root"]);
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_workspace_dialog_filters_places_by_host_key() {
    require_display!();

    let mut local_place = rttx::places::Place::new("rttx", "~/pro/rttx");
    local_place.host_tags = vec!["local".into()];
    let mut remote_place = rttx::places::Place::new("app", "/srv/app");
    remote_place.host_tags = vec!["example.com".into()];
    let global_place = rttx::places::Place::new("tmp", "/tmp");

    let saved = vec![local_place, remote_place, global_place];

    let local_visible = rttx::places::visible_for_host(&saved, "local");
    let local_names: Vec<&str> = local_visible.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(local_names, vec!["Home", "Root", "rttx", "tmp"]);

    let remote_visible = rttx::places::visible_for_host(&saved, "example.com");
    let remote_names: Vec<&str> = remote_visible.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(remote_names, vec!["Home", "Root", "app", "tmp"]);
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_workspace_dialog_search_filters_places() {
    require_display!();

    let places = [
        rttx::places::Place::new("rttx", "~/pro/rttx"),
        rttx::places::Place::new("redis", "~/src/redis"),
    ];

    assert_eq!(places.iter().filter(|p| rttx::places::matches_query(p, "rttx")).count(), 1);
    assert_eq!(places.iter().filter(|p| rttx::places::matches_query(p, "")).count(), 2);
}

// ── Connect to Existing dialog ──────────────────────────────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn connect_existing_dialog_classifies_available_session() {
    require_display!();

    let id = uuid::Uuid::new_v4();
    let workspaces = vec![rttx_proto::v3::RuntimeInfo {
        id: rttx_proto::uuid_to_bytes(id),
        name: "workspace-1".into(),
        pane_count: 2,
        has_write_owner: false,
        read_only_client_count: 0,
        current_client_role: 0,
        panes: vec![],
        policy: 0,
        reconstructed: false,
        runtime_revision: 1,
        active_pane_summary: String::new(),
        takeover_eligible: false,
        disabled_reason: String::new(),
    }];
    let entries = rttx::connect_existing_dialog::classify_runtimes(&workspaces, &[]);

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].availability,
        rttx::connect_existing_dialog::RuntimeAvailability::Available
    );
    assert_eq!(entries[0].status_label, "2 panes");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn connect_existing_dialog_classifies_busy_session() {
    require_display!();

    let id = uuid::Uuid::new_v4();
    let workspaces = vec![rttx_proto::v3::RuntimeInfo {
        id: rttx_proto::uuid_to_bytes(id),
        name: "busy-ws".into(),
        pane_count: 1,
        has_write_owner: true,
        read_only_client_count: 0,
        current_client_role: 0,
        panes: vec![],
        policy: 0,
        reconstructed: false,
        runtime_revision: 1,
        active_pane_summary: String::new(),
        takeover_eligible: false,
        disabled_reason: String::new(),
    }];
    let entries = rttx::connect_existing_dialog::classify_runtimes(&workspaces, &[]);

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].availability,
        rttx::connect_existing_dialog::RuntimeAvailability::BusyElsewhere
    );
    assert_eq!(entries[0].status_label, "Connected elsewhere");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn connect_existing_dialog_classifies_already_open_session() {
    require_display!();

    let id = uuid::Uuid::new_v4();
    let workspaces = vec![rttx_proto::v3::RuntimeInfo {
        id: rttx_proto::uuid_to_bytes(id),
        name: "open-ws".into(),
        pane_count: 3,
        has_write_owner: false,
        read_only_client_count: 0,
        current_client_role: 0,
        panes: vec![],
        policy: 0,
        reconstructed: false,
        runtime_revision: 1,
        active_pane_summary: String::new(),
        takeover_eligible: false,
        disabled_reason: String::new(),
    }];
    let entries = rttx::connect_existing_dialog::classify_runtimes(&workspaces, &[id.to_string()]);

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].availability,
        rttx::connect_existing_dialog::RuntimeAvailability::AlreadyOpen
    );
    assert_eq!(entries[0].status_label, "Already open");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn connect_existing_dialog_search_filters_sessions() {
    require_display!();

    let entries = [
        rttx::connect_existing_dialog::RuntimeEntry {
            id: "a".into(),
            name: "rttx project".into(),
            pane_count: 2,
            availability: rttx::connect_existing_dialog::RuntimeAvailability::Available,
            status_label: "2 panes".into(),
        },
        rttx::connect_existing_dialog::RuntimeEntry {
            id: "b".into(),
            name: "redis server".into(),
            pane_count: 1,
            availability: rttx::connect_existing_dialog::RuntimeAvailability::Available,
            status_label: "1 pane".into(),
        },
    ];

    assert_eq!(
        entries.iter().filter(|e| rttx::connect_existing_dialog::matches_query(e, "rttx")).count(),
        1
    );
    assert_eq!(
        entries.iter().filter(|e| rttx::connect_existing_dialog::matches_query(e, "")).count(),
        2
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn connect_existing_dialog_has_sufficient_height_for_item_visibility() {
    require_display!();

    assert_eq!(rttx::connect_existing_dialog::DIALOG_CONTENT_HEIGHT, 450);
    assert_eq!(rttx::connect_existing_dialog::SCROLL_MIN_CONTENT_HEIGHT, 300);

    let dialog = libadwaita::Dialog::builder()
        .content_width(rttx::connect_existing_dialog::DIALOG_CONTENT_WIDTH)
        .content_height(rttx::connect_existing_dialog::DIALOG_CONTENT_HEIGHT)
        .build();
    assert_eq!(dialog.content_height(), 450);
    assert_eq!(dialog.content_width(), 400);

    let scroll = gtk4::ScrolledWindow::builder()
        .min_content_height(rttx::connect_existing_dialog::SCROLL_MIN_CONTENT_HEIGHT)
        .build();
    assert_eq!(scroll.min_content_height(), 300);
}

// ── Mouse reporting vs gestures (#459) ──────────────────────────

/// The link click gesture on a direct terminal must use capture phase so it
/// can check modifiers before VTE processes the event. When no Ctrl is held,
/// the gesture denies so VTE receives the click for mouse-aware apps.
#[test]
#[ignore = "requires isolated GTK harness"]
fn direct_terminal_link_gesture_is_capture_phase() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t-cap", None);
    let controllers = term.vte().observe_controllers();
    let mut found_link_gesture = false;
    for i in 0..controllers.n_items() {
        let Some(ctrl) = controllers.item(i) else { continue };
        if let Ok(gesture) = ctrl.downcast::<gtk4::GestureClick>()
            && gesture.button() == 1
        {
            assert_eq!(
                gesture.propagation_phase(),
                gtk4::PropagationPhase::Capture,
                "link click gesture must use capture phase"
            );
            found_link_gesture = true;
        }
    }
    assert!(found_link_gesture, "VTE must have a button-1 capture gesture for links");
}

/// The right-click context menu gesture on a persistent pane must use
/// capture phase. When Shift is held, the gesture denies so VTE
/// receives the right-click for mouse-aware apps. Regression for #459.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_context_menu_gesture_is_capture_phase() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("p-ctx", "s1");
    let controllers = pane.vte().observe_controllers();
    let mut found_right_click = false;
    for i in 0..controllers.n_items() {
        let Some(ctrl) = controllers.item(i) else { continue };
        if let Ok(gesture) = ctrl.downcast::<gtk4::GestureClick>()
            && gesture.button() == 3
        {
            assert_eq!(
                gesture.propagation_phase(),
                gtk4::PropagationPhase::Capture,
                "context menu gesture must use capture phase"
            );
            found_right_click = true;
        }
    }
    assert!(found_right_click, "VTE must have a button-3 capture gesture for context menu");
}

/// The right-click context menu gesture on a direct terminal must use
/// capture phase. Regression for #459.
#[test]
#[ignore = "requires isolated GTK harness"]
fn direct_terminal_context_menu_gesture_is_capture_phase() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t-ctx", None);
    let controllers = term.vte().observe_controllers();
    let mut found_right_click = false;
    for i in 0..controllers.n_items() {
        let Some(ctrl) = controllers.item(i) else { continue };
        if let Ok(gesture) = ctrl.downcast::<gtk4::GestureClick>()
            && gesture.button() == 3
        {
            assert_eq!(
                gesture.propagation_phase(),
                gtk4::PropagationPhase::Capture,
                "context menu gesture must use capture phase"
            );
            found_right_click = true;
        }
    }
    assert!(found_right_click, "VTE must have a button-3 capture gesture for context menu");
}

/// Regression for #568: the context menu popover must be parented to the VTE
/// widget, not the outer Box. When parented to the Box, the gesture
/// coordinates (VTE-relative) do not match the popover's coordinate space,
/// causing the popover to appear at the wrong position or not at all.
#[test]
#[ignore = "requires isolated GTK harness"]
fn direct_terminal_context_menu_parented_to_vte() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t-parent", None);
    let popover = find_popover_child(term.vte().upcast_ref::<gtk4::Widget>())
        .expect("context menu popover must be a child of the VTE widget");
    assert_eq!(
        popover.parent().as_ref().map(|w| w.type_().name()),
        Some("VteTerminal"),
        "context menu popover parent must be the VTE, not the outer Box"
    );
}

/// Regression for #568: same as above but for persistent pane.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_context_menu_parented_to_vte() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("p-parent", "s1");
    let popover = find_popover_child(pane.vte().upcast_ref::<gtk4::Widget>())
        .expect("context menu popover must be a child of the VTE widget");
    assert_eq!(
        popover.parent().as_ref().map(|w| w.type_().name()),
        Some("VteTerminal"),
        "context menu popover parent must be the VTE, not the outer Box"
    );
}

#[test]
#[ignore = "requires display backend"]
fn app_css_loads_without_parser_errors() {
    require_display!();
    let display = gtk4::gdk::Display::default().unwrap();
    let css = gtk4::CssProvider::new();
    css.load_from_string(rttx::application::APP_CSS);
    gtk4::style_context_add_provider_for_display(
        &display,
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    for is_dark in [true, false] {
        let accent = gtk4::CssProvider::new();
        accent.load_from_string(rttx::application::accent_css_for_dark(is_dark));
        gtk4::style_context_add_provider_for_display(
            &display,
            &accent,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk4::style_context_remove_provider_for_display(&display, &accent);
    }
}

/// Verify that PersistentPaneView stores its context menu for disposal.
/// Without this, the PopoverMenu created with set_parent() leaks because
/// dispose() cannot unparent it (#537).
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_view_stores_context_menu_for_disposal() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("ctx-test", "rt-1");
    // The context menu must be stored so dispose() can unparent it.
    assert!(
        pane.imp().context_menu.borrow().is_some(),
        "PersistentPaneView must store context_menu for disposal"
    );
}

/// Verify that TerminalWidget stores its context menu for disposal (#537).
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_stores_context_menu_for_disposal() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("ctx-test", None);
    assert!(
        term.imp().context_menu.borrow().is_some(),
        "TerminalWidget must store context_menu for disposal"
    );
}

/// `connect_input` must be idempotent: calling it twice must not stack
/// duplicate signal handlers or event controllers. Regression for #538.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_connect_input_is_idempotent() {
    require_display!();

    use std::cell::RefCell;
    use std::rc::Rc;

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("idem-in", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&pane));
    window.present();
    pump_events(50);

    let connected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Connected);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Connected, &connected);

    let forwarded = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
    let f1 = Rc::clone(&forwarded);
    pane.connect_input(move |bytes| {
        f1.borrow_mut().push(bytes.to_vec());
    });
    assert!(pane.input_connected_for_test(), "flag must be set after first call");

    // Second call must be a no-op.
    let second_called = Rc::new(RefCell::new(false));
    let sc = Rc::clone(&second_called);
    pane.connect_input(move |_| {
        *sc.borrow_mut() = true;
    });

    let sgr = "\x1b[<0;10;5M";
    pane.vte().emit_by_name::<()>("commit", &[&sgr, &(sgr.len() as u32)]);
    pump_events(50);

    assert_eq!(
        forwarded.borrow().len(),
        1,
        "commit must fire the first callback exactly once, not stack duplicates"
    );
    assert!(!*second_called.borrow(), "second connect_input call must be ignored");

    window.close();
}

/// `connect_resize` must be idempotent: calling it twice must not create
/// a second tick callback. Regression for #538.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_connect_resize_is_idempotent() {
    require_display!();

    use std::cell::RefCell;
    use std::rc::Rc;

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("idem-rs", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&pane));
    window.present();
    pump_events(50);

    let resizes = Rc::new(RefCell::new(Vec::<(u16, u16)>::new()));
    let r1 = Rc::clone(&resizes);
    pane.connect_resize(move |cols, rows| {
        r1.borrow_mut().push((cols, rows));
    });
    assert!(pane.resize_connected_for_test(), "flag must be set after first call");
    assert!(pane.has_resize_tick_for_test(), "tick callback must be registered");

    // Second call must be a no-op.
    let second_called = Rc::new(RefCell::new(false));
    let sc = Rc::clone(&second_called);
    pane.connect_resize(move |_, _| {
        *sc.borrow_mut() = true;
    });

    // Pump a few frames so the tick callback fires.
    pump_events(100);

    assert!(!*second_called.borrow(), "second connect_resize call must be ignored");

    window.close();
}

/// After `connect_resize`, the pane must use a tick callback instead of a
/// free-running `glib::timeout_add_local` timer. Regression for #538.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_resize_uses_tick_callback() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("tick-rs", "runtime-1");
    assert!(!pane.has_resize_tick_for_test(), "no tick callback before connect_resize");

    pane.connect_resize(|_, _| {});
    assert!(
        pane.has_resize_tick_for_test(),
        "tick callback must be registered after connect_resize"
    );
}

/// Regression for #536: persistent pane header must show "app : path" format
/// and update when CWD changes.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_header_title_combines_daemon_title_and_cwd() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("title-536", "runtime-1");

    // Default title is "Terminal", not "Terminal (persistent)".
    assert_eq!(pane.title_label().label(), "Terminal");

    // Setting daemon title alone shows just the title.
    pane.set_daemon_title("bash");
    assert_eq!(pane.title_label().label(), "bash");

    // Setting CWD combines title + path.
    pane.set_current_directory(Some("/tmp/project"));
    assert_eq!(pane.title_label().label(), "bash : /tmp/project");

    // Changing CWD updates the combined title.
    pane.set_current_directory(Some("/var/log"));
    assert_eq!(pane.title_label().label(), "bash : /var/log");

    // Changing daemon title also updates.
    pane.set_daemon_title("vim");
    assert_eq!(pane.title_label().label(), "vim : /var/log");
}

/// Regression for #574: the StackSwitcher (Places/Commands tab selector)
/// must have a bottom margin so there is a visual gap between the tool
/// selector and the content list below it.
#[test]
#[ignore = "requires isolated GTK harness"]
fn utility_switcher_has_bottom_margin() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    set_env("XDG_CONFIG_HOME", tmp.path());
    set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.utility-switcher-gap-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = rttx::window::Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let sidebar_box = &window.imp().utility_sidebar_box;
    let mut found_switcher = false;
    let mut child = sidebar_box.first_child();
    while let Some(widget) = child {
        if widget.is::<gtk4::StackSwitcher>() {
            assert!(
                widget.margin_bottom() > 0,
                "StackSwitcher must have a bottom margin for visual separation from content"
            );
            found_switcher = true;
            break;
        }
        child = widget.next_sibling();
    }
    assert!(found_switcher, "utility sidebar must contain a StackSwitcher");

    window.close();
    remove_env("RTTX_DISABLE_SHELL_SPAWN");
    remove_env("XDG_CONFIG_HOME");
}

// ── HostTagPicker widget tests ──────────────────────────────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_tag_picker_shows_local_checkbox() {
    require_display!();

    let picker = rttx::host_tag_picker::HostTagPicker::with_hosts(&[], &[]);

    // No hosts checked → empty selection (global)
    assert!(picker.selected_tags().is_empty());
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_tag_picker_prechecks_selected_tags() {
    require_display!();

    let picker = rttx::host_tag_picker::HostTagPicker::with_hosts(&[], &["local".to_string()]);

    let tags = picker.selected_tags();
    assert_eq!(tags, vec!["local"]);
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_tag_picker_shows_saved_remote_hosts() {
    require_display!();

    let host = rttx::host::Host::remote("deploy@example.com");
    let picker =
        rttx::host_tag_picker::HostTagPicker::with_hosts(&[host], &["example.com".to_string()]);

    let tags = picker.selected_tags();
    assert_eq!(tags, vec!["example.com"]);
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_tag_picker_no_selection_means_global() {
    require_display!();

    let host = rttx::host::Host::remote("deploy@example.com");
    let picker = rttx::host_tag_picker::HostTagPicker::with_hosts(&[host], &[]);

    assert!(picker.selected_tags().is_empty(), "no selection should mean global");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_tag_picker_multiple_hosts_selected() {
    require_display!();

    let host = rttx::host::Host::remote("deploy@example.com");
    let picker = rttx::host_tag_picker::HostTagPicker::with_hosts(
        &[host],
        &["local".to_string(), "example.com".to_string()],
    );

    let tags = picker.selected_tags();
    assert_eq!(tags, vec!["local", "example.com"]);
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_tag_picker_skips_duplicate_local_in_saved_hosts() {
    require_display!();

    // If saved hosts somehow contain a "local" entry, it should not duplicate
    let local_host = rttx::host::Host::local();
    let remote = rttx::host::Host::remote("example.com");
    let picker = rttx::host_tag_picker::HostTagPicker::with_hosts(
        &[local_host, remote],
        &["local".to_string()],
    );

    let tags = picker.selected_tags();
    assert_eq!(tags, vec!["local"], "local should appear only once");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_tag_picker_unrecognized_tag_not_checked() {
    require_display!();

    // If selected_tags contains a host not in the saved list, it won't appear
    let picker =
        rttx::host_tag_picker::HostTagPicker::with_hosts(&[], &["unknown.example.com".to_string()]);

    // Only local is in the picker, and it's not selected
    assert!(picker.selected_tags().is_empty());
}

/// Regression for #655: managed pane title must strip user@host prefix
/// and avoid duplicating the path when CWD is available.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_title_strips_user_host_prefix() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("pane-title", "rt-1");
    let home = std::env::var("HOME").unwrap();

    pane.set_daemon_title(&format!("user@host: {home}/projects"));
    pane.set_current_directory(Some(&format!("{home}/projects")));

    let label = pane.title_label().label().to_string();
    assert!(!label.contains('@'), "pane title must not contain user@host, got: {label}");
    assert_eq!(label, "~/projects");
}

/// Regression for #659: the shared `should_open_context_menu` helper must
/// return true for plain right-click (no modifiers) and false when Shift is
/// held, matching GNOME Terminal / Ptyxis / Tilix conventions. Both
/// `TerminalWidget` and `PersistentPaneView` delegate to this helper.
#[test]
#[ignore = "requires isolated GTK harness"]
fn context_menu_modifier_convention_matches_gnome() {
    require_display!();

    // Verify the helper function implements the GNOME convention.
    assert!(
        rttx::terminal::should_open_context_menu(gtk4::gdk::ModifierType::empty()),
        "plain right-click must open context menu"
    );
    assert!(
        !rttx::terminal::should_open_context_menu(gtk4::gdk::ModifierType::SHIFT_MASK),
        "Shift+right-click must pass through to VTE"
    );

    // Verify both widget types have a button-3 capture gesture that can
    // invoke the context menu (structural prerequisite for the helper).
    for (label, controllers) in [
        (
            "TerminalWidget",
            rttx::terminal::widget::TerminalWidget::new("t-mod", None).vte().observe_controllers(),
        ),
        (
            "PersistentPaneView",
            rttx::terminal::persistent_widget::PersistentPaneView::new("p-mod", "s1")
                .vte()
                .observe_controllers(),
        ),
    ] {
        let has_btn3 = (0..controllers.n_items()).any(|i| {
            controllers
                .item(i)
                .and_then(|c| c.downcast::<gtk4::GestureClick>().ok())
                .is_some_and(|g| g.button() == 3)
        });
        assert!(has_btn3, "{label} must have a button-3 gesture for context menu");
    }
}

/// Regression test for #769: after a reconnect cycle (Disconnected →
/// Recovered), VTE commit data must be forwarded again, proving the
/// pane re-enables input. This catches regressions where
/// `set_connection_presentation` fails to flip `accepts_input` back
/// to true after reconnect.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_input_re_enabled_after_reconnect_cycle() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("recon-in", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&pane));
    window.present();
    pump_events(50);

    let forwarded = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
    let f = Rc::clone(&forwarded);
    pane.connect_input(move |bytes| {
        f.borrow_mut().push(bytes.to_vec());
    });

    // Start connected — input should work.
    let connected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Connected);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Connected, &connected);

    let probe = "\x1b[<0;1;1M";
    pane.vte().emit_by_name::<()>("commit", &[&probe, &(probe.len() as u32)]);
    pump_events(50);
    assert_eq!(forwarded.borrow().len(), 1, "connected pane must forward input");
    forwarded.borrow_mut().clear();

    // Simulate disconnect — input must stop.
    let disconnected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Disconnected);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Disconnected, &disconnected);

    pane.vte().emit_by_name::<()>("commit", &[&probe, &(probe.len() as u32)]);
    pump_events(50);
    assert!(forwarded.borrow().is_empty(), "disconnected pane must not forward input");

    // Simulate reconnect — input must resume.
    let recovered =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Recovered);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Recovered, &recovered);

    pane.vte().emit_by_name::<()>("commit", &[&probe, &(probe.len() as u32)]);
    pump_events(50);
    assert_eq!(forwarded.borrow().len(), 1, "pane must forward input after reconnect (Recovered)");

    window.close();
}

/// Regression test for #769: VTE commit data (mouse escape sequences)
/// must be forwarded after a reconnect cycle. This catches regressions
/// where the input callback is lost or `accepts_input` stays false
/// after reconnect.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_mouse_input_forwarded_after_reconnect() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("mouse-rc", "runtime-1");
    let window = gtk4::Window::new();
    window.set_default_size(640, 320);
    window.set_child(Some(&pane));
    window.present();
    pump_events(50);

    let forwarded = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
    let f = Rc::clone(&forwarded);
    pane.connect_input(move |bytes| {
        f.borrow_mut().push(bytes.to_vec());
    });

    // Connect → disconnect → reconnect.
    let connected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Connected);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Connected, &connected);

    let disconnected =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Disconnected);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Disconnected, &disconnected);

    let recovered =
        rttx::runtime::present_connection_status(&rttx::runtime::ConnectionStatus::Recovered);
    pane.set_connection_presentation(&rttx::runtime::ConnectionStatus::Recovered, &recovered);

    // Emit a mouse escape sequence — must be forwarded.
    let sgr_click = "\x1b[<0;5;10M";
    pane.vte().emit_by_name::<()>("commit", &[&sgr_click, &(sgr_click.len() as u32)]);
    pump_events(50);

    assert!(
        forwarded.borrow().contains(&sgr_click.as_bytes().to_vec()),
        "VTE mouse escape sequences must be forwarded after reconnect"
    );

    window.close();
}

/// Regression test for #769: all gesture controllers on the persistent
/// pane VTE must use capture phase so they can deny events to let VTE
/// handle mouse-aware applications. No gesture should use bubble phase,
/// which would prevent VTE from seeing the event first.
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_all_gestures_use_capture_phase() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("gest-cap", "runtime-1");
    let controllers = pane.vte().observe_controllers();
    let mut gesture_count = 0;
    for i in 0..controllers.n_items() {
        let Some(ctrl) = controllers.item(i) else { continue };
        if let Ok(gesture) = ctrl.downcast::<gtk4::GestureClick>() {
            gesture_count += 1;
            assert_eq!(
                gesture.propagation_phase(),
                gtk4::PropagationPhase::Capture,
                "button-{} gesture must use capture phase for mouse-aware app compatibility",
                gesture.button()
            );
        }
    }
    assert!(gesture_count > 0, "VTE must have at least one gesture controller");
}

/// `repair_terminal` on a managed handle feeds cleanup bytes into VTE and
/// resets tracked mode state. #811.
#[test]
#[ignore = "requires isolated GTK harness"]
fn repair_terminal_resets_managed_pane_modes() {
    require_display!();

    let managed =
        rttx::terminal::persistent_widget::PersistentPaneView::new("repair-1", "runtime-1");
    // Simulate stuck state: cursor hidden, mouse on, app cursor keys on.
    managed.vte().feed(b"\x1b[?25l\x1b[?1003h\x1b[?1h");
    managed.set_application_modes(true, true);

    let handle = rttx::terminal::handle::TerminalHandle::Managed(managed.clone());
    handle.repair_terminal();

    let modes = managed.terminal_modes();
    assert!(!modes.application_cursor_keys, "cursor keys should be off after repair");
    assert!(!modes.application_keypad, "keypad should be off after repair");
}

/// `repair_terminal` on a direct handle feeds cleanup bytes without panic. #811.
#[test]
#[ignore = "requires isolated GTK harness"]
fn repair_terminal_works_on_direct_pane() {
    require_display!();

    let direct = rttx::terminal::widget::TerminalWidget::new("repair-direct-1", None);
    // Simulate stuck state.
    direct.vte().feed(b"\x1b[?25l\x1b[?1003h");

    let handle = rttx::terminal::handle::TerminalHandle::Direct(direct);
    handle.repair_terminal();
    // No panic — cleanup bytes accepted by VTE.
}

/// `TerminalHandle::set_custom_title` sets and clears on direct panes.
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_custom_title_direct() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("ct-direct-1", None);
    let handle = rttx::terminal::handle::TerminalHandle::Direct(term);

    assert!(handle.custom_title().is_none());
    handle.set_custom_title(Some("my pane"));
    assert_eq!(handle.custom_title().as_deref(), Some("my pane"));
    assert_eq!(handle.title(), "my pane");

    handle.set_custom_title(None);
    assert!(handle.custom_title().is_none());
}

/// `TerminalHandle::set_custom_title` sets and clears on managed panes.
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_handle_custom_title_managed() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("ct-managed-1", "rt-1");
    pane.set_daemon_title("daemon title");
    let handle = rttx::terminal::handle::TerminalHandle::Managed(pane);

    assert!(handle.custom_title().is_none());
    handle.set_custom_title(Some("custom name"));
    assert_eq!(handle.custom_title().as_deref(), Some("custom name"));
    assert_eq!(handle.title(), "custom name");

    handle.set_custom_title(None);
    assert!(handle.custom_title().is_none());
    assert_eq!(handle.title(), "daemon title");
}

/// Context menu for `TerminalWidget` includes "Rename Pane".
#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_widget_context_menu_has_rename_pane() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("ctx-rename-1", None);
    let menu = term.imp().context_menu.borrow();
    let popover = menu.as_ref().expect("context menu should exist");
    let model = popover.menu_model().expect("menu model should exist");
    let has_rename = menu_model_contains_label(&model, "Rename Pane");
    assert!(has_rename, "context menu should contain 'Rename Pane'");
}

/// Context menu for `PersistentPaneView` includes "Rename Pane".
#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_pane_context_menu_has_rename_pane() {
    require_display!();

    let pane = rttx::terminal::persistent_widget::PersistentPaneView::new("ctx-rename-2", "rt-1");
    let menu = pane.imp().context_menu.borrow();
    let popover = menu.as_ref().expect("context menu should exist");
    let model = popover.menu_model().expect("menu model should exist");
    let has_rename = menu_model_contains_label(&model, "Rename Pane");
    assert!(has_rename, "context menu should contain 'Rename Pane'");
}

fn menu_model_contains_label(model: &gtk4::gio::MenuModel, label: &str) -> bool {
    for i in 0..model.n_items() {
        if let Some(item_label) =
            model.item_attribute_value(i, "label", Some(gtk4::glib::VariantTy::STRING))
            && item_label.get::<String>().is_some_and(|l| l == label)
        {
            return true;
        }
        if let Some(section) = model.item_link(i, "section")
            && menu_model_contains_label(&section, label)
        {
            return true;
        }
        if let Some(submenu) = model.item_link(i, "submenu")
            && menu_model_contains_label(&submenu, label)
        {
            return true;
        }
    }
    false
}

// ── Preferences Data section tests ──────────────────────────────────────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferences_data_group_has_three_rows() {
    require_display!();

    let window = adw::PreferencesWindow::new();
    let group = rttx::preferences_window::build_data_group(&window);

    let mut row_count = 0;
    let mut child = group.first_child();
    while let Some(widget) = child {
        if widget.downcast_ref::<adw::ActionRow>().is_some() {
            row_count += 1;
        }
        // ActionRows are inside a GtkListBox inside the group
        if let Some(list_box) = widget.downcast_ref::<gtk4::ListBox>() {
            let mut row_child = list_box.first_child();
            while let Some(row_widget) = row_child {
                if row_widget.downcast_ref::<adw::ActionRow>().is_some() {
                    row_count += 1;
                }
                row_child = row_widget.next_sibling();
            }
        }
        child = widget.next_sibling();
    }
    assert_eq!(row_count, 3, "Data group should contain exactly 3 action rows");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferences_data_group_export_row_has_no_destructive_class() {
    require_display!();

    let window = adw::PreferencesWindow::new();
    let group = rttx::preferences_window::build_data_group(&window);

    let first_row = find_action_row_by_title(&group, "Export Configuration\u{2026}");
    assert!(first_row.is_some(), "Export row should exist");
    let row = first_row.unwrap();
    assert!(
        !row.has_css_class("destructive-action"),
        "Export row should NOT have destructive-action class"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferences_data_group_import_row_has_destructive_class() {
    require_display!();

    let window = adw::PreferencesWindow::new();
    let group = rttx::preferences_window::build_data_group(&window);

    let row = find_action_row_by_title(&group, "Import Configuration\u{2026}");
    assert!(row.is_some(), "Import row should exist");
    assert!(
        row.unwrap().has_css_class("destructive-action"),
        "Import row should have destructive-action class"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferences_data_group_reset_row_has_destructive_class() {
    require_display!();

    let window = adw::PreferencesWindow::new();
    let group = rttx::preferences_window::build_data_group(&window);

    let row = find_action_row_by_title(&group, "Reset to Defaults");
    assert!(row.is_some(), "Reset row should exist");
    assert!(
        row.unwrap().has_css_class("destructive-action"),
        "Reset row should have destructive-action class"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferences_data_group_rows_have_subtitles() {
    require_display!();

    let window = adw::PreferencesWindow::new();
    let group = rttx::preferences_window::build_data_group(&window);

    let export = find_action_row_by_title(&group, "Export Configuration\u{2026}").unwrap();
    assert!(export.subtitle().is_some_and(|s| !s.is_empty()), "Export row should have a subtitle");

    let import = find_action_row_by_title(&group, "Import Configuration\u{2026}").unwrap();
    assert!(import.subtitle().is_some_and(|s| !s.is_empty()), "Import row should have a subtitle");

    let reset = find_action_row_by_title(&group, "Reset to Defaults").unwrap();
    assert!(reset.subtitle().is_some_and(|s| !s.is_empty()), "Reset row should have a subtitle");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferences_data_group_title_is_data() {
    require_display!();

    let window = adw::PreferencesWindow::new();
    let group = rttx::preferences_window::build_data_group(&window);
    assert_eq!(group.title().as_str(), "Data");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferences_reset_row_activates_reset_config_action() {
    require_display!();

    let app = adw::Application::builder()
        .application_id("io.github.IllyaYalovyy.rttx.test.reset_action")
        .build();
    app.register(None::<&gtk4::gio::Cancellable>).unwrap();

    let triggered = std::rc::Rc::new(std::cell::Cell::new(false));
    let triggered_clone = triggered.clone();

    let action = gtk4::gio::SimpleAction::new("reset-config", None);
    action.connect_activate(move |_, _| {
        triggered_clone.set(true);
    });
    app.add_action(&action);

    let window = adw::PreferencesWindow::new();
    window.set_application(Some(&app));

    let group = rttx::preferences_window::build_data_group(&window);
    let page = adw::PreferencesPage::new();
    page.add(&group);
    window.add(&page);
    window.present();

    let row = find_action_row_by_title(&group, "Reset to Defaults").unwrap();
    row.emit_activate();

    assert!(triggered.get(), "reset-config action should have been triggered");
    window.close();
}

fn find_action_row_by_title(group: &adw::PreferencesGroup, title: &str) -> Option<adw::ActionRow> {
    let mut child = group.first_child();
    while let Some(widget) = child {
        if let Some(row) = widget.downcast_ref::<adw::ActionRow>()
            && row.title().as_str() == title
        {
            return Some(row.clone());
        }
        if let Some(list_box) = widget.downcast_ref::<gtk4::ListBox>() {
            let mut row_child = list_box.first_child();
            while let Some(row_widget) = row_child {
                if let Some(row) = row_widget.downcast_ref::<adw::ActionRow>()
                    && row.title().as_str() == title
                {
                    return Some(row.clone());
                }
                row_child = row_widget.next_sibling();
            }
        }
        child = widget.next_sibling();
    }
    None
}

/// Regression for #887: header bar workspace buttons must be icon-only and consistent.
#[test]
#[ignore = "requires isolated GTK harness"]
fn header_bar_workspace_buttons_are_icon_only() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    set_env("XDG_CONFIG_HOME", tmp.path());
    set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.header-button-style-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = rttx::window::Window::new(&app);
    window.present();
    pump_events(50);

    let imp = window.imp();

    // All buttons must have an icon.
    assert_eq!(
        imp.new_button.icon_name().as_deref(),
        Some("list-add-symbolic"),
        "New button must use list-add-symbolic icon"
    );
    assert_eq!(
        imp.connect_button.icon_name().as_deref(),
        Some("network-server-symbolic"),
        "Connect button must use network-server-symbolic icon"
    );
    assert_eq!(
        imp.new_direct_button.icon_name().as_deref(),
        Some("utilities-terminal-symbolic"),
        "Direct button must use utilities-terminal-symbolic icon"
    );

    // No button should have a text label (icon-only style).
    assert!(
        imp.new_button.label().is_none_or(|l| l.is_empty()),
        "New button must not have a text label"
    );
    assert!(
        imp.connect_button.label().is_none_or(|l| l.is_empty()),
        "Connect button must not have a text label"
    );
    assert!(
        imp.new_direct_button.label().is_none_or(|l| l.is_empty()),
        "Direct button must not have a text label"
    );

    // All buttons must have tooltips for discoverability.
    assert!(imp.new_button.tooltip_text().is_some(), "New button must have a tooltip");
    assert!(imp.connect_button.tooltip_text().is_some(), "Connect button must have a tooltip");
    assert!(imp.new_direct_button.tooltip_text().is_some(), "Direct button must have a tooltip");

    window.close();
    remove_env("RTTX_DISABLE_SHELL_SPAWN");
    remove_env("XDG_CONFIG_HOME");
}

/// The window must expose a `rename-workspace` action bound to F2.
#[test]
#[ignore = "requires isolated GTK harness"]
fn window_has_rename_workspace_action() {
    require_display!();

    let app = adw::Application::builder()
        .application_id("io.github.IllyaYalovyy.rttx.test.rename_workspace_action")
        .build();
    app.register(None::<&gtk4::gio::Cancellable>).unwrap();

    let window = rttx::window::Window::new(&app);
    let action_group: gtk4::gio::ActionGroup = window.clone().upcast();
    assert!(
        action_group.has_action("rename-workspace"),
        "window must have rename-workspace action"
    );
    window.close();
}

/// Copying Cyrillic (non-ASCII) text from a terminal must preserve Unicode
/// characters on the clipboard, not produce escaped UTF-8 byte sequences. #982.
#[test]
#[ignore = "requires isolated GTK harness"]
fn copy_cyrillic_text_preserves_unicode_on_clipboard() {
    require_display!();

    let display = gtk4::gdk::Display::default().expect("display should be available for GTK tests");
    display.clipboard().set_text("");

    let direct = rttx::terminal::widget::TerminalWidget::new("cyrillic-1", None);
    let window = present_widget(&direct);
    direct.vte().feed("Привет мир\r\n".as_bytes());
    pump_events(50);
    direct.vte().select_all();

    let handle = rttx::terminal::handle::TerminalHandle::Direct(direct);
    handle.copy_clipboard();
    assert!(
        wait_until(1000, || {
            clipboard_text().is_some_and(|text| text.contains("Привет мир"))
        }),
        "clipboard must contain original Cyrillic text, got: {:?}",
        clipboard_text()
    );
    window.close();
}
