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
| Contributors | Minimal: one `ShortcutController` on `TerminalWidget`; one boolean preference |
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

A `gtk4::ShortcutController` is added to `TerminalWidget` at construction time. It runs in the
`Capture` propagation phase and `Local` scope to intercept shortcuts before VTE's default handler.
The callbacks downcast the widget to read the live preference value from `imp().smart_clipboard`,
avoiding stale-capture bugs from `Cell::clone()`.

```text
ShortcutController (Capture phase, Local scope)
  Ctrl+C pressed:
    if smart_clipboard and vte.has_selection() → copy_clipboard_format(Text) → unselect_all() → Stop
    else → Proceed (shell receives \x03)
  Ctrl+V pressed:
    if smart_clipboard → paste_clipboard() → Stop propagation
    else → Proceed (shell receives \x16)
```

Preference: `smart_clipboard: bool` in `Preferences`, default `false`.
Toggle exposed in `AdwPreferencesWindow` under the Terminal section.

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
- [x] Add `ShortcutController` to `TerminalWidget` (Capture phase, Local scope)
- [x] Implement `Ctrl+C` selection-guard copy
- [x] Implement `Ctrl+V` conditional paste
- [x] Expose toggle in `AdwPreferencesWindow`
- [x] Fix: callbacks read live preference via widget downcast, not stale `Cell::clone()`
- [x] Test: `set_smart_clipboard_takes_effect_after_construction` regression test

---
