use gtk4::prelude::*;
use rttx::session::*;
use std::sync::OnceLock;

static GTK_INIT: OnceLock<bool> = OnceLock::new();

fn ensure_gtk_init() -> bool {
    *GTK_INIT.get_or_init(|| {
        std::env::set_var("GTK_A11Y", "none");
        gtk4::init().is_ok()
    })
}

macro_rules! require_display {
    () => {
        if !ensure_gtk_init() {
            eprintln!("SKIPPED: no display available");
            return;
        }
    };
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
    let root_paned = widget2
        .downcast_ref::<gtk4::Paned>()
        .expect("Root should be a Paned");
    let t1_widget = root_paned.start_child().expect("Should have t1");
    assert_eq!(
        t1_widget.downcast_ref::<gtk4::Label>().unwrap().label(),
        "t1"
    );
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
