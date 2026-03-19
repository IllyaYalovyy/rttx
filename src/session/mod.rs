pub mod layout;

pub use layout::{LayoutNode, SessionState, SplitOrientation, WindowState};

use crate::config;
use gtk4::glib;
use gtk4::prelude::*;
use std::fs;
use std::path::PathBuf;

/// Returns the path to the sessions directory, creating it if needed.
pub fn sessions_dir() -> Option<PathBuf> {
    let config = glib::user_config_dir();
    let dir = config.join(config::CONFIG_DIR).join(config::SESSIONS_DIR);
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Save window state to disk.
pub fn save_window_state(state: &WindowState) -> Result<(), Box<dyn std::error::Error>> {
    let dir = sessions_dir().ok_or("Cannot determine config directory")?;
    let path = dir.join("window-state.json");
    let json = serde_json::to_string_pretty(state)?;
    fs::write(path, json)?;
    Ok(())
}

/// Load window state from disk, returning default if not found.
pub fn load_window_state() -> WindowState {
    let dir = match sessions_dir() {
        Some(d) => d,
        None => return WindowState::default(),
    };
    let path = dir.join("window-state.json");
    match fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => WindowState::default(),
    }
}

/// Build the GTK widget tree for a layout node.
pub fn build_layout_widget(
    node: &LayoutNode,
    make_terminal: &dyn Fn(&str, Option<&str>, Option<&str>) -> gtk4::Widget,
) -> gtk4::Widget {
    match node {
        LayoutNode::Terminal {
            uuid,
            cwd,
            custom_title: _,
            profile: _,
        } => make_terminal(uuid, cwd.as_deref(), None),
        LayoutNode::Split {
            orientation,
            ratio,
            first,
            second,
        } => {
            let gtk_orientation = match orientation {
                SplitOrientation::Horizontal => gtk4::Orientation::Horizontal,
                SplitOrientation::Vertical => gtk4::Orientation::Vertical,
            };
            let paned = gtk4::Paned::new(gtk_orientation);
            paned.set_wide_handle(true);

            let first_widget = build_layout_widget(first, make_terminal);
            let second_widget = build_layout_widget(second, make_terminal);

            paned.set_start_child(Some(&first_widget));
            paned.set_end_child(Some(&second_widget));

            let ratio_val = *ratio;
            paned.connect_realize(move |p| {
                let size = match gtk_orientation {
                    gtk4::Orientation::Horizontal => p.width(),
                    _ => p.height(),
                };
                if size > 0 {
                    p.set_position((size as f64 * ratio_val) as i32);
                }
            });

            paned.upcast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // ── Persistence tests (using test_helpers to bypass glib) ────

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = window_state(vec![
            session("s1", "Session 1", term("t1")),
            session("s2", "Session 2", hsplit(term("t2"), term("t3"))),
        ]);

        save_state_to(tmp.path(), &state).unwrap();
        let loaded = load_state_from(tmp.path());
        assert_eq!(state, loaded);
    }

    #[test]
    fn load_returns_default_when_no_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let loaded = load_state_from(tmp.path());
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.active_session_index, 0);
        assert_eq!(loaded.width, 900);
    }

    #[test]
    fn load_returns_default_on_corrupt_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp
            .path()
            .join(config::CONFIG_DIR)
            .join(config::SESSIONS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("window-state.json"), "not valid json{{{").unwrap();

        let loaded = load_state_from(tmp.path());
        assert_eq!(loaded.sessions.len(), 1);
    }

    #[test]
    fn save_complex_layout_and_reload() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = window_state(vec![session(
            "s1",
            "Complex",
            hsplit(
                vsplit(
                    term_full("t1", "/home/user/project", "editor"),
                    term_full("t2", "/home/user/project", "build"),
                ),
                vsplit(term("t3"), term("t4")),
            ),
        )]);

        save_state_to(tmp.path(), &state).unwrap();
        let loaded = load_state_from(tmp.path());
        assert_eq!(state, loaded);
        assert_eq!(loaded.sessions[0].layout.terminal_count(), 4);
    }

    // ── Parameterized persistence edge cases ─────────────────────

    #[rstest]
    #[case(0, true)]
    #[case(1, false)]
    #[case(5, false)]
    fn window_state_active_index_preserved(
        #[case] active_index: usize,
        #[case] is_maximized: bool,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sessions = vec![];
        for i in 0..=active_index.max(1) {
            sessions.push(session(
                &format!("s{i}"),
                &format!("Session {i}"),
                term(&format!("t{i}")),
            ));
        }
        let state = WindowState {
            sessions,
            active_session_index: active_index.min(active_index.max(1)),
            width: 1920,
            height: 1080,
            is_maximized,
        };

        save_state_to(tmp.path(), &state).unwrap();
        let loaded = load_state_from(tmp.path());
        assert_eq!(state.active_session_index, loaded.active_session_index);
        assert_eq!(state.is_maximized, loaded.is_maximized);
        assert_eq!(state.width, loaded.width);
    }
}
