# RFC-024: Customizable Keyboard Shortcuts

| Field         | Value         |
|---------------|---------------|
| Status        | Implemented   |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Allow users to customize every keyboard shortcut in rttx through the
preferences window. Shortcuts persist as a map of action-name to GTK
accelerator strings in `preferences.json`, storing only overrides from
the built-in defaults.

---

## Goals

- **G1** — Every shortcut listed in the README keyboard shortcuts table is
  user-customizable.
- **G2** — Existing preferences files load without error (backward compatible).
- **G3** — A "Reset to default" option per shortcut and a global reset.

## Non-Goals

- **NG1** — Per-workspace or per-profile shortcut sets.
- **NG2** — Vim/Emacs keybinding presets.
- **NG3** — Shortcut customization for Alt+1–9 workspace switching (these remain
  fixed).

---

## Background & Motivation

All keyboard shortcuts except pane navigation keys are hardcoded in
`window/actions.rs`. The existing `PaneNavigationKeys` enum in preferences
proves the pattern works but only covers one shortcut group with two presets.
Users who need different bindings (e.g., to avoid conflicts with tiling window
managers or accessibility tools) have no recourse.

Issue: https://github.com/IllyaYalovyy/rttx/issues/71

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Can remap any shortcut from Preferences → Keyboard |
| Contributors | New shortcuts automatically appear in the customization UI |
| Packagers    | No impact — preferences.json format is backward compatible |

---

## Design

### Data model

A new `keyboard_shortcuts` field in `Preferences`:

```rust
#[serde(default)]
pub keyboard_shortcuts: BTreeMap<String, Vec<String>>,
```

Keys are action names (e.g., `"close-terminal"`). Values are GTK accelerator
string arrays (e.g., `["<Ctrl><Shift>W"]`). Only overrides are stored; absent
keys use the built-in default.

### Default shortcut table

A `const` array `DEFAULT_SHORTCUTS` in a new `shortcuts.rs` module defines
every customizable shortcut with its action name, human-readable label, and
default accelerator(s). This is the single source of truth for both
registration and the preferences UI.

### Registration

`setup_actions` consults `preferences.keyboard_shortcuts` for each action.
If the action has an override, those accelerators are used; otherwise the
default from `DEFAULT_SHORTCUTS` is used.

### Preferences UI

The Keyboard section in the preferences window shows one row per shortcut.
Each row displays the action label and current binding. Clicking a row opens
a capture dialog where the user presses the desired key combination. A
"Reset" button per row restores the default.

### Migration

The existing `pane_navigation_keys` field is preserved for deserialization.
During load, if `pane_navigation_keys` is set to `CtrlShiftArrow` and no
explicit `keyboard_shortcuts` override exists for the navigation actions,
the navigation overrides are populated from the enum value. New saves always
use `keyboard_shortcuts`.

---

## Development Plan

- [x] **Step 1** — Add `shortcuts.rs` with default table and lookup
- [x] **Step 2** — Add `keyboard_shortcuts` to `Preferences` with serde
- [x] **Step 3** — Wire `setup_actions` and `reapply_terminal_preferences`
- [x] **Step 4** — Build preferences UI for shortcut customization
- [x] **Step 5** — Add unit tests for the data model and migration
- [x] **Step 6** — Add GTK tests for the preferences UI

---

## References

- [Tracking issue](https://github.com/IllyaYalovyy/rttx/issues/71)
