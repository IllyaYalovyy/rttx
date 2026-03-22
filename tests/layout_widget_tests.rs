use gtk4::prelude::*;
use rttx::session::{self, *};
use std::sync::Once;
use std::time::{Duration, Instant};

static GTK_INIT: Once = Once::new();

fn ensure_gtk_init() -> bool {
    let mut success = false;
    GTK_INIT.call_once(|| {
        // SAFETY: GTK init runs once before any threads spawn; no concurrent env readers.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("GTK_A11Y", "none")
        };
        success = gtk4::init().is_ok();
    });
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

fn wait_until(max_ms: u64, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    while Instant::now() < deadline {
        pump_events(20);
        if condition() {
            return true;
        }
    }
    condition()
}

#[test]
fn test_build_layout_widget_multiple_splits() {
    require_display!();

    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Vertical,
        ratio: 0.5,
        first: Box::new(LayoutNode::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Terminal {
                uuid: "t1".to_string(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: "t2".to_string(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t3".to_string(),
            profile: None,
            cwd: None,
            custom_title: None,
        }),
    };

    let created_uuids = std::cell::RefCell::new(Vec::new());
    let _widget = build_layout_widget(&layout, &|uuid, _, _, _| {
        created_uuids.borrow_mut().push(uuid.to_string());
        gtk4::Label::new(Some(uuid)).upcast()
    });

    let created_uuids = created_uuids.borrow();
    assert_eq!(created_uuids.len(), 3);
    assert!(created_uuids.contains(&"t1".to_string()));
    assert!(created_uuids.contains(&"t2".to_string()));
    assert!(created_uuids.contains(&"t3".to_string()));
}

#[test]
fn test_rebuild_session_content_reuses_terminals() {
    require_display!();

    let layout1 = LayoutNode::Terminal {
        uuid: "t1".to_string(),
        profile: None,
        cwd: None,
        custom_title: None,
    };

    let terminals =
        std::cell::RefCell::new(std::collections::HashMap::<String, gtk4::Widget>::new());

    let build_widget = |layout: &LayoutNode| {
        build_layout_widget(layout, &|uuid, _, _, _| {
            let mut terms = terminals.borrow_mut();
            if let Some(existing) = terms.get(uuid) {
                return existing.clone();
            }
            let new_term: gtk4::Widget = gtk4::Label::new(Some(uuid)).upcast();
            terms.insert(uuid.to_string(), new_term.clone());
            new_term
        })
    };

    let _widget1 = build_widget(&layout1);

    let layout2 = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(layout1.clone()),
        second: Box::new(LayoutNode::Terminal {
            uuid: "t2".to_string(),
            profile: None,
            cwd: None,
            custom_title: None,
        }),
    };

    if let Some(t1) = terminals.borrow().get("t1") {
        if t1.parent().is_some() {
            t1.unparent();
        }
    }

    let widget2 = build_widget(&layout2);
    let root_paned = widget2.downcast_ref::<gtk4::Paned>().expect("Root should be a Paned");
    let t1_widget = root_paned.start_child().expect("Should have t1");
    assert_eq!(t1_widget.downcast_ref::<gtk4::Label>().unwrap().label(), "t1");
}

#[test]
fn test_build_layout_widget_with_parented_terminals() {
    require_display!();

    let t1 = gtk4::Label::new(Some("t1")).upcast::<gtk4::Widget>();
    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    container.append(&t1);
    assert!(t1.parent().is_some());

    let layout = LayoutNode::Terminal {
        uuid: "t1".to_string(),
        profile: None,
        cwd: None,
        custom_title: None,
    };

    let widget = build_layout_widget(&layout, &|_, _, _, _| {
        if t1.parent().is_some() {
            t1.unparent();
        }
        t1.clone()
    });

    assert_eq!(widget, t1);
    assert!(t1.parent().is_none());
}

/// capture_paned_ratios must read the live Paned divider position back into
/// the layout's ratio field.  This is the invariant that makes split positions
/// persist across restarts: capture_state calls capture_paned_ratios before
/// serialising, so user-dragged positions are saved as ratios.
#[test]
fn test_capture_paned_ratios_reads_position() {
    require_display!();

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

    let widget =
        build_layout_widget(&layout, &|uuid, _, _, _| gtk4::Label::new(Some(uuid)).upcast());

    let paned = widget.downcast_ref::<gtk4::Paned>().unwrap();
    paned.set_size_request(800, 600);
    paned.allocate(800, 600, -1, None);

    // Simulate the user dragging the handle to 30% from the left.
    paned.set_position(240); // 240/800 = 0.3

    let mut updated = layout.clone();
    session::capture_paned_ratios(&mut updated, &widget);

    let LayoutNode::Split { ratio, .. } = updated else {
        panic!("Expected Split layout node");
    };
    assert!((ratio - 0.3).abs() < 0.02, "ratio should be ≈0.3 after capture, got {ratio}");
}

/// capture_paned_ratios must recurse into nested splits.
#[test]
fn test_capture_paned_ratios_nested() {
    require_display!();

    let layout = LayoutNode::Split {
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
    };

    let widget =
        build_layout_widget(&layout, &|uuid, _, _, _| gtk4::Label::new(Some(uuid)).upcast());

    let outer = widget.downcast_ref::<gtk4::Paned>().unwrap();
    outer.set_size_request(800, 600);
    outer.allocate(800, 600, -1, None);
    outer.set_position(400); // 0.5

    let inner = outer.start_child().unwrap().downcast::<gtk4::Paned>().unwrap();
    // Force inner to a known position on a known total.
    inner.set_size_request(400, 600);
    inner.allocate(400, 600, -1, None);
    inner.set_position(100); // 100/400 = 0.25

    let mut updated = layout.clone();
    session::capture_paned_ratios(&mut updated, &widget);

    let LayoutNode::Split { ratio: outer_ratio, first, .. } = updated else {
        panic!("Expected outer Split");
    };
    let LayoutNode::Split { ratio: inner_ratio, .. } = *first else {
        panic!("Expected inner Split");
    };

    assert!((outer_ratio - 0.5).abs() < 0.02, "outer ratio should be ≈0.5, got {outer_ratio}");
    assert!((inner_ratio - 0.25).abs() < 0.02, "inner ratio should be ≈0.25, got {inner_ratio}");
}

/// apply_paned_ratios must set Paned positions from layout ratios and
/// current allocated sizes.  This is the inverse of capture_paned_ratios
/// and is called (via idle) after adding the widget tree to the window so
/// splits are visually equal on first display.
#[test]
fn test_apply_paned_ratios_sets_position() {
    require_display!();

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

    let widget =
        build_layout_widget(&layout, &|uuid, _, _, _| gtk4::Label::new(Some(uuid)).upcast());

    let paned = widget.downcast_ref::<gtk4::Paned>().unwrap();
    paned.set_size_request(800, 600);
    paned.allocate(800, 600, -1, None);

    // Now apply_paned_ratios should read width=800 and set position=400.
    session::apply_paned_ratios(&layout, &widget);

    let pos = paned.position();
    assert!(
        (pos - 400).abs() <= 5,
        "position should be ≈400 (0.5 × 800) after apply_paned_ratios, got {pos}"
    );
}

#[test]
fn test_scheduled_initial_paned_ratios_apply_once_widget_has_size() {
    require_display!();

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

    let widget =
        build_layout_widget(&layout, &|uuid, _, _, _| gtk4::Label::new(Some(uuid)).upcast());
    session::schedule_initial_paned_ratios(&widget, &layout);

    let window = gtk4::Window::new();
    window.set_default_size(800, 600);
    window.set_child(Some(&widget));
    window.present();

    let settled = wait_until(1000, || {
        let paned = widget.downcast_ref::<gtk4::Paned>().unwrap();
        paned.width() > 0 && (paned.position() - (paned.width() / 2)).abs() <= 5
    });

    let paned = widget.downcast_ref::<gtk4::Paned>().unwrap();
    assert!(
        settled,
        "scheduled ratio application should settle the initial split near 50/50, got position={} width={}",
        paned.position(),
        paned.width()
    );

    window.close();
}

#[test]
fn test_scheduled_initial_paned_ratios_do_not_clobber_user_resized_ratio() {
    require_display!();

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

    let widget =
        build_layout_widget(&layout, &|uuid, _, _, _| gtk4::Label::new(Some(uuid)).upcast());
    session::schedule_initial_paned_ratios(&widget, &layout);

    let window = gtk4::Window::new();
    window.set_default_size(800, 600);
    window.set_child(Some(&widget));
    window.present();

    let settled = wait_until(1000, || widget.downcast_ref::<gtk4::Paned>().unwrap().width() > 0);
    assert!(settled, "test widget should receive an allocation before resize assertions");

    let paned = widget.downcast_ref::<gtk4::Paned>().unwrap();
    paned.set_position(240);

    paned.allocate(801, 600, -1, None);
    pump_events(50);

    let mut updated = layout.clone();
    session::capture_paned_ratios(&mut updated, &widget);

    let LayoutNode::Split { ratio, .. } = updated else {
        panic!("Expected Split layout node");
    };
    assert!(
        (ratio - 0.3).abs() < 0.03,
        "later allocations must not reset a user-resized split back to its original ratio, got {ratio}"
    );

    window.close();
}
