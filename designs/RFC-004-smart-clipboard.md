# RFC-004: Smart Clipboard (Opt-in Ctrl+C / Ctrl+V)

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

An opt-in mode that intercepts `Ctrl+C` and `Ctrl+V` in the terminal and routes them to clipboard
operations when contextually appropriate: `Ctrl+C` copies when text is selected; `Ctrl+V` pastes
from clipboard. When the context is not right (no selection, feature disabled), the keystrokes
pass through to the shell unchanged. Disabled by default to preserve standard terminal behavior.

---

## Goals

- **G1** — `Ctrl+C` copies selected text without sending SIGINT when a VTE selection exists and the feature is enabled
- **G2** — `Ctrl+V` pastes from clipboard without sending `\x16` (quoted-insert) when the feature is enabled
- **G3** — Feature is opt-in; default behavior is unchanged standard terminal

## Non-Goals

- **NG1** — No detection of TUI alternate-screen mode; VTE selection state is the only signal used
- **NG2** — No smart detection of "dangerous" paste content (that is a separate paste-protection feature)
- **NG3** — Does not replace `Ctrl+Shift+C` / `Ctrl+Shift+V` (those remain always available)

---

## Background & Motivation

Standard terminal emulators reserve `Ctrl+C` for SIGINT and `Ctrl+V` for quoted-insert. Users
coming from IDEs or Windows-style applications expect these keys to perform clipboard operations.
The clash causes frequent accidental SIGINTs when a user meant to copy. The solution is
context-aware interception: if VTE has selected text, copy it; otherwise let the shell see the
keystroke.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Optional: `Ctrl+C` copies selected text; `Ctrl+V` pastes — matches IDE conventions |
| Contributors | Minimal: one `EventControllerKey` per terminal widget; one boolean preference |
| Packagers | None |

---

## Considered Options

### Option A — Always intercept `Ctrl+C` and `Ctrl+V` *(reconstructed)*

**Pros**: Consistent behavior, easy to document.
**Cons**: Breaks every use of `Ctrl+C` for SIGINT and `Ctrl+V` for quoted-insert. Non-starter for
users who run shells and TUI applications.

### Option B — Context-aware interception, opt-in

Both `Ctrl+C` and `Ctrl+V` are gated by a single `smart_clipboard` preference. Within that gate,
`Ctrl+C` only copies when `vte.has_selection()` is true, making it safe — pressing `Ctrl+C`
without a selection still sends SIGINT as normal.

**Pros**: Standard terminal behavior is the default. Opt-in users get IDE-like convenience without
any shell breakage for SIGINT paths (selection is never set when pressing `Ctrl+C` to kill a
process).
**Cons**: `Ctrl+V` is always intercepted when enabled, even if the user intended quoted-insert.
Users who rely on quoted-insert should leave the feature disabled.

### Option C — Do not implement; keep `Ctrl+Shift+C/V` only *(reconstructed)*

**Pros**: Zero complexity, no edge cases.
**Cons**: A recurring pain point for users coming from GUI applications. The feature is
straightforward to implement correctly.

---

## Decision

Chosen option: B

Both keys require the `smart_clipboard` preference to be on. The selection guard on `Ctrl+C` is
the safety mechanism that makes the feature accident-free in practice — you never have a selection
when pressing `Ctrl+C` to kill a process — but it is not a reason to bypass the opt-in. A single
preference governs both keys.

---

## Design

Key interception is handled by a centralized `terminal_key_action()` function that determines the
correct action for any key event based on the terminal backend type, modifier state, selection
state, and smart clipboard preference. This function is shared by both direct (`TerminalWidget`)
and managed (`PersistentPaneView`) terminal paths.

Modifier normalization strips non-shortcut modifiers (pointer buttons, lock keys) before matching,
so `Ctrl+C` with Caps Lock or a mouse button held down still triggers the clipboard action.

### Direct terminals (`TerminalWidget`)

A `gtk4::EventControllerKey` is added to the VTE widget at construction time. It runs in the
`Capture` propagation phase to intercept key events before VTE's default handler. The callback
upgrades a weak reference to the widget to read the live `smart_clipboard` preference value,
then delegates to `terminal_key_action()`.

### Managed terminals (`PersistentPaneView`)

The managed terminal's input `EventControllerKey` (which already intercepts all keys to forward
them to the daemon PTY) calls the same `terminal_key_action()` function. Smart clipboard actions
are handled identically; other keys are encoded and forwarded to the daemon.

### Key action logic

```text
terminal_key_action(backend, key, modifiers, has_selection, smart_clipboard_enabled):
  normalized = strip non-shortcut modifiers

  if smart_clipboard_enabled and normalized == Ctrl:
    Ctrl+C with selection → CopySelection
    Ctrl+V              → PasteClipboard

  (managed backend only) if normalized == Ctrl+Shift:
    Ctrl+Shift+C with selection → CopySelection
    Ctrl+Shift+V               → PasteClipboard

  ... remaining key handling per backend
```

When `CopySelection` fires: `vte.copy_clipboard_format(Text)` then `vte.unselect_all()`.
When `PasteClipboard` fires: activates `win.paste` action on the parent window.

### Preference

`smart_clipboard: bool` in `Preferences`, default `false`. Stored in the user's preferences JSON
file with `#[serde(default)]` for backward compatibility.

Toggle exposed in `AdwPreferencesWindow` as an `adw::SwitchRow` titled "Smart Ctrl+C / Ctrl+V"
in the Terminal section.

---

## Implementation Snapshot

### Source locations

| Component | File |
| --- | --- |
| Key action logic | `clients/rttx/src/terminal/mod.rs` — `smart_clipboard_action()`, `terminal_key_action()` |
| Modifier normalization | `clients/rttx/src/terminal/mod.rs` — `normalized_shortcut_modifiers()` |
| Direct terminal controller | `clients/rttx/src/terminal/widget.rs` — `EventControllerKey` in `constructed()` |
| Managed terminal controller | `clients/rttx/src/terminal/persistent_widget.rs` — `connect_input()` |
| Preference storage | `clients/rttx/src/preferences.rs` — `smart_clipboard: bool` |
| Preference UI | `clients/rttx/src/preferences_window.rs` — `SwitchRow` |
| Preference application | `clients/rttx/src/window/input.rs` — `apply_preferences_to_terminal()`, `apply_preferences_to_persistent_pane()` |

### Test coverage

| Test | Layer | What it verifies |
| --- | --- | --- |
| `smart_clipboard_only_copies_selected_ctrl_c` | Unit | Ctrl+C copies only when selection exists and feature is enabled |
| `smart_clipboard_paste_requires_plain_ctrl_v_and_opt_in` | Unit | Ctrl+V requires opt-in, rejects Ctrl+Shift+V and disabled state |
| `smart_clipboard_ignores_extra_non_shortcut_modifiers_for_ctrl_shortcuts` | Unit | Lock keys and pointer buttons do not block clipboard shortcuts |
| `smart_clipboard_key_controller_ignores_extra_non_shortcut_modifiers` | GTK widget | Live EventControllerKey handles modifier normalization |
| `smart_clipboard_preference_reaches_live_terminals` | GTK widget | Preference toggle propagates to live terminal widgets |
| `direct_and_managed_share_clipboard_policy` | Unit | Both backends produce the same clipboard action for the same inputs |
| `managed_ctrl_v_prefers_clipboard_paste_over_shell_syn` | Unit | Managed backend routes Ctrl+V to clipboard, not PTY |

### Resolved issues

- **#169** — Managed pane key input swallows clipboard shortcuts
- **#175** — Ctrl+V smart clipboard breaks when lock modifiers are present
- **#180** — Ctrl+V can fail when GTK reports extra lock modifiers
- **#233** — Managed workspaces: Ctrl+V does not paste clipboard text

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Ctrl+C copies selection | `smart_clipboard` guard + `has_selection()` guard; `copy_clipboard_format(Text)` |
| G2 — Ctrl+V pastes | `paste_clipboard()` when `prefs.smart_clipboard` is true |
| G3 — Opt-in, default off | `smart_clipboard: bool` default `false`; standard behavior unchanged |

---

## Development Plan

- [x] Add `smart_clipboard: bool` to `Preferences`
- [x] Add `EventControllerKey` to `TerminalWidget` (Capture phase)
- [x] Implement `Ctrl+C` selection-guard copy
- [x] Implement `Ctrl+V` conditional paste
- [x] Expose toggle in `AdwPreferencesWindow` as `SwitchRow`
- [x] Fix: callbacks read live preference via weak reference upgrade, not stale `Cell::clone()`
- [x] Centralize key action logic in `terminal_key_action()` shared by both backends
- [x] Add smart clipboard support to managed terminals (`PersistentPaneView`)
- [x] Normalize modifiers to ignore lock keys and pointer buttons (#175, #180)
- [x] Unit tests for key action logic (copy, paste, modifier normalization)
- [x] GTK widget test for live preference propagation
- [x] GTK widget test for EventControllerKey modifier handling

---
