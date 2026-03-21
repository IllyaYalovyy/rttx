/// Contract tests for the Rust/GTK boundary.
///
/// GTK is a C library. Our Rust code makes assumptions about GTK behavior
/// that, if violated, cause segfaults or panics in C code (not Rust panics).
/// These tests validate those assumptions without requiring a display server.
///
/// Categories:
/// 1. Layout tree → widget tree mapping contracts
/// 2. Serialization contracts (what we persist must be valid when restored)
/// 3. State machine contracts (valid operation sequences)
/// 4. Data integrity contracts (what crosses the Rust/C boundary)
use pretty_assertions::assert_eq;
use rttx::session::layout::*;

fn term(id: &str) -> LayoutNode {
    LayoutNode::Terminal { uuid: id.into(), profile: None, cwd: None, custom_title: None }
}
fn term_full(id: &str, cwd: &str, title: &str) -> LayoutNode {
    LayoutNode::Terminal {
        uuid: id.into(),
        profile: Some("default".into()),
        cwd: Some(cwd.into()),
        custom_title: Some(title.into()),
    }
}
fn hsplit(a: LayoutNode, b: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(a),
        second: Box::new(b),
    }
}
fn vsplit(a: LayoutNode, b: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Vertical,
        ratio: 0.5,
        first: Box::new(a),
        second: Box::new(b),
    }
}

// ═══════════════════════════════════════════════════════════════════
// 1. WIDGET TREE MAPPING CONTRACTS
//
// Our code builds a gtk::Paned tree from LayoutNode. These tests verify
// the LayoutNode invariants that the widget builder relies on.
// ═══════════════════════════════════════════════════════════════════

/// Contract: every terminal UUID in a layout must be unique.
/// Violation would cause: two VTE widgets sharing the same HashMap key,
/// one silently dropped → C-level use-after-free when GTK tries to draw it.
#[test]
fn contract_all_uuids_unique_after_any_operation_sequence() {
    let mut layout = term("t1");

    // Split 5 times
    for _ in 0..5 {
        layout = layout.split_terminal("t1", SplitOrientation::Horizontal).unwrap();
    }

    let uuids = layout.terminal_uuids();
    let unique: std::collections::HashSet<_> = uuids.iter().collect();
    assert_eq!(uuids.len(), unique.len(), "Duplicate UUIDs after splits: {:?}", uuids);

    // Remove some, split others
    let to_remove = uuids[1].clone();
    layout = layout.remove_terminal(&to_remove).unwrap();
    let remaining = layout.terminal_uuids();
    let unique2: std::collections::HashSet<_> = remaining.iter().collect();
    assert_eq!(remaining.len(), unique2.len(), "Duplicate UUIDs after remove: {:?}", remaining);
}

/// Contract: terminal_count must always equal terminal_uuids().len().
/// Violation would cause: widget builder creates wrong number of VTE widgets,
/// some paned children are null → GTK null pointer dereference.
#[test]
fn contract_count_equals_uuid_vec_length() {
    let layouts = vec![
        term("t1"),
        hsplit(term("t1"), term("t2")),
        vsplit(hsplit(term("a"), term("b")), hsplit(term("c"), term("d"))),
    ];
    for layout in layouts {
        assert_eq!(
            layout.terminal_count(),
            layout.terminal_uuids().len(),
            "Count/UUID mismatch for: {:?}",
            layout
        );
    }
}

/// Contract: split_terminal must return a tree where the original terminal
/// still exists. Violation would cause: widget builder can't find the
/// existing VTE widget in the HashMap → creates a duplicate, old one leaks.
#[test]
fn contract_split_never_loses_original() {
    let layout = hsplit(vsplit(term("a"), term("b")), term("c"));
    for uuid in layout.terminal_uuids() {
        let result = layout.split_terminal(&uuid, SplitOrientation::Horizontal).unwrap();
        assert!(result.contains_terminal(&uuid), "Original terminal '{uuid}' lost after split");
        // All OTHER terminals must also survive
        for other in layout.terminal_uuids() {
            assert!(
                result.contains_terminal(&other),
                "Sibling terminal '{other}' lost when splitting '{uuid}'"
            );
        }
    }
}

/// Contract: remove_terminal must not leave dangling references.
/// After removal, the UUID must not appear anywhere in the tree.
/// Violation would cause: widget builder tries to look up a removed terminal
/// in the HashMap → None → panic or null widget in Paned.
#[test]
fn contract_remove_leaves_no_dangling_references() {
    let layout = hsplit(vsplit(term("a"), term("b")), hsplit(term("c"), term("d")));
    for uuid in layout.terminal_uuids() {
        if let Some(result) = layout.remove_terminal(&uuid) {
            assert!(
                !result.contains_terminal(&uuid),
                "Terminal '{uuid}' still in tree after removal"
            );
            assert!(
                !result.terminal_uuids().contains(&uuid.to_string()),
                "Terminal '{uuid}' still in UUID list after removal"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// 2. SERIALIZATION CONTRACTS
//
// What we write to disk must be valid when read back. Invalid data
// causes panics or C crashes when we try to build widgets from it.
// ═══════════════════════════════════════════════════════════════════

/// Contract: any WindowState we can construct must survive JSON roundtrip.
/// Violation would cause: app crashes on startup when restoring state.
#[test]
fn contract_any_constructible_state_roundtrips() {
    let states = vec![
        // Minimal
        WindowState::default(),
        // Empty session name
        WindowState {
            sessions: vec![SessionState {
                uuid: "s".into(),
                name: "".into(),
                layout: term("t"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
            }],
            active_session_index: 0,
            width: 1,
            height: 1,
            is_maximized: false,
        },
        // Unicode in names
        WindowState {
            sessions: vec![SessionState {
                uuid: "s".into(),
                name: "日本語セッション".into(),
                layout: term_full("t", "/home/用户", "编辑器"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: true,
            }],
            active_session_index: 0,
            width: 800,
            height: 600,
            is_maximized: false,
        },
        // Deep nesting
        {
            let mut l = term("t0");
            for i in 1..20 {
                l = hsplit(l, term(&format!("t{i}")));
            }
            WindowState {
                sessions: vec![SessionState {
                    uuid: "s".into(),
                    name: "Deep".into(),
                    layout: l,
                    terminal_recovery: Default::default(),
                    active_terminal_uuid: None,
                    input_sync: false,
                }],
                active_session_index: 0,
                width: 800,
                height: 600,
                is_maximized: false,
            }
        },
        // Many sessions
        {
            let sessions: Vec<_> = (0..50)
                .map(|i| SessionState {
                    uuid: format!("s{i}"),
                    name: format!("Session {i}"),
                    layout: term(&format!("t{i}")),
                    terminal_recovery: Default::default(),
                    active_terminal_uuid: None,
                    input_sync: i % 2 == 0,
                })
                .collect();
            WindowState {
                sessions,
                active_session_index: 25,
                width: 1920,
                height: 1080,
                is_maximized: true,
            }
        },
    ];

    for (i, state) in states.iter().enumerate() {
        let json = serde_json::to_string(state)
            .unwrap_or_else(|e| panic!("State {i} failed to serialize: {e}"));
        let restored: WindowState = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("State {i} failed to deserialize: {e}"));
        assert_eq!(*state, restored, "State {i} roundtrip mismatch");
    }
}

/// Contract: state saved by an older version (missing new fields) must load
/// without error. Violation would cause: app crashes on startup after upgrade.
#[test]
fn contract_forward_compat_missing_fields() {
    // Simulate v0.1 state without input_sync or custom_title
    let old_json = r#"{
        "sessions": [{
            "uuid": "s1", "name": "Old",
            "layout": {"Split": {
                "orientation": "horizontal",
 "ratio": 0.5,
                "first": {"Terminal": {"uuid": "t1", "profile": null, "cwd": "/home", "custom_title": null}},
                "second": {"Terminal": {"uuid": "t2", "profile": null, "cwd": null, "custom_title": null}}
            }}
        }],
        "active_session_index": 0, "width": 800, "height": 600, "is_maximized": false
    }"#;

    let state: WindowState =
        serde_json::from_str(old_json).expect("Must load old format without error");
    assert_eq!(state.sessions[0].layout.terminal_count(), 2);
    assert!(!state.sessions[0].input_sync); // default
}

/// Contract: state with extra unknown fields must load without error.
/// Violation would cause: app crashes if we add fields in a future version
/// and the user downgrades.
#[test]
fn contract_backward_compat_extra_fields() {
    let future_json = r#"{
        "sessions": [{
            "uuid": "s1", "name": "Future",
            "layout": {"Terminal": {"uuid": "t1", "profile": null, "cwd": null, "custom_title": null}},
            "input_sync": false,
            "future_field": "should be ignored",
            "another_future": 42
        }],
        "active_session_index": 0, "width": 800, "height": 600, "is_maximized": false,
        "future_window_field": true
    }"#;

    let state: WindowState =
        serde_json::from_str(future_json).expect("Must tolerate unknown fields");
    assert_eq!(state.sessions[0].name, "Future");
}

// ═══════════════════════════════════════════════════════════════════
// 3. STATE MACHINE CONTRACTS
//
// Valid sequences of operations that the window.rs code performs.
// These test the layout model under the exact patterns the UI uses.
// ═══════════════════════════════════════════════════════════════════

/// Contract: the "close last terminal in session" path.
/// Window code checks terminal_count() <= 1 before calling remove_terminal.
/// If this invariant breaks, we'd call close_session with a stale UUID.
#[test]
fn contract_single_terminal_session_close_path() {
    let layout = term("t1");
    assert_eq!(layout.terminal_count(), 1);
    // Window code does: if count <= 1 { close_session } else { remove_terminal }
    // Verify remove_terminal returns None for the last terminal
    assert!(layout.remove_terminal("t1").is_none());
}

/// Contract: the "split then immediately close new terminal" path.
/// This is a common user action. The new terminal's UUID must be discoverable.
#[test]
fn contract_split_then_close_new_terminal() {
    let layout = term("t1");
    let after_split = layout.split_terminal("t1", SplitOrientation::Horizontal).unwrap();

    let new_uuid = after_split
        .terminal_uuids()
        .into_iter()
        .find(|u| u != "t1")
        .expect("Must be able to find the new terminal's UUID");

    let after_close = after_split.remove_terminal(&new_uuid).unwrap();
    assert_eq!(after_close.terminal_count(), 1);
    assert!(after_close.contains_terminal("t1"));
}

/// Contract: rapid split-close cycles must not corrupt the tree.
/// Simulates a user rapidly splitting and closing.
#[test]
fn contract_rapid_split_close_cycles() {
    let mut layout = hsplit(term("t1"), term("t2"));

    for _ in 0..10 {
        // Split t1
        layout = layout.split_terminal("t1", SplitOrientation::Vertical).unwrap();
        let new_uuid =
            layout.terminal_uuids().into_iter().find(|u| u != "t1" && u != "t2").unwrap();
        // Close the new one
        layout = layout.remove_terminal(&new_uuid).unwrap();
    }

    // Must be back to original structure
    assert_eq!(layout.terminal_count(), 2);
    assert!(layout.contains_terminal("t1"));
    assert!(layout.contains_terminal("t2"));
}

/// Contract: swap_terminals followed by rebuild must produce a valid tree.
/// The widget builder iterates terminal_uuids() to find widgets in the HashMap.
/// After swap, the UUIDs must still all be present and unique.
#[test]
fn contract_swap_then_rebuild_is_valid() {
    let mut layout = hsplit(
        vsplit(term_full("a", "/a", "A"), term_full("b", "/b", "B")),
        term_full("c", "/c", "C"),
    );

    layout.swap_terminals("a", "c");

    // All original UUIDs must still be present
    let uuids = layout.terminal_uuids();
    assert!(uuids.contains(&"a".to_string()));
    assert!(uuids.contains(&"b".to_string()));
    assert!(uuids.contains(&"c".to_string()));
    assert_eq!(uuids.len(), 3);

    // Must still serialize
    let json = serde_json::to_string(&layout).unwrap();
    let restored: LayoutNode = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.terminal_count(), 3);
}

// ═══════════════════════════════════════════════════════════════════
// 4. DATA INTEGRITY AT THE C BOUNDARY
//
// Values that cross into GTK/VTE C code. Invalid values here cause
// C-level crashes, not Rust panics.
// ═══════════════════════════════════════════════════════════════════

/// Contract: CWD paths restored from JSON must be valid UTF-8 strings.
/// VTE's spawn_async takes a &str for cwd. If we somehow stored invalid
/// UTF-8, the conversion would panic before reaching C code.
#[test]
fn contract_cwd_paths_are_valid_utf8() {
    let paths = vec![
        "/home/user/project",
        "/tmp/build-output",
        "/home/用户/项目",         // CJK
        "/home/user/my project",   // spaces
        "/home/user/.config/rttx", // dots
    ];

    for path in paths {
        let layout = LayoutNode::Terminal {
            uuid: "t1".into(),
            profile: None,
            cwd: Some(path.into()),
            custom_title: None,
        };
        let json = serde_json::to_string(&layout).unwrap();
        let restored: LayoutNode = serde_json::from_str(&json).unwrap();
        if let LayoutNode::Terminal { cwd, .. } = &restored {
            assert_eq!(cwd.as_deref(), Some(path));
        }
    }
}

/// Contract: color hex strings must be parseable by gdk::RGBA::parse.
/// Invalid colors passed to VTE's set_colors cause C assertion failures.
#[test]
fn contract_test_palette_colors_are_valid() {
    use rttx::color_scheme::ColorScheme;

    let palette = [
        "#2E3436", "#CC0000", "#4E9A06", "#C4A000", "#3465A4", "#75507B", "#06989A", "#D3D7CF",
        "#555753", "#EF2929", "#8AE234", "#FCE94F", "#729FCF", "#AD7FA8", "#34E2E2", "#EEEEEC",
    ];
    for (i, color) in palette.iter().enumerate() {
        assert!(
            ColorScheme::parse_color(color).is_some(),
            "Palette[{i}] = '{color}' failed to parse"
        );
    }
}

/// Contract: split ratios must be in (0, 1). GTK Paned position is
/// calculated as ratio * size. Ratio <= 0 or >= 1 means one child
/// gets zero size → GTK rendering issues or division by zero.
#[test]
fn contract_split_ratios_in_valid_range() {
    let layout = hsplit(vsplit(term("a"), term("b")), hsplit(term("c"), term("d")));

    fn check_ratios(node: &LayoutNode) {
        if let LayoutNode::Split { ratio, first, second, .. } = node {
            assert!(*ratio > 0.0 && *ratio < 1.0, "Invalid ratio {ratio} — must be in (0, 1)");
            check_ratios(first);
            check_ratios(second);
        }
    }
    check_ratios(&layout);

    // Also check after split operations
    let after = layout.split_terminal("a", SplitOrientation::Vertical).unwrap();
    check_ratios(&after);
}

/// Contract: preferences font string must be parseable by Pango.
/// Invalid font descriptions passed to vte.set_font() cause C warnings
/// and potentially undefined behavior.
#[test]
fn contract_default_font_is_valid_pango_description() {
    let prefs = rttx::preferences::Preferences::default();
    // Pango font descriptions have format "Family Size" or "Family Style Size"
    // At minimum, must contain a size number
    assert!(
        prefs.font.chars().any(|c| c.is_ascii_digit()),
        "Default font '{}' has no size component",
        prefs.font
    );
    assert!(!prefs.font.is_empty());
}

/// Contract: when a single-terminal session is split, the layout must
/// transition from Terminal to Split correctly. This is the exact path
/// that caused the "duplicate child name in GtkStack" crash — the old
/// terminal was the direct child of the Stack, and unparenting it before
/// removing from the Stack broke GTK's parent invariant.
#[test]
fn contract_single_terminal_split_produces_valid_tree() {
    let layout = term("t1");
    assert_eq!(layout.terminal_count(), 1);

    // This is what window.rs does on split
    let new_layout = layout.split_terminal("t1", SplitOrientation::Horizontal).unwrap();
    assert_eq!(new_layout.terminal_count(), 2);
    assert!(new_layout.contains_terminal("t1"));

    // The new layout must be a Split (not a Terminal) — this is what
    // determines whether the Stack child is a Paned or a TerminalWidget
    assert!(
        matches!(new_layout, LayoutNode::Split { .. }),
        "Split of single terminal must produce Split node, got Terminal"
    );

    // The old terminal must be findable for reuse
    let uuids = new_layout.terminal_uuids();
    assert!(uuids.contains(&"t1".to_string()));
}

// ═══════════════════════════════════════════════════════════════════
// 5. WIDGET REUSE CONTRACTS
//
// window.rs reuses existing TerminalWidget instances across rebuilds
// by looking them up by UUID in a HashMap. These tests verify the
// invariants that make reuse safe: all pre-existing UUIDs survive
// every operation so the HashMap lookup always succeeds.
//
// If a UUID disappeared, rebuild_session_content would call
// make_terminal() for it again → signals connected twice → C4 crash.
// ═══════════════════════════════════════════════════════════════════

/// Contract: after a split, every UUID that existed before must still
/// exist after. window.rs uses this to find the existing TerminalWidget
/// in the HashMap for reuse — no second widget creation, no doubled signals.
#[test]
fn contract_split_preserves_all_existing_uuids() {
    let layouts = vec![
        term("t1"),
        hsplit(term("t1"), term("t2")),
        vsplit(hsplit(term("a"), term("b")), term("c")),
    ];

    for layout in &layouts {
        let before: std::collections::HashSet<_> = layout.terminal_uuids().into_iter().collect();

        for uuid in layout.terminal_uuids() {
            let after_split = layout.split_terminal(&uuid, SplitOrientation::Horizontal).unwrap();
            let after: std::collections::HashSet<_> =
                after_split.terminal_uuids().into_iter().collect();

            assert!(
                before.is_subset(&after),
                "Split of '{uuid}' lost pre-existing UUID(s): {:?}",
                before.difference(&after).collect::<Vec<_>>()
            );
        }
    }
}

/// Contract: after closing a terminal, every OTHER terminal's UUID must
/// still be present in the new layout. window.rs reuses those widgets.
#[test]
fn contract_remove_preserves_all_sibling_uuids() {
    let layout = vsplit(hsplit(term("a"), term("b")), hsplit(term("c"), term("d")));

    for uuid_to_close in layout.terminal_uuids() {
        if let Some(new_layout) = layout.remove_terminal(&uuid_to_close) {
            for sibling in layout.terminal_uuids() {
                if sibling == uuid_to_close {
                    continue;
                }
                assert!(
                    new_layout.contains_terminal(&sibling),
                    "Closing '{uuid_to_close}' lost sibling '{sibling}' — \
                     widget would be recreated and signals doubled"
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// 6. BORROW ORDERING CONTRACTS
//
// GTK signals fire synchronously during widget operations. If a
// RefCell is mutably borrowed when a signal fires and the handler
// also tries borrow_mut, Rust panics with BorrowMutError.
//
// These tests prove: (a) the wrong pattern panics, and (b) the
// correct pattern (extract-release-operate) does not.
// ═══════════════════════════════════════════════════════════════════

/// Documents what the WRONG pattern looks like and proves it panics.
/// This is the class of bug that caused the child_exited crash.
///
/// Simulates: state.borrow_mut() held → "signal fires" → handler calls
/// borrow_mut() again → BorrowMutError.
#[test]
#[should_panic(expected = "already borrowed")]
fn contract_refcell_reentrant_borrow_panics() {
    let state = std::rc::Rc::new(std::cell::RefCell::new(0i32));
    let state_clone = state.clone();

    // Simulate a GTK signal handler that borrows state
    let signal_handler = move || {
        *state_clone.borrow_mut() += 1;
    };

    // WRONG: hold borrow_mut while calling a function that also borrows_mut.
    // In GTK this happens when a widget op fires a signal while state is held.
    let _held = state.borrow_mut();
    signal_handler(); // BorrowMutError: already borrowed
}

/// Proves the correct pattern does not panic.
/// Extract needed data, release the borrow, THEN do the widget operation.
#[test]
fn contract_borrow_released_before_signal_does_not_panic() {
    let state = std::rc::Rc::new(std::cell::RefCell::new(0i32));
    let state_clone = state.clone();

    let signal_handler = move || {
        *state_clone.borrow_mut() += 1;
    };

    // CORRECT: extract data and release borrow before the widget op
    let _extracted = { *state.borrow() }; // borrow dropped at end of block
    signal_handler(); // safe: no active borrow

    assert_eq!(*state.borrow(), 1);
}

// ═══════════════════════════════════════════════════════════════════
// 7. INDEX BOUNDS CONTRACTS
//
// active_session_index is an unchecked index into sessions[]. If it
// is out of bounds, window.rs panics when accessing state.sessions[idx].
// ═══════════════════════════════════════════════════════════════════

/// Contract: a state file with active_session_index pointing past the
/// end of sessions must be handled without a panic.
/// Callers must clamp: idx.min(sessions.len().saturating_sub(1))
#[test]
fn contract_active_session_index_out_of_bounds_is_clamped_safely() {
    let json = r#"{
        "sessions": [
            {"uuid":"s1","name":"Only","layout":{"Terminal":{"uuid":"t1","profile":null,"cwd":null,"custom_title":null}},"input_sync":false}
        ],
        "active_session_index": 99,
        "width": 800, "height": 600, "is_maximized": false
    }"#;

    let loaded: WindowState =
        serde_json::from_str(json).expect("Must deserialize even with out-of-bounds index");

    // Prove the raw value is out of bounds
    assert!(loaded.active_session_index >= loaded.sessions.len());

    // Prove the correct clamping produces a valid index
    let safe = loaded.active_session_index.min(loaded.sessions.len().saturating_sub(1));
    assert!(safe < loaded.sessions.len(), "Clamped index {safe} is still out of bounds");
    // Must be accessible without panic
    let _ = &loaded.sessions[safe];
}

/// Contract: an empty sessions vec must never cause a panic on index access.
/// `sessions.get(idx)` must be used instead of `sessions[idx]` when
/// sessions could be empty.
#[test]
fn contract_empty_sessions_is_handled_without_panic() {
    let json = r#"{
        "sessions": [],
        "active_session_index": 0,
        "width": 800, "height": 600, "is_maximized": false
    }"#;

    let loaded: WindowState =
        serde_json::from_str(json).expect("Must deserialize empty sessions list");

    assert_eq!(loaded.sessions.len(), 0);

    // Safe access pattern: get() returns None instead of panicking
    assert!(
        loaded.sessions.get(loaded.active_session_index).is_none(),
        "sessions.get() must return None for empty vec, not panic"
    );

    // Direct index would panic — callers must use get() or guard on len()
    assert!(loaded.sessions.is_empty());
}
