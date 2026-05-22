# RFC-030: One-Off Command Execution

| Field         | Value                                                       |
|---------------|-------------------------------------------------------------|
| Status        | Draft                                                       |
| Author(s)     | Illya Yalovyy                                               |
| Supersedes    | —                                                           |
| Superseded by | —                                                           |

---

## Summary

Add a "one-off" execution mode for saved commands that runs them in a transient overlay
pane without affecting workspace layout, scrollback history, or pane state. Designed for
credential refreshes, package installs, and other utility commands that need user interaction
but shouldn't pollute the working environment.

---

## Goals

- **G1** — Execute utility commands without altering workspace layout (no new splits, no tab changes)
- **G2** — Command output and history do not persist in any pane's scrollback
- **G3** — Support interactive commands (user input required — not background execution)
- **G4** — Integrate with the existing Commands sidebar (mark commands as "one-off")
- **G5** — Minimal friction: one click or keyboard shortcut to run

## Non-Goals

- **NG1** — Background/headless execution (commands may need passwords, confirmations)
- **NG2** — Parallel execution of multiple one-offs simultaneously
- **NG3** — Persisting one-off output for later review (it's intentionally ephemeral)
- **NG4** — Replacing the existing "Run" / "Run in new pane" modes

---

## Background & Motivation

Users frequently run utility commands that are unrelated to their current work:

- `mwinit -s` (refresh corporate credentials)
- `ada credentials update ...` (AWS credential rotation)
- `sudo apt update && sudo apt upgrade` (system updates)
- `ssh-add ~/.ssh/id_rsa` (add SSH key, requires passphrase)
- `kinit` (Kerberos ticket refresh)

Currently these commands must be run in an existing pane (polluting its history and CWD)
or in a new split/pane (cluttering the layout). After the command finishes, the user must
manually close the extra pane or live with the noise in their scrollback.

The "one-off" mode solves this by providing a transient execution surface that appears,
runs the command, allows interaction, and disappears — leaving no trace in the workspace.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Utility commands no longer clutter workspaces; one-click credential refresh |
| Contributors | New overlay pane widget; new command execution mode |
| Packagers    | No impact |

---

## Considered Options

### Option A — Overlay pane (modal, in-window)

A VTE terminal appears as an overlay on top of the current workspace content (like a
dropdown terminal or a dialog). It runs the command, accepts input, and closes when the
command exits (or the user presses a key to dismiss).

**Pros**: Doesn't affect layout. Visually distinct from workspace panes. Can be dismissed
easily. Feels like a "quick action" rather than a workspace change.
**Cons**: Blocks interaction with the workspace underneath while open. Requires a new
overlay widget.

### Option B — Dedicated "scratch" pane in the sidebar

A permanent hidden pane that one-off commands run in. It's always there but not visible
in the layout. User can toggle it open/closed.

**Pros**: No new widget needed — reuse existing pane infrastructure.
**Cons**: Still persists in the session. History accumulates. Doesn't feel "transient."

### Option C — System notification with embedded terminal

Run the command and show output in a desktop notification or a small floating window.

**Pros**: Completely separate from the workspace.
**Cons**: GTK4 doesn't support terminal widgets in notifications. Floating windows are
a separate feature (RFC-029). Over-engineered for the use case.

---

## Decision

**Chosen option: Option A — Overlay pane**

Rationale: The overlay model perfectly matches the mental model of "quick utility action
that doesn't belong to my workspace." It's visually distinct, inherently transient, and
blocks only while the command needs attention — which is exactly when the user is focused
on it anyway.

---

## Design

### 1. Command property: `one_off`

Add a boolean field to `SavedCommand`:

```rust
#[serde(default)]
pub one_off: bool,
```

When `one_off` is true:
- The command sidebar shows a distinct icon/badge (e.g., ⚡ or "1×")
- Clicking the command opens the overlay pane instead of running in the current pane
- The command is NOT added to pane recovery recipes

### 2. Overlay pane widget

A new `OverlayPane` widget that:
- Renders as a centered panel covering ~80% of the workspace area
- Has a semi-transparent backdrop (dims the workspace behind it)
- Contains a VTE terminal (same as persistent pane, but ephemeral)
- Shows the command title in a header bar
- Has a "Close" button (enabled after command exits) and "Kill" button (while running)
- Auto-closes 2 seconds after the command exits with status 0 (configurable)
- Stays open on non-zero exit (so the user can see the error)

Visual layout:
```
┌─────────────────────────────────────────────┐
│ ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ │
│ ░░┌─────────────────────────────────────┐░░ │
│ ░░│ ⚡ mwinit                     [✕]  │░░ │
│ ░░├─────────────────────────────────────┤░░ │
│ ░░│                                     │░░ │
│ ░░│  $ mwinit -s                        │░░ │
│ ░░│  Enter PIN: _                       │░░ │
│ ░░│                                     │░░ │
│ ░░│                                     │░░ │
│ ░░└─────────────────────────────────────┘░░ │
│ ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ │
└─────────────────────────────────────────────┘
```

### 3. Execution flow

1. User clicks a one-off command in the sidebar (or triggers via leader key)
2. If the command has parameters, the parameter dialog appears first (existing flow)
3. After parameters are resolved, the overlay pane opens
4. A new shell is spawned in the overlay's VTE with the command's CWD
5. The command text is sent as input (same as existing "Run" mode)
6. User interacts with the terminal (enters passwords, confirms prompts)
7. When the shell exits:
   - Exit 0: overlay shows "✓ Done" for 2 seconds, then auto-closes
   - Non-zero: overlay shows "✗ Failed (exit N)" and stays open until dismissed
8. User can press Escape or click Close at any time to kill and dismiss

### 4. Daemon interaction

The overlay pane uses a **`no_persist` pane** on the daemon:
- `CreatePane { no_persist: true }` — the daemon won't save scrollback or screen snapshots
- The pane is created in a dedicated ephemeral runtime (or the workspace's existing runtime)
- When the overlay closes, `TerminatePane` is sent — the pane is destroyed immediately
- No trace remains in the daemon's persisted state

### 5. Keyboard interaction

- **While overlay is open**: all keyboard input goes to the overlay VTE (not the workspace)
- **Escape**: if command is running → kill it; if command exited → close overlay
- **Ctrl+C**: forwarded to the command (standard terminal behavior)
- The workspace underneath is non-interactive while the overlay is open

### 6. Commands sidebar integration

In the command form editor:
- New toggle: "One-off execution" (checkbox or switch)
- When enabled, the "Default action" dropdown is hidden (one-offs always use overlay mode)
- The sidebar shows a distinct visual indicator for one-off commands

Execution from sidebar:
- One-off commands: single click opens overlay directly
- Regular commands: existing behavior (run in current pane, insert, or new pane)

### 7. CWD for one-off commands

The overlay shell starts in:
1. The command's explicit CWD (if configured in the command record)
2. Otherwise: the active pane's current CWD
3. Otherwise: `$HOME`

This ensures commands like `ada credentials update` work regardless of which workspace
is active.

### 8. Multiple one-offs

Only one overlay can be open at a time (per window). If the user triggers another one-off
while one is running:
- Show a toast: "A one-off command is already running"
- Do not queue or stack overlays

### 9. Persistence model

Add to `SavedCommand`:
```rust
#[serde(default, skip_serializing_if = "is_false")]
pub one_off: bool,
```

Add to `CommandRecord` (library.json):
```rust
#[serde(default, skip_serializing_if = "is_false")]
pub one_off: bool,
```

No changes to workspace state — one-off panes are never persisted.

### 10. Auto-close behavior

| Exit status | Behavior |
|---|---|
| 0 | Show "✓ Done" badge, auto-close after 2s |
| Non-zero | Show "✗ Failed (exit N)" badge, stay open |
| Killed by user | Close immediately |

The 2-second delay on success gives the user a moment to see the output (e.g., "Credentials
updated successfully") before the overlay disappears. If they want to read more, they can
click the overlay to cancel the auto-close timer.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | Overlay pane doesn't modify workspace layout — it floats on top |
| G2   | `no_persist` pane + immediate termination = no history trace |
| G3   | Full VTE terminal in overlay — supports passwords, confirmations, interactive prompts |
| G4   | `one_off` field on SavedCommand, distinct sidebar badge, single-click execution |
| G5   | One click from sidebar, or leader key shortcut — no dialogs unless parameters needed |

---

## Development Plan

- [ ] **Step 1** — Add `one_off` field to `SavedCommand` and `CommandRecord` with serde compat
- [ ] **Step 2** — Sidebar: show distinct badge for one-off commands, route click to overlay
- [ ] **Step 3** — `OverlayPane` widget: VTE in a centered overlay with backdrop
- [ ] **Step 4** — Overlay lifecycle: spawn `no_persist` pane, send command, handle exit
- [ ] **Step 5** — Auto-close on success, stay-open on failure, Escape to dismiss
- [ ] **Step 6** — Parameter dialog integration (show params before opening overlay)
- [ ] **Step 7** — Keyboard routing: overlay captures all input while open
- [ ] **Step 8** — Command form: add "One-off" toggle, hide run mode when enabled

---

## Open Questions

- [ ] **Q1** — Should the auto-close delay be configurable per-command, or is 2s always right?
  Leaning toward fixed 2s — simplicity over configuration.
- [ ] **Q2** — Should one-off commands be runnable from the leader key shortcut system?
  Yes — leader keys should work for one-offs too, opening the overlay directly.
- [ ] **Q3** — Should the overlay support resizing (drag edges) or is fixed 80% sufficient?
  Leaning toward fixed — it's a transient surface, not a workspace pane.

---

## References

- [RFC-025 — Commands UX v2](./RFC-025-commands-ux-v2.md)
- [#796 — Command palette for saved commands](https://github.com/IllyaYalovyy/rttx/issues/796)
- Guake/Yakuake dropdown terminal — prior art for overlay terminals
- VS Code "Run Task" — prior art for transient command execution
