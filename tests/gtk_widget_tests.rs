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

use gtk4::prelude::*;
use std::sync::Once;

static GTK_INIT: Once = Once::new();

fn ensure_gtk_init() -> bool {
    // Try to initialize GTK. If no display is available, skip.
    let mut success = false;
    GTK_INIT.call_once(|| {
        // Suppress accessibility warnings in test environment
        std::env::set_var("GTK_A11Y", "none");
        success = gtk4::init().is_ok();
    });
    // After call_once, we can't re-check, so try a widget allocation
    // as a smoke test.
    if !success {
        success = std::panic::catch_unwind(|| {
            let _ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        })
        .is_ok();
    }
    success
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

    use rttx::session::layout::*;
    use rttx::session::build_layout_widget;

    // (t1 / t2) | t3 — nested split
    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::Split {
            orientation: SplitOrientation::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Terminal {
                uuid: "t1".into(), profile: None, cwd: None, custom_title: None,
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: "t2".into(), profile: None, cwd: None, custom_title: None,
            }),
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t3".into(), profile: None, cwd: None, custom_title: None,
        }),
    };

    let widget = build_layout_widget(&layout, &|_uuid, _cwd, _profile, _title| {
        gtk4::Label::new(Some("terminal")).upcast()
    });

    let outer = widget.downcast_ref::<gtk4::Paned>()
        .expect("Root must be Paned");

    // Before allocation, position is 0 — expected
    assert_eq!(outer.position(), 0);

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

    use rttx::session::layout::*;
    use rttx::session::build_layout_widget;

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
                    uuid: "t1".into(), profile: None, cwd: None, custom_title: None,
                }),
                second: Box::new(LayoutNode::Terminal {
                    uuid: "t2".into(), profile: None, cwd: None, custom_title: None,
                }),
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: "t3".into(), profile: None, cwd: None, custom_title: None,
            }),
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t4".into(), profile: None, cwd: None, custom_title: None,
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
