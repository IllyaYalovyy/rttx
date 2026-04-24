pub mod layout;
pub mod recovery;
pub mod state;

pub use layout::{Direction, LayoutNode, MAX_SPLIT_DEPTH, SplitOrientation};
pub use recovery::{PaneRecovery, PaneSource, PaneTarget, StartupStep};
pub use state::{WindowState, WorkspaceColor, WorkspaceState};

use gtk4::glib;
use gtk4::prelude::*;

/// Walk the live widget tree and apply split ratios to `GtkPaned` positions.
///
/// Call this after the widget has been allocated to guarantee ratio-correct initial split positions.
pub fn apply_paned_ratios(layout: &LayoutNode, widget: &gtk4::Widget) {
    let LayoutNode::Split { orientation, ratio, first, second } = layout else {
        return;
    };

    let Some(paned) = widget.downcast_ref::<gtk4::Paned>() else {
        return;
    };

    let total = match orientation {
        SplitOrientation::Horizontal => paned.width(),
        SplitOrientation::Vertical => paned.height(),
    };

    if total > 0 {
        paned.set_position((f64::from(total) * ratio.clamp(0.05, 0.95)) as i32);
    }

    if let Some(first_child) = paned.start_child() {
        apply_paned_ratios(first, &first_child);
    }
    if let Some(second_child) = paned.end_child() {
        apply_paned_ratios(second, &second_child);
    }
}

/// Walk the live widget tree and update split ratios from divider positions.
///
/// Call this before serialising state so that user-adjusted splits are preserved.
pub fn capture_paned_ratios(layout: &mut LayoutNode, widget: &gtk4::Widget) {
    let LayoutNode::Split { orientation, ratio, first, second } = layout else {
        return;
    };

    let Some(paned) = widget.downcast_ref::<gtk4::Paned>() else {
        return;
    };

    let total = match orientation {
        SplitOrientation::Horizontal => paned.width(),
        SplitOrientation::Vertical => paned.height(),
    };

    if total > 0 {
        let new_ratio = f64::from(paned.position()) / f64::from(total);
        *ratio = new_ratio.clamp(0.05, 0.95);
    }

    if let Some(first_child) = paned.start_child() {
        capture_paned_ratios(first, &first_child);
    }
    if let Some(second_child) = paned.end_child() {
        capture_paned_ratios(second, &second_child);
    }
}

/// Terminal properties passed to the `build_layout_widget` closure.
#[derive(Debug)]
pub struct TerminalSpec<'a> {
    pub uuid: &'a str,
    pub cwd: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub custom_title: Option<&'a str>,
}

/// Build a tree of `GtkPaned` widgets matching the `LayoutNode` structure.
pub fn build_layout_widget<F>(layout: &LayoutNode, make_terminal: &F) -> gtk4::Widget
where
    F: Fn(TerminalSpec<'_>) -> gtk4::Widget,
{
    match layout {
        LayoutNode::Terminal { uuid, cwd, profile, custom_title } => make_terminal(TerminalSpec {
            uuid,
            cwd: cwd.as_deref(),
            profile: profile.as_deref(),
            custom_title: custom_title.as_deref(),
        }),
        LayoutNode::Split { orientation, ratio, first, second } => {
            let gtk_orientation = match orientation {
                SplitOrientation::Horizontal => gtk4::Orientation::Horizontal,
                SplitOrientation::Vertical => gtk4::Orientation::Vertical,
            };
            let paned = gtk4::Paned::new(gtk_orientation);
            paned.set_wide_handle(true);
            paned.set_resize_start_child(true);
            paned.set_resize_end_child(true);

            let first_widget = build_layout_widget(first, make_terminal);
            let second_widget = build_layout_widget(second, make_terminal);

            paned.set_start_child(Some(&first_widget));
            paned.set_end_child(Some(&second_widget));

            install_proportional_resize(&paned, *ratio);

            paned.upcast()
        }
    }
}

/// Attach a proportional-resize handler to a `GtkPaned`.
///
/// GTK4 `GtkPaned` distributes extra space equally (or only to one side) on
/// resize, which destroys the user's chosen split ratio.  This function
/// stores the desired ratio on the widget and reapplies it whenever the
/// paned's total allocation changes.
fn install_proportional_resize(paned: &gtk4::Paned, initial_ratio: f64) {
    use std::cell::Cell;

    let ratio = std::rc::Rc::new(Cell::new(initial_ratio.clamp(0.05, 0.95)));
    let last_total = std::rc::Rc::new(Cell::new(0i32));
    let applying = std::rc::Rc::new(Cell::new(false));

    // When the user drags the divider, update the stored ratio.
    {
        let ratio = ratio.clone();
        let applying = applying.clone();
        paned.connect_notify_local(Some("position"), move |p, _| {
            if applying.get() {
                return;
            }
            let total = match p.orientation() {
                gtk4::Orientation::Horizontal => p.width(),
                _ => p.height(),
            };
            if total > 0 {
                ratio.set((f64::from(p.position()) / f64::from(total)).clamp(0.05, 0.95));
            }
        });
    }

    // When the paned's size changes, reapply the stored ratio.
    paned.add_tick_callback(move |p, _| {
        let total = match p.orientation() {
            gtk4::Orientation::Horizontal => p.width(),
            _ => p.height(),
        };
        if total > 0 && total != last_total.get() {
            applying.set(true);
            p.set_position((f64::from(total) * ratio.get()) as i32);
            applying.set(false);
            last_total.set(total);
        }
        glib::ControlFlow::Continue
    });
}

pub fn schedule_initial_paned_ratios(content: &gtk4::Widget, layout: &LayoutNode) {
    if !matches!(layout, LayoutNode::Split { .. }) {
        return;
    }

    // Apply ratios on realize — before the first paint — to avoid a visible
    // jump from the default paned position to the target ratio.
    let realize_layout = layout.clone();
    content.connect_realize(move |widget| {
        apply_initial_paned_ratios(&realize_layout, widget);
    });

    let tick_layout = layout.clone();
    content.add_tick_callback(move |widget, _| {
        apply_initial_paned_ratios(&tick_layout, widget);
        if all_split_widgets_allocated(&tick_layout, widget) {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn all_split_widgets_allocated(layout: &LayoutNode, widget: &gtk4::Widget) -> bool {
    let LayoutNode::Split { orientation, first, second, .. } = layout else {
        return true;
    };

    let Some(paned) = widget.downcast_ref::<gtk4::Paned>() else {
        return false;
    };
    let total = match orientation {
        SplitOrientation::Horizontal => paned.width(),
        SplitOrientation::Vertical => paned.height(),
    };
    if total <= 0 {
        return false;
    }

    let Some(first_child) = paned.start_child() else {
        return false;
    };
    let Some(second_child) = paned.end_child() else {
        return false;
    };

    all_split_widgets_allocated(first, &first_child)
        && all_split_widgets_allocated(second, &second_child)
}

fn apply_initial_paned_ratios(layout: &LayoutNode, widget: &gtk4::Widget) {
    let LayoutNode::Split { orientation, ratio, first, second } = layout else {
        return;
    };

    let Some(paned) = widget.downcast_ref::<gtk4::Paned>() else {
        return;
    };

    let total = match orientation {
        SplitOrientation::Horizontal => paned.width(),
        SplitOrientation::Vertical => paned.height(),
    };

    if total > 0 {
        paned.set_position((f64::from(total) * ratio.clamp(0.05, 0.95)) as i32);
    }

    if let Some(first_child) = paned.start_child() {
        apply_initial_paned_ratios(first, &first_child);
    }
    if let Some(second_child) = paned.end_child() {
        apply_initial_paned_ratios(second, &second_child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let ok =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gtk4::init().is_ok()))
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

    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn apply_initial_paned_ratios_restores_nested_non_sentinel_positions() {
        require_display!();

        let layout = LayoutNode::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.35,
            first: Box::new(LayoutNode::Terminal {
                uuid: "t1".into(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
            second: Box::new(LayoutNode::Split {
                orientation: SplitOrientation::Vertical,
                ratio: 0.7,
                first: Box::new(LayoutNode::Terminal {
                    uuid: "t2".into(),
                    profile: None,
                    cwd: None,
                    custom_title: None,
                }),
                second: Box::new(LayoutNode::Terminal {
                    uuid: "t3".into(),
                    profile: None,
                    cwd: None,
                    custom_title: None,
                }),
            }),
        };

        let widget =
            build_layout_widget(&layout, &|spec| gtk4::Label::new(Some(spec.uuid)).upcast());

        let outer = widget.downcast_ref::<gtk4::Paned>().expect("root widget should be a Paned");
        outer.set_size_request(1000, 800);
        outer.allocate(1000, 800, -1, None);

        let inner = outer
            .end_child()
            .expect("nested branch should exist")
            .downcast::<gtk4::Paned>()
            .expect("nested branch should be a Paned");
        inner.set_size_request(650, 800);
        inner.allocate(650, 800, -1, None);

        outer.set_position(111);
        inner.set_position(123);

        apply_initial_paned_ratios(&layout, &widget);

        let outer_ratio = outer.position() as f64 / outer.width().max(1) as f64;
        let inner_ratio = inner.position() as f64 / inner.height().max(1) as f64;

        assert!(
            (outer_ratio - 0.35).abs() < 0.03,
            "outer split should restore saved ratio from a non-sentinel position, got {outer_ratio}"
        );
        assert!(
            (inner_ratio - 0.7).abs() < 0.03,
            "inner split should restore saved ratio from a non-sentinel position, got {inner_ratio}"
        );
    }
}
