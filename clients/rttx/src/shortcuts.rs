use std::collections::BTreeMap;

/// A customizable shortcut definition: action name, human-readable label,
/// and default GTK accelerator strings.
#[derive(Debug)]
pub struct ShortcutDef {
    pub action: &'static str,
    pub label: &'static str,
    pub default_accels: &'static [&'static str],
}

/// All customizable shortcuts with their defaults.
///
/// Alt+1–9 workspace switching is intentionally excluded — those are
/// positional and not meaningful to remap.
pub static DEFAULT_SHORTCUTS: &[ShortcutDef] = &[
    ShortcutDef {
        action: "new-session",
        label: "New workspace",
        default_accels: &["<Ctrl><Shift>T"],
    },
    ShortcutDef {
        action: "close-terminal",
        label: "Close pane",
        default_accels: &["<Ctrl><Shift>W"],
    },
    ShortcutDef {
        action: "split-horizontal",
        label: "Split horizontal",
        default_accels: &["<Ctrl><Shift>E"],
    },
    ShortcutDef {
        action: "split-vertical",
        label: "Split vertical",
        default_accels: &["<Ctrl><Shift>O"],
    },
    ShortcutDef { action: "search", label: "Search", default_accels: &["<Ctrl><Shift>F"] },
    ShortcutDef { action: "copy", label: "Copy", default_accels: &["<Ctrl><Shift>C"] },
    ShortcutDef { action: "paste", label: "Paste", default_accels: &["<Ctrl><Shift>V"] },
    ShortcutDef {
        action: "prev-session",
        label: "Previous workspace",
        default_accels: &["<Ctrl><Shift>Tab"],
    },
    ShortcutDef { action: "next-session", label: "Next workspace", default_accels: &["<Ctrl>Tab"] },
    ShortcutDef {
        action: "toggle-sidebar",
        label: "Toggle workspace sidebar",
        default_accels: &["<Ctrl><Shift>N"],
    },
    ShortcutDef {
        action: "toggle-utility-sidebar",
        label: "Toggle tools sidebar",
        default_accels: &["<Ctrl><Shift>B"],
    },
    ShortcutDef {
        action: "toggle-input-sync",
        label: "Toggle input sync",
        default_accels: &["<Ctrl><Shift>i"],
    },
    ShortcutDef { action: "fullscreen", label: "Fullscreen", default_accels: &["F11"] },
    ShortcutDef {
        action: "zoom-in",
        label: "Zoom in",
        default_accels: &["<Ctrl>plus", "<Ctrl>equal"],
    },
    ShortcutDef { action: "zoom-out", label: "Zoom out", default_accels: &["<Ctrl>minus"] },
    ShortcutDef { action: "zoom-reset", label: "Zoom reset", default_accels: &["<Ctrl>0"] },
    ShortcutDef {
        action: "toggle-pane-zoom",
        label: "Zoom pane (toggle)",
        default_accels: &["<Ctrl><Shift>Z"],
    },
    ShortcutDef {
        action: "rotate-layout",
        label: "Rotate layout",
        default_accels: &["<Ctrl><Shift>R"],
    },
    ShortcutDef {
        action: "connect-to-existing",
        label: "Connect to existing workspace",
        default_accels: &["<Ctrl><Shift>A"],
    },
    ShortcutDef {
        action: "new-direct",
        label: "New direct terminal",
        default_accels: &["<Ctrl><Shift>D"],
    },
    ShortcutDef { action: "preferences", label: "Preferences", default_accels: &["<Ctrl>comma"] },
    ShortcutDef { action: "navigate-left", label: "Navigate left", default_accels: &["<Alt>Left"] },
    ShortcutDef {
        action: "navigate-right",
        label: "Navigate right",
        default_accels: &["<Alt>Right"],
    },
    ShortcutDef { action: "navigate-up", label: "Navigate up", default_accels: &["<Alt>Up"] },
    ShortcutDef { action: "navigate-down", label: "Navigate down", default_accels: &["<Alt>Down"] },
];

/// Look up the effective accelerators for an action, checking user overrides
/// first, then falling back to the built-in default.
#[must_use]
pub fn effective_accels(action: &str, overrides: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    if let Some(custom) = overrides.get(action) {
        return custom.clone();
    }
    DEFAULT_SHORTCUTS
        .iter()
        .find(|d| d.action == action)
        .map(|d| d.default_accels.iter().map(|s| (*s).to_string()).collect())
        .unwrap_or_default()
}

/// Return the default accelerators for an action.
#[must_use]
pub fn default_accels(action: &str) -> Vec<String> {
    DEFAULT_SHORTCUTS
        .iter()
        .find(|d| d.action == action)
        .map(|d| d.default_accels.iter().map(|s| (*s).to_string()).collect())
        .unwrap_or_default()
}

/// Migrate legacy `PaneNavigationKeys` into shortcut overrides.
///
/// If the user had `CtrlShiftArrow` selected and no explicit overrides exist
/// for the navigation actions, populate them.
pub fn migrate_pane_navigation(
    legacy: &crate::preferences::PaneNavigationKeys,
    shortcuts: &mut BTreeMap<String, Vec<String>>,
) {
    use crate::preferences::PaneNavigationKeys;
    if *legacy == PaneNavigationKeys::AltArrow {
        return; // default — nothing to migrate
    }
    let (left, right, up, down) = legacy.accels();
    let nav = [
        ("navigate-left", left),
        ("navigate-right", right),
        ("navigate-up", up),
        ("navigate-down", down),
    ];
    for (action, accel) in nav {
        shortcuts.entry(action.to_string()).or_insert_with(|| vec![accel.to_string()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_accels_returns_default_when_no_override() {
        let overrides = BTreeMap::new();
        let accels = effective_accels("close-terminal", &overrides);
        assert_eq!(accels, vec!["<Ctrl><Shift>W"]);
    }

    #[test]
    fn effective_accels_returns_override_when_present() {
        let mut overrides = BTreeMap::new();
        overrides.insert("close-terminal".into(), vec!["<Ctrl>q".into()]);
        let accels = effective_accels("close-terminal", &overrides);
        assert_eq!(accels, vec!["<Ctrl>q"]);
    }

    #[test]
    fn effective_accels_returns_empty_for_unknown_action() {
        let overrides = BTreeMap::new();
        let accels = effective_accels("nonexistent-action", &overrides);
        assert!(accels.is_empty());
    }

    #[test]
    fn default_accels_returns_correct_values() {
        assert_eq!(default_accels("fullscreen"), vec!["F11"]);
        assert_eq!(default_accels("zoom-in"), vec!["<Ctrl>plus", "<Ctrl>equal"]);
    }

    #[test]
    fn override_with_empty_vec_disables_shortcut() {
        let mut overrides = BTreeMap::new();
        overrides.insert("fullscreen".into(), vec![]);
        let accels = effective_accels("fullscreen", &overrides);
        assert!(accels.is_empty());
    }

    #[test]
    fn all_default_shortcuts_have_unique_actions() {
        let mut seen = std::collections::HashSet::new();
        for def in DEFAULT_SHORTCUTS {
            assert!(seen.insert(def.action), "duplicate action: {}", def.action);
        }
    }

    #[test]
    fn all_default_shortcuts_have_labels() {
        for def in DEFAULT_SHORTCUTS {
            assert!(!def.label.is_empty(), "empty label for action: {}", def.action);
        }
    }

    #[test]
    fn migrate_pane_navigation_noop_for_alt_arrow() {
        use crate::preferences::PaneNavigationKeys;
        let mut shortcuts = BTreeMap::new();
        migrate_pane_navigation(&PaneNavigationKeys::AltArrow, &mut shortcuts);
        assert!(shortcuts.is_empty());
    }

    #[test]
    fn migrate_pane_navigation_populates_ctrl_shift_arrow() {
        use crate::preferences::PaneNavigationKeys;
        let mut shortcuts = BTreeMap::new();
        migrate_pane_navigation(&PaneNavigationKeys::CtrlShiftArrow, &mut shortcuts);
        assert_eq!(shortcuts["navigate-left"], vec!["<Ctrl><Shift>Left"]);
        assert_eq!(shortcuts["navigate-right"], vec!["<Ctrl><Shift>Right"]);
        assert_eq!(shortcuts["navigate-up"], vec!["<Ctrl><Shift>Up"]);
        assert_eq!(shortcuts["navigate-down"], vec!["<Ctrl><Shift>Down"]);
    }

    #[test]
    fn migrate_pane_navigation_does_not_overwrite_existing() {
        use crate::preferences::PaneNavigationKeys;
        let mut shortcuts = BTreeMap::new();
        shortcuts.insert("navigate-left".into(), vec!["<Alt>h".into()]);
        migrate_pane_navigation(&PaneNavigationKeys::CtrlShiftArrow, &mut shortcuts);
        assert_eq!(shortcuts["navigate-left"], vec!["<Alt>h"]);
        assert_eq!(shortcuts["navigate-right"], vec!["<Ctrl><Shift>Right"]);
    }
}
