use gtk4::glib;
use gtk4::prelude::*;
use rttx::terminal::widget::TerminalWidget;
use rttx::workspace::{LayoutNode, SplitOrientation, build_layout_widget};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Once;

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

fn is_descendant_of(widget: &gtk4::Widget, root: &gtk4::Widget) -> bool {
    let mut current = Some(widget.clone());
    while let Some(node) = current {
        if node == *root {
            return true;
        }
        current = node.parent();
    }
    false
}

fn widget_chain(widget: &gtk4::Widget) -> String {
    let mut parts = Vec::new();
    let mut current = Some(widget.clone());
    while let Some(node) = current {
        parts.push(node.type_().name().to_string());
        current = node.parent();
    }
    parts.join(" <- ")
}

fn widget_ptr(widget: &gtk4::Widget) -> usize {
    widget.as_ptr() as usize
}

fn collect_leaf_widgets(widget: &gtk4::Widget, leaves: &mut Vec<gtk4::Widget>) {
    if let Some(paned) = widget.downcast_ref::<gtk4::Paned>() {
        let start = paned.start_child().expect("Paned in rebuilt tree must have a start child");
        let end = paned.end_child().expect("Paned in rebuilt tree must have an end child");
        collect_leaf_widgets(&start, leaves);
        collect_leaf_widgets(&end, leaves);
        return;
    }

    leaves.push(widget.clone());
}

fn detach_from_detached_tree(widget: &gtk4::Widget) {
    if let Some(paned) = widget.downcast_ref::<gtk4::Paned>() {
        if let Some(start) = paned.start_child() {
            detach_from_detached_tree(&start);
            paned.set_start_child(None::<&gtk4::Widget>);
        }
        if let Some(end) = paned.end_child() {
            detach_from_detached_tree(&end);
            paned.set_end_child(None::<&gtk4::Widget>);
        }
    }
}

fn leaf_descriptions(leaves: &[gtk4::Widget]) -> Vec<String> {
    leaves
        .iter()
        .map(|leaf| {
            leaf.clone().downcast::<TerminalWidget>().map_or_else(
                |_| leaf.type_().name().to_string(),
                |term| format!("{}({})", leaf.type_().name(), term.title_label().label()),
            )
        })
        .collect()
}

fn assert_live_tree_matches_layout(
    layout: &LayoutNode,
    root: &gtk4::Widget,
    container: &gtk4::Box,
    terminals: &Rc<RefCell<HashMap<String, TerminalWidget>>>,
    rebuild_index: usize,
) {
    assert_eq!(
        root.parent(),
        Some(container.clone().upcast::<gtk4::Widget>()),
        "Rebuild {rebuild_index}: current root must be attached to the live container",
    );

    let mut leaves = Vec::new();
    collect_leaf_widgets(root, &mut leaves);

    assert_eq!(
        leaves.len(),
        layout.terminal_count(),
        "Rebuild {rebuild_index}: live tree leaf count must match layout terminal count",
    );

    let terminals = terminals.borrow();
    for uuid in layout.terminal_uuids() {
        let term = terminals
            .get(&uuid)
            .unwrap_or_else(|| {
                panic!("Rebuild {rebuild_index}: missing terminal map entry for {uuid}")
            })
            .clone();
        let term_widget = term.clone().upcast::<gtk4::Widget>();

        let scroller = term
            .vte()
            .parent()
            .and_then(|parent| parent.downcast::<gtk4::ScrolledWindow>().ok())
            .unwrap_or_else(|| {
                panic!(
                    "Rebuild {rebuild_index}: terminal {uuid} lost the ScrolledWindow wrapper around its VTE"
                )
            });
        assert_eq!(
            scroller.parent(),
            Some(term_widget.clone()),
            "Rebuild {rebuild_index}: terminal {uuid} lost its ScrolledWindow child",
        );
        assert_eq!(
            scroller.child(),
            Some(term.vte().clone().upcast::<gtk4::Widget>()),
            "Rebuild {rebuild_index}: terminal {uuid} lost its VTE child inside the ScrolledWindow",
        );
        assert!(
            is_descendant_of(&term_widget, root),
            "Rebuild {rebuild_index}: terminal {uuid} is not attached to the current live tree\n\
             terminal ptr: 0x{:x}\n\
             terminal chain: {}\n\
             root type: {}\n\
             leaf types: {:?}\n\
             leaf ptrs: {:?}",
            widget_ptr(&term_widget),
            widget_chain(&term_widget),
            root.type_().name(),
            leaf_descriptions(&leaves),
            leaves.iter().map(|leaf| format!("0x{:x}", widget_ptr(leaf))).collect::<Vec<_>>(),
        );
        assert!(
            leaves.contains(&term_widget),
            "Rebuild {rebuild_index}: terminal {uuid} is not reachable as a leaf from the current root",
        );
    }
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn test_terminal_rebuild_integrity_simple() {
    require_display!();

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let terminals: Rc<RefCell<HashMap<String, TerminalWidget>>> =
        Rc::new(RefCell::new(HashMap::new()));

    let build_session = |layout: &LayoutNode| {
        build_layout_widget(layout, &|spec| {
            let mut map = terminals.borrow_mut();
            if let Some(existing) = map.get(spec.uuid) {
                let existing = existing.clone();
                drop(map);
                if existing.parent().is_some() {
                    existing.unparent();
                }
                return existing.upcast();
            }

            let term: TerminalWidget = glib::Object::builder().build();
            term.set_title(spec.uuid);
            map.insert(spec.uuid.to_string(), term.clone());
            term.upcast()
        })
    };

    let t1_uuid = "t1".to_string();
    let mut current_layout = LayoutNode::Terminal {
        uuid: t1_uuid.clone(),
        profile: None,
        cwd: None,
        custom_title: None,
    };

    let mut current_root = build_session(&current_layout);
    container.append(&current_root);
    assert_live_tree_matches_layout(&current_layout, &current_root, &container, &terminals, 1);

    for split_index in 2..=20 {
        current_layout = current_layout
            .split_terminal(&t1_uuid, SplitOrientation::Vertical)
            .expect("split must succeed");

        if current_root.parent().is_some() {
            container.remove(&current_root);
        }
        detach_from_detached_tree(&current_root);

        current_root = build_session(&current_layout);
        container.append(&current_root);

        assert_live_tree_matches_layout(
            &current_layout,
            &current_root,
            &container,
            &terminals,
            split_index,
        );
    }
}
