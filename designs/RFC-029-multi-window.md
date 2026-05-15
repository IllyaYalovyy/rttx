# RFC-029: Multi-Window Support

| Field         | Value                                                       |
|---------------|-------------------------------------------------------------|
| Status        | Draft                                                       |
| Author(s)     | Illya Yalovyy                                               |
| Supersedes    | —                                                           |
| Superseded by | —                                                           |

---

## Summary

Allow multiple rttx windows within a single application instance. Users can open new windows,
move workspaces between them via drag-and-drop or menu action, and close any window without
losing workspaces — they return to the remaining window.

---

## Goals

- **G1** — Open additional windows to spread workspaces across monitors or virtual desktops
- **G2** — Move workspaces between windows without disconnecting from the daemon
- **G3** — Closing a window does not destroy its workspaces — they migrate to another window
- **G4** — State persistence: window layout (which workspaces in which window) survives restart

## Non-Goals

- **NG1** — Multiple independent application instances (separate processes)
- **NG2** — Tiling window manager integration (external WM handles window placement)
- **NG3** — Detaching a single pane into its own window (only whole workspaces move)

---

## Background & Motivation

Currently rttx is a single-window application. All workspaces live in one window's sidebar.
Users with multiple monitors must either:
- Use the OS workspace/virtual desktop feature to switch contexts
- Keep all workspaces in one crowded sidebar

Terminal multiplexers (tmux, screen) and other terminal emulators (GNOME Terminal, Tilix,
Kitty) support multiple windows. For rttx, multi-window is particularly natural because
workspaces are already independent units backed by daemon runtimes — moving them between
windows is a pure UI operation with no daemon-side impact.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Can spread work across monitors; less sidebar clutter |
| Contributors | Window management code becomes more complex; state model changes |
| Packagers    | No impact |

---

## Considered Options

### Option A — Multiple windows, shared workspace pool

All windows share a single workspace list. Each workspace has a `window_id` assignment.
The sidebar in each window shows only its assigned workspaces. Unassigned workspaces
appear in all windows (or a designated "home" window).

**Pros**: Simple mental model. Workspaces are never lost. One persistence file.
**Cons**: Need a "move to window" action. Sidebar filtering adds complexity.

### Option B — Independent windows with separate state

Each window has its own independent workspace list and state file. Windows don't know
about each other.

**Pros**: Simpler implementation. No cross-window coordination.
**Cons**: Closing a window loses its workspaces. No way to move workspaces between windows.
Violates G2 and G3.

### Option C — Multiple windows, drag-and-drop workspace transfer

Like Option A, but workspaces can be moved between windows via sidebar drag-and-drop
(drag a tab from one window's sidebar to another). The workspace pool is shared.

**Pros**: Most intuitive UX. Matches browser tab behavior.
**Cons**: GTK4 cross-window DnD is complex. Requires careful state synchronization.

---

## Decision

**Chosen option: Option A + C (shared pool with drag-and-drop)**

Rationale: The daemon-backed architecture already separates workspace state from window
state. Workspaces are logical entities; windows are presentation. The shared pool ensures
no workspace is ever lost (G3), and drag-and-drop provides the most natural transfer UX.
Menu-based "Move to Window" serves as a fallback for keyboard users.

---

## Design

### 1. Window model

```
Application (1)
  └── Windows (1..N)
        └── Assigned Workspaces (0..M)
              └── Panes, runtime bindings, etc.
```

Each window has a stable UUID. Workspaces are assigned to exactly one window at a time.
A workspace with no window assignment defaults to the "primary" window (the first one
created, or the last one standing).

### 2. Creating a new window

**Triggers:**
- Menu: "File → New Window" or app menu action
- Keyboard: `Ctrl+Shift+N`
- D-Bus: `org.gtk.Application.Activate` (standard GNOME "new window" integration)
- Dragging a workspace tab outside the current window (tear-off)

**Behavior:**
- A new `Window` instance is created with an empty sidebar
- The new window gets a new UUID
- No workspaces are assigned initially (user moves them, or creates new ones)
- The new window inherits the same daemon connection manager (shared across windows)

### 3. Moving workspaces between windows

**Via menu:**
- Right-click workspace tab → "Move to Window → [Window 2]" submenu
- Lists all other open windows by their title or number

**Via drag-and-drop:**
- Drag a workspace tab from the sidebar
- Drop on another window's sidebar (or anywhere in the window)
- The workspace disappears from the source sidebar and appears in the target

**Via keyboard:**
- `Ctrl+Shift+M` → opens a "Move to Window" picker (if multiple windows exist)

**What happens on move:**
- The workspace's `window_id` is updated in the shared state
- The source window removes the workspace from its sidebar and session stack
- The target window adds it to its sidebar and builds the session UI
- The daemon connection is NOT affected (it's shared at the application level)
- Terminal widgets are destroyed in the source and recreated in the target
  (VTE widgets cannot be reparented across windows)
- The pane content is restored from the daemon snapshot (seamless for the user)

### 4. Closing a window

**Rules:**
- Closing the **last** window quits the application (existing behavior)
- Closing a **non-last** window:
  - All workspaces assigned to it are reassigned to the primary window
  - The workspaces appear in the primary window's sidebar
  - No daemon disconnection occurs
  - A toast in the primary window: "3 workspaces moved from closed window"

**Edge case — closing the primary window when others exist:**
- The next window in creation order becomes the new primary
- Workspaces from the closed window move to the new primary

### 5. State persistence

The `workspaces.json` store gains a `window_id` field per workspace:

```json
{
  "workspaces": [
    {
      "id": "ws-1",
      "window_id": "win-abc",
      "name": "rttx",
      ...
    },
    {
      "id": "ws-2",
      "window_id": "win-def",
      "name": "Knowledge",
      ...
    }
  ],
  "windows": [
    { "id": "win-abc", "width": 1920, "height": 1080, "is_maximized": true },
    { "id": "win-def", "width": 1200, "height": 800, "is_maximized": false }
  ]
}
```

On startup:
- Create one window per unique `window_id` in the persisted state
- Assign workspaces to their respective windows
- If a `window_id` references a window that doesn't exist in the `windows` array,
  assign to the primary window (graceful degradation)

### 6. Connection manager sharing

The `ConnectionManager` (daemon bridge) is owned by the `Application`, not by individual
windows. All windows share the same daemon connections. When a workspace moves between
windows, its runtime binding stays intact — only the UI representation changes.

### 7. Sidebar behavior

Each window's sidebar shows only its assigned workspaces. The sidebar actions
(new workspace, connect, close) operate within the window's scope. "New Workspace"
creates a workspace assigned to the current window.

### 8. Window identification

Windows are identified in the UI by:
- A number (Window 1, Window 2, ...) in the title bar
- Or by the name of their first workspace (e.g., "rttx — Window 2")

The "Move to Window" menu shows these identifiers.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | "New Window" action creates additional windows |
| G2   | Drag-and-drop + menu action moves workspaces without daemon impact |
| G3   | Closing a non-last window migrates workspaces to the primary window |
| G4   | `window_id` per workspace + `windows` array in persistence |

---

## Development Plan

- [ ] **Step 1** — Add `window_id` to `WorkspaceRecord` persistence model (`#[serde(default)]`)
- [ ] **Step 2** — Add `windows` array to `WorkspaceStore` persistence model
- [ ] **Step 3** — Refactor `ConnectionManager` ownership from `Window` to `Application`
- [ ] **Step 4** — Support multiple `Window` instances in `Application::activate`
- [ ] **Step 5** — "New Window" action (`Ctrl+Shift+N`) + D-Bus activation
- [ ] **Step 6** — "Move to Window" context menu action
- [ ] **Step 7** — Close-window workspace migration logic
- [ ] **Step 8** — Multi-window state persistence (save/restore window assignments)
- [ ] **Step 9** — Sidebar drag-and-drop workspace transfer between windows
- [ ] **Step 10** — Tab tear-off (drag outside window creates new window)

---

## Open Questions

- [ ] **Q1** — Should "New Window" start empty or offer to move the current workspace?
  Leaning toward empty — the user explicitly moves what they want.
- [ ] **Q2** — Should windows remember their monitor/position, or let the WM handle it?
  Leaning toward WM handles it — GTK4 on Wayland cannot set window position anyway.
- [ ] **Q3** — When a workspace is moved, should the terminal content be preserved visually
  (brief flash) or is a full re-render from daemon snapshot acceptable?
  Leaning toward re-render — VTE widgets can't be reparented, and the snapshot restore
  is fast enough to be imperceptible.

---

## References

- [#77 — Integration: Add New Window action](https://github.com/IllyaYalovyy/rttx/issues/77)
- [#33 — Sidebar: Detach tab into a new window](https://github.com/IllyaYalovyy/rttx/issues/33)
- [GNOME Terminal multi-window](https://help.gnome.org/users/gnome-terminal/stable/) — prior art
- [GTK4 Application window management](https://docs.gtk.org/gtk4/class.Application.html)
