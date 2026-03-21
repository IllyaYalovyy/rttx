pub mod layout;

pub use layout::{
    LayoutNode, PaneRecovery, PaneSource, SessionState, SplitOrientation, StartupStep,
    WindowState,
};

use crate::config;
use gtk4::glib;
use gtk4::prelude::*;
use std::fs;
use std::path::PathBuf;

/// Returns the path to the sessions directory in `XDG_CONFIG_HOME`.
#[must_use]
pub fn sessions_dir() -> Option<PathBuf> {
    let mut path = glib::user_config_dir();
    path.push(config::CONFIG_DIR);
    Some(path)
}

/// Save the current window state to a JSON file.
pub fn save_window_state(state: &WindowState) -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut path) = sessions_dir() else {
        return Ok(());
    };
    fs::create_dir_all(&path)?;
    path.push("sessions.json");
    let json = serde_json::to_string_pretty(state)?;
    fs::write(path, json)?;
    Ok(())
}

/// Load the window state from the JSON file, or return default.
#[must_use]
pub fn load_window_state() -> WindowState {
    let Some(mut path) = sessions_dir() else {
        return WindowState::default();
    };
    path.push("sessions.json");
    fs::read_to_string(path).map_or_else(
        |_| WindowState::default(),
        |json| serde_json::from_str(&json).unwrap_or_default(),
    )
}

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
    let LayoutNode::Split { orientation, ref mut ratio, first, second } = layout else {
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

/// Build a tree of `GtkPaned` widgets matching the `LayoutNode` structure.
pub fn build_layout_widget<F>(layout: &LayoutNode, make_terminal: &F) -> gtk4::Widget
where
    F: Fn(&str, Option<&str>, Option<&str>, Option<&str>) -> gtk4::Widget,
{
    match layout {
        LayoutNode::Terminal { uuid, cwd, profile, custom_title } => {
            make_terminal(uuid, cwd.as_deref(), profile.as_deref(), custom_title.as_deref())
        }
        LayoutNode::Split { orientation, ratio: _, first, second } => {
            let gtk_orientation = match orientation {
                SplitOrientation::Horizontal => gtk4::Orientation::Horizontal,
                SplitOrientation::Vertical => gtk4::Orientation::Vertical,
            };
            let paned = gtk4::Paned::new(gtk_orientation);
            paned.set_wide_handle(true);
            paned.set_position(200);

            let first_widget = build_layout_widget(first, make_terminal);
            let second_widget = build_layout_widget(second, make_terminal);

            paned.set_start_child(Some(&first_widget));
            paned.set_end_child(Some(&second_widget));

            paned.upcast()
        }
    }
}

pub fn schedule_initial_paned_ratios(content: &gtk4::Widget, layout: &LayoutNode) {
    if !matches!(layout, LayoutNode::Split { .. }) {
        return;
    }

    let idle_layout = layout.clone();
    glib::idle_add_local_once(glib::clone!(
        #[weak]
        content,
        move || {
            apply_initial_paned_ratios(&idle_layout, &content);
        }
    ));

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
        let current = paned.position();
        if current == 0 || current == 200 {
            paned.set_position((f64::from(total) * ratio.clamp(0.05, 0.95)) as i32);
        }
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
    use tempfile::TempDir;

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut state = WindowState::default();
        state.width = 123;
        state.height = 456;

        let path = tmp.path().join("sessions.json");
        let json = serde_json::to_string_pretty(&state).unwrap();
        fs::write(&path, json).unwrap();

        let loaded = load_state_from(tmp.path());
        assert_eq!(state.width, loaded.width);
        assert_eq!(state.height, loaded.height);
    }

    #[test]
    fn save_complex_layout_and_reload() {
        let tmp = TempDir::new().unwrap();
        let mut state = WindowState::default();
        let root = LayoutNode::Terminal {
            uuid: "t1".into(),
            profile: None,
            cwd: None,
            custom_title: None,
        };
        state.sessions[0].layout = root.split(SplitOrientation::Horizontal);

        let path = tmp.path().join("sessions.json");
        let json = serde_json::to_string_pretty(&state).unwrap();
        fs::write(&path, json).unwrap();

        let loaded = load_state_from(tmp.path());
        assert_eq!(state.sessions[0].layout, loaded.sessions[0].layout);
    }

    #[rstest::rstest]
    #[case(0)]
    #[case(1)]
    #[case(99)]
    fn window_state_active_index_preserved(#[case] index: usize) {
        let tmp = TempDir::new().unwrap();
        let mut state = WindowState::default();
        state.active_session_index = index;

        let path = tmp.path().join("sessions.json");
        let json = serde_json::to_string_pretty(&state).unwrap();
        fs::write(&path, json).unwrap();

        let loaded = load_state_from(tmp.path());
        assert_eq!(state.active_session_index, loaded.active_session_index);
    }

    #[test]
    fn load_returns_default_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let loaded = load_state_from(tmp.path());
        assert_eq!(loaded, WindowState::default_for_test());
    }

    #[test]
    fn load_returns_default_on_corrupt_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("sessions.json");
        fs::write(path, "{corrupt").unwrap();
        let loaded = load_state_from(tmp.path());
        assert_eq!(loaded, WindowState::default_for_test());
    }

    fn load_state_from(dir: &std::path::Path) -> WindowState {
        let path = dir.join("sessions.json");
        fs::read_to_string(path).map_or_else(
            |_| WindowState::default_for_test(),
            |json| serde_json::from_str(&json).unwrap_or_else(|_| WindowState::default_for_test()),
        )
    }
}
