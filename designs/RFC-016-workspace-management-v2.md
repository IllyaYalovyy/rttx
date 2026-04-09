# RFC-016: Workspace Management v2

| Field         | Value         |
|---------------|---------------|
| Status        | Draft         |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Rework tab creation, bookmarks, and the right sidebar to reduce friction and eliminate
confusion. Replace the single "+" button with explicit creation buttons, replace bookmarks
with Hosts and Places, scope commands and places to the active host, and remove tmux and
template support.

The one-tab-one-endpoint architecture is preserved.

---

## Goals

- **G1** — Creating any workspace type (direct, persistent, remote) takes at most 2 clicks
- **G2** — The right sidebar shows only content relevant to the active workspace's host
- **G3** — Saved paths display compactly in dropdowns without losing distinguishability
- **G4** — Connecting to a remote host with existing runtimes offers a clear attach-or-create
  choice
- **G5** — Remove dead-weight features (tmux bookmarks, templates) that add confusion

## Non-Goals

- **NG1** — Mixed-endpoint tabs (one tab with panes on different hosts)
- **NG2** — Bookmark/command management UI (deferred)
- **NG3** — Automatic host discovery (SSH config parsing, mDNS)

---

## Background & Motivation

The current workspace creation flow has three problems:

1. **The "+" button always creates local-persistent.** Ephemeral and remote are buried in the
   hamburger menu. Most users never discover them.

2. **Bookmarks conflate four concepts** (folder, SSH host, tmux session, combined) into one
   entity. Users must understand the interaction matrix to create a useful bookmark.

3. **Commands and bookmarks are global.** The same list appears whether you're on localhost or
   a remote dev box. Commands that make sense locally (`cargo build`) are useless on a remote
   host running a different project.

### What we're removing

| Feature | Reason |
|---------|--------|
| tmux bookmark type | Users can run tmux directly. Special handling adds complexity without proportional value. |
| Templates | Low usage, unclear UX. Will revisit when the new model stabilizes. |
| Combined bookmarks (SSH + folder + tmux) | Replaced by the Host + Place model which is more intuitive. |

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Faster workspace creation, less confusion, host-aware sidebar |
| Contributors | Simpler bookmark model, fewer code paths |
| Packagers    | No impact |

---

## Design

### 1. Tab creation bar

The header bar replaces the single "+" button with a row of creation controls:

```
┌──────────────────────────────────────────────────────────────────┐
│ [Direct ▾]  [Persistent ▾]  [Remote ▾]          [Tools] [≡]    │
└──────────────────────────────────────────────────────────────────┘
```

Each is a split button: clicking the button creates with defaults, clicking the dropdown
arrow shows saved places (for Direct/Persistent) or saved hosts (for Remote).

#### Direct button

- **Click**: New direct tab in `$HOME`
- **Dropdown**: List of saved local places. Selecting one creates a direct tab in that
  directory.

#### Persistent button

- **Click**: New local-persistent tab in `$HOME`
- **Dropdown**: List of saved local places. Selecting one creates a persistent tab in that
  directory.

#### Remote button

- **Click**: Opens the host list dropdown immediately (no default — you must pick a host)
- **Dropdown**: List of saved hosts. Selecting one initiates connection.

When a host is selected, the remote attach flow begins (see §3).

#### Keyboard shortcuts

| Action | Shortcut |
|--------|----------|
| New direct tab | Ctrl+Shift+T (unchanged) |
| New persistent tab | Ctrl+Shift+P |
| New remote tab | Ctrl+Shift+R |

### 2. Hosts and Places

Bookmarks are replaced by two separate concepts.

#### Hosts

A saved SSH connection target.

```rust
struct Host {
    uuid: String,
    name: String,           // display name, e.g. "dev-box"
    ssh_target: String,     // user@host or host, passed to ssh
}
```

Hosts appear in the Remote dropdown and in the right sidebar when managing connections.

#### Places

A saved directory shortcut, scoped to a host.

```rust
struct Place {
    uuid: String,
    path: String,           // absolute path, e.g. "/home/user/projects/rttx"
    host_uuid: Option<String>,  // None = local, Some(uuid) = specific host
}
```

Places appear in the Direct/Persistent dropdowns (local places) and in the right sidebar
(host-scoped places for the active workspace).

#### Global places

A place with `host_uuid: None` and a special `global: true` flag appears in the sidebar
for every host. Use case: paths like `/tmp` or `/var/log` that exist on every machine.

#### Migration

Existing bookmarks are migrated automatically:
- Bookmark with only `directory` → local Place
- Bookmark with only `ssh_target` → Host
- Bookmark with `ssh_target` + `directory` → Host + Place scoped to that host
- Bookmark with `tmux_session` → dropped (with a one-time notification)

### 3. Path contraction for dropdowns

Places are displayed in dropdowns with limited width. Full paths like
`/home/user/projects/rttx/clients/rttx/src` are too long. We need a contraction algorithm
that is both compact and unambiguous.

#### Algorithm: disambiguating fish-style contraction

Given a set of paths to display together, contract each path so that:
1. The leaf component (last segment) is always shown in full
2. Parent components are shortened to the minimum prefix that distinguishes them from
   siblings at the same level in the set
3. Home directory is replaced with `~`
4. The result fits within a maximum character width (default: 40)

**Step 1 — Tilde collapse**: Replace `$HOME` prefix with `~`.

**Step 2 — Build a trie** of all path components across all paths in the set.

**Step 3 — For each path**, walk the trie. At each level, find the shortest prefix of the
component that is unique among its siblings in the trie. If the component has no siblings,
use 1 character.

**Step 4 — Width constraint**: If the contracted path still exceeds the max width, shorten
from the left (closest to root), reducing components to 1 character each until it fits.
The leaf is never shortened.

#### Examples

Given these three local places:
```
/home/user/projects/rttx
/home/user/projects/redis
/home/user/pictures/vacation
```

After tilde collapse:
```
~/projects/rttx
~/projects/redis
~/pictures/vacation
```

Trie at level 2 (under `~`): `projects` and `pictures` are siblings.
- `projects` → `pro` (minimum to distinguish from `pictures`)
- `pictures` → `pic`

Trie at level 3 (under `projects`): `rttx` and `redis` are siblings, but both are leaves
so they stay full.

Result:
```
~/pro/rttx
~/pro/redis
~/pic/vacation
```

If we add `/home/user/projects/rttx/clients/rttx/src`:
```
~/pro/rttx
~/pro/redis
~/pic/vacation
~/pro/rttx/c/r/src
```

The inner `clients` has no siblings at that level → 1 char. Same for the inner `rttx`.

#### Edge cases

- **Single path**: No siblings anywhere → every parent gets 1 char.
  `/home/user/projects/rttx` → `~/p/rttx`
- **Root paths**: `/var/log` → `/v/log` (no tilde collapse)
- **Identical prefixes**: `/opt/app-v1/src` and `/opt/app-v2/src` →
  `app-v1` and `app-v2` need 6 chars each to disambiguate → `/o/app-v1/src` and
  `/o/app-v2/src`

### 4. Remote attach flow

When the user selects a host from the Remote dropdown, the GUI connects to the remote
daemon. The daemon may already have running runtimes (persistent workspaces from a previous
session).

#### Case A: No existing runtimes

The daemon has no running runtimes. The GUI creates a new persistent workspace on the
remote host and opens it as a new tab.

**User journey**: Remote ▾ → "dev-box" → new tab appears, shell prompt on dev-box.
**Clicks**: 2 (dropdown + host selection).

#### Case B: Existing runtimes

The daemon reports one or more running runtimes via `ListSessions`. The GUI must let the
user choose: attach to an existing runtime, or create a new one.

**UX: Inline popover on the Remote dropdown**

When the host has existing runtimes, instead of immediately creating a tab, a popover
appears below the host entry:

```
┌─────────────────────────────────┐
│  dev-box                        │
│  ───────────────────────────    │
│  ● Attach: ~/pro/rttx    (2p)  │
│  ● Attach: ~/src/redis   (1p)  │
│  ───────────────────────────    │
│  + New workspace                │
└─────────────────────────────────┘
```

Each existing runtime shows:
- Its contracted CWD (from the first pane's last known working directory)
- Pane count in parentheses (`2p` = 2 panes)

Selecting "Attach" opens that runtime as a new tab. Selecting "New workspace" creates a
fresh persistent workspace.

**User journey (attach)**: Remote ▾ → "dev-box" → popover → "Attach: ~/pro/rttx" → tab.
**Clicks**: 3.

**User journey (new)**: Remote ▾ → "dev-box" → popover → "New workspace" → tab.
**Clicks**: 3.

#### Case C: Single existing runtime

When there is exactly one runtime, the popover still appears (no auto-attach). The user
might want a new workspace, not the existing one. Auto-attaching would be surprising.

#### Remote + Place

A user can also create a remote workspace at a specific directory. This is a 2-click
operation from the right sidebar:

1. Switch to a tab connected to the target host (or create one via Remote dropdown)
2. Click a place in the right sidebar → the active pane `cd`s to that directory

This is not a single-action "remote + place" creation from the header bar. The header bar
Remote button connects to a host; the sidebar navigates within it. This separation keeps
the header bar simple and avoids a two-level dropdown (host → place) which would be
clunky.

### 5. Host-scoped right sidebar

The right sidebar auto-switches content based on the active workspace's endpoint.

#### Structure

```
┌─────────────────────┐
│  [Places] [Commands] │
│  ─────────────────── │
│  🔍 filter           │
│                      │
│  ~/pro/rttx          │
│  ~/pro/redis         │
│  ~/pic/vacation      │
│                      │
│  ── Global ────────  │
│  /tmp                │
│  /var/log            │
└─────────────────────┘
```

#### Switching logic

When the user switches tabs:
1. Determine the active workspace's endpoint: `Local` or `Remote { host }`
2. Filter places to show only those matching the endpoint's host (plus global places)
3. Filter commands to show only those matching the endpoint's host (plus global commands)

For local workspaces, show places/commands where `host_uuid` is `None`.
For remote workspaces, show places/commands where `host_uuid` matches the host.

Global entries (marked `global: true`) appear in a separate section at the bottom,
regardless of the active host.

#### Commands

Commands gain a `host_uuid` field, same as places:

```rust
struct SavedCommand {
    uuid: String,
    title: String,
    body: String,
    default_run_mode: CommandRunMode,
    host_uuid: Option<String>,  // None = local, Some(uuid) = specific host
    global: bool,               // true = show for all hosts
}
```

Existing commands are migrated as local (`host_uuid: None`).

### 6. Split pane behavior

When splitting a pane (Ctrl+Shift+E / Ctrl+Shift+O), the new pane clones the parent:
same endpoint, same working directory. This is the "clone parent" behavior — no chooser,
no extra clicks.

This is unchanged from current behavior. The "new pane chooser" idea from the earlier
discussion is deferred — it adds friction to the most common operation.

### 7. Terminology changes

| Old term | New term | Reason |
|----------|----------|--------|
| Bookmark | *(removed)* | Replaced by Host and Place |
| Bookmark (folder) | Place | Clearer: it's a directory shortcut |
| Bookmark (SSH) | Host | Clearer: it's a connection target |
| Bookmark (tmux) | *(removed)* | Feature removed |
| Bookmark (combined) | *(removed)* | Replaced by Host + Place scoping |
| Template | *(removed)* | Feature removed, revisit later |

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — 2-click creation | Split buttons: 1 click for default, 2 for place/host selection |
| G2 — Host-scoped sidebar | Auto-switch on tab change, filter by endpoint host |
| G3 — Compact paths | Disambiguating fish-style contraction with max-width constraint |
| G4 — Remote attach UX | Popover showing existing runtimes + "New workspace" option |
| G5 — Remove dead weight | tmux bookmarks and templates removed |

---

## Development Plan

- [ ] **Data model: Host and Place** — new structs, storage, migration from bookmarks
- [ ] **Path contraction** — implement the disambiguating contraction algorithm
- [ ] **Tab creation bar** — replace "+" with Direct/Persistent/Remote split buttons
- [ ] **Remote attach popover** — inventory query + attach/create choice
- [ ] **Host-scoped sidebar** — auto-switch places and commands on tab change
- [ ] **Command host scoping** — add `host_uuid` to `SavedCommand`, migrate existing
- [ ] **Remove tmux support** — drop `tmux_session` from data model and UI
- [ ] **Remove templates** — drop template UI and related code
- [ ] **Keyboard shortcuts** — Ctrl+Shift+P (persistent), Ctrl+Shift+R (remote)

---

## Open Questions

- **Q1** — Should the path contraction algorithm run on every dropdown open (recomputing
  the trie from the current place set), or precompute and cache? The place set is small
  (typically <50 entries), so recomputing is likely fine.
- **Q2** — When a remote host is unreachable, should the Remote dropdown show it grayed out
  with a tooltip, or show it normally and fail on click? Grayed out requires background
  connectivity checks which add complexity.
- **Q3** — Should places support user-defined short names (aliases) in addition to the
  auto-contracted path? This would let users name a place "rttx" instead of "~/pro/rttx".
  Deferred for now — the contraction algorithm should be sufficient.

---

## References

- [RFC-002: Adwaita Modernization & SessionRow Redesign](./RFC-002-adwaita-modernization.md)
- [RFC-013: Persistent Host Sessions](./RFC-013-persistent-host-sessions.md)
- [RFC-015: Workspace Sidebar Row Content Specification](./RFC-015-workspace-sidebar-rows.md)
- [fish shell `prompt_pwd`](https://fishshell.com/docs/current/cmds/prompt_pwd.html) — prior art for path shortening
- [spwd](https://github.com/ayosec/spwd) — prior art for max-width path contraction
