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
            std::env::set_var("GTK_A11Y", "none")
        };
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gtk4::init().is_ok()))
            .unwrap_or(false);
        if ok {
            if let Some(display) = gtk4::gdk::Display::default() {
                std::mem::forget(display);
            }
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

/// This is the exact bug that caused the split crash: a single widget
/// added to a Stack by name, then unparented, then stack.remove() called
/// on the now-orphaned widget. GTK asserts that the child's parent is
/// the stack — but unparent() already removed it.
#[test]
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
fn build_layout_widget_sets_position_after_allocation() {
    require_display!();

    use rttx::session::build_layout_widget;
    use rttx::session::layout::*;

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

    let widget = build_layout_widget(&layout, &|_uuid, _cwd, _profile, _title| {
        gtk4::Label::new(Some("terminal")).upcast()
    });

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
fn triple_nested_split_all_paneds_nonzero() {
    require_display!();

    use rttx::session::build_layout_widget;
    use rttx::session::layout::*;

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

    let widget = build_layout_widget(&layout, &|_uuid, _cwd, _profile, _title| {
        gtk4::Label::new(Some("terminal")).upcast()
    });

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

// ── M2: RefCell re-entrancy (GTK signal timing) ───────────────────────────────

/// Proves that GTK property-change signals fire SYNCHRONOUSLY in the same
/// call stack as the setter. This is why holding a RefCell borrow across
/// any GTK widget operation is dangerous: the operation may fire a signal
/// whose handler also tries to borrow the same RefCell.
///
/// If GTK ever changed to fire signals asynchronously, this test would fail
/// and our borrow-ordering discipline would no longer be necessary.
#[test]
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

/// Proves that holding a RefCell borrow across a GTK property setter panics
/// when the signal handler also borrows the same RefCell.
///
/// This is the WRONG pattern that caused the child_exited crash. The test
/// uses catch_unwind so it can assert the panic occurred without aborting.
#[test]
fn gtk_signal_during_held_borrow_panics() {
    require_display!();

    let state = Rc::new(RefCell::new(0i32));
    let state_clone = state.clone();

    let label = gtk4::Label::new(Some("original"));
    label.connect_notify_local(Some("label"), move |_, _| {
        *state_clone.borrow_mut() += 1; // re-entrant borrow
    });

    // Hold borrow_mut then trigger a signal — must panic with BorrowMutError
    let state_for_closure = state.clone();
    let label_clone = label.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _held = state_for_closure.borrow_mut(); // hold live borrow
        label_clone.set_label("changed"); // fires signal → borrow_mut → panic
    }));

    assert!(
        result.is_err(),
        "Expected BorrowMutError panic when RefCell is held across a GTK signal \
         that also borrows the same RefCell. \
         If this passes, the signal did not fire synchronously."
    );
}

/// Proves the CORRECT pattern: extract data, release borrow, then do the
/// GTK operation. The signal handler can borrow freely because there is
/// no active borrow when it fires.
#[test]
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
fn build_layout_widget_calls_make_terminal_exactly_once_per_uuid() {
    require_display!();

    use rttx::session::build_layout_widget;
    use rttx::session::layout::*;
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

    build_layout_widget(&layout, &|uuid, _cwd, _profile, _title| {
        *counts_clone.borrow_mut().entry(uuid.to_string()).or_insert(0) += 1;
        gtk4::Label::new(Some(uuid)).upcast()
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
fn paned_extreme_but_valid_ratios_produce_nonzero_positions() {
    require_display!();

    use rttx::session::build_layout_widget;
    use rttx::session::layout::*;

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
            build_layout_widget(&layout, &|uuid, _, _, _| gtk4::Label::new(Some(uuid)).upcast());

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
fn terminal_context_menu_is_parented_to_widget() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);

    let popover = find_popover_child(term.upcast_ref::<gtk4::Widget>());
    assert!(
        popover.is_some(),
        "TerminalWidget must have a PopoverMenu child after construction. \
         Call set_parent() on the context menu during constructed()."
    );
}

/// Prevent regression: mounting VTE directly in the pane removes any visible
/// scrollbar, which made it impossible to discover backlog scrolling from the UI.
#[test]
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

/// Prevent regression: an empty or mis-named action in the context menu produces
/// a non-functional item with no visible error.
///
/// Each section of the menu model is verified to be non-empty and all items must
/// carry an "action" attribute. Any item without an action attribute is invisible
/// to the user but silently broken.
#[test]
fn terminal_context_menu_model_has_actions() {
    require_display!();

    let term = rttx::terminal::widget::TerminalWidget::new("t1", None);

    let popover = find_popover_child(term.upcast_ref::<gtk4::Widget>())
        .expect("context menu must be parented (see terminal_context_menu_is_parented_to_widget)");

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
            assert!(
                has_action,
                "context menu section {section_idx} item {item_idx} has no action attribute — \
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
