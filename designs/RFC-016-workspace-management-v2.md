# RFC-016: Workspace Management v2

| Field         | Value         |
|---------------|---------------|
| Status        | Draft         |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Rework workspace creation, session attachment, split-pane creation, and the right sidebar to
make the product action-oriented and consistent.

The UI should lead with what the user wants to do:
- create a new daemon-backed workspace
- connect to an existing daemon-backed workspace
- fall back to a direct connection when needed

The one-workspace-one-endpoint architecture is preserved. A workspace remains a tab, and a
tab never mixes endpoints or runtime policies.

---

## Goals

- **G1** — The primary actions are explicit: `New`, `Connect to Existing`, `New Direct`
- **G2** — Local and remote hosts follow the same creation and attachment model
- **G3** — Discovering existing sessions on a host is explicit and not hidden behind side
  effects
- **G4** — The right sidebar is host-aware by default, but still lets users inspect and manage
  global and orphaned content
- **G5** — Direct mode remains available as a backup path without dominating the main UX
- **G6** — Tmux and template features are removed from this model

## Non-Goals

- **NG1** — Mixed-endpoint tabs
- **NG2** — Full command/place management UI
- **NG3** — Session takeover from another client
- **NG4** — Automatic host discovery (SSH config parsing, mDNS)

---

## Background & Motivation

The current experience is moving in the right direction, but it still feels implementation-led
instead of user-led.

### Problems in the current experience

1. **Creation is framed around backend types instead of user intent.**
   `Direct`, `Persistent`, and `Remote` are transport/runtime concepts. Users usually think in
   terms of "start something new" or "connect to what is already running."

2. **Discoverability of existing sessions is inconsistent.**
   A user should be able to explicitly ask "what sessions exist on this host?" without first
   triggering a create-or-attach side effect.

3. **Direct mode has too much visual weight.**
   It still needs to exist as a fallback path when daemon-backed flows are unavailable or
   broken, but it should not lead the product.

4. **Bookmarks are still too rigid.**
   A place or command may be useful on multiple hosts, or globally. A single-host binding is
   too narrow.

5. **The sidebar cannot gracefully represent orphaned host associations.**
   If a host is removed but items still reference it, those items should remain visible and
   manageable.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Clearer new-vs-attach flows, more consistent host model, better sidebar scoping |
| Contributors | Simpler top-level UX model, less attach/create ambiguity |
| Packagers    | No impact |

---

## Design

### 1. Top bar actions

Replace the current creation affordance with three primary actions:

```
┌────────────────────────────────────────────────────────────────────┐
│ [New ▾]  [Connect to Existing ▾]  [New Direct]   [Tools] [≡]     │
└────────────────────────────────────────────────────────────────────┘
```

These controls are action-oriented, not runtime-oriented.

#### New

Creates a new daemon-backed workspace.

- The menu contains the local host plus saved remote hosts
- Selecting a host opens the **New Workspace** dialog for that host
- The dialog is the same for local and remote

#### Connect to Existing

Attaches to an already-running daemon-backed workspace.

- The menu contains the local host plus saved remote hosts
- Selecting a host opens the **Connect to Existing** dialog for that host
- The dialog is the same for local and remote

#### New Direct

Creates a new direct workspace immediately.

This remains available as a fallback path when the daemon model is unavailable or when the
user intentionally wants a direct session. It is intentionally a secondary action, not the
main workflow.

#### Keyboard shortcuts

| Action | Shortcut |
|--------|----------|
| New workspace | Ctrl+Shift+T |
| Connect to existing | Ctrl+Shift+A |
| New direct | Ctrl+Shift+D |

### 2. Host model

Local and remote should participate in the same selection model.

```rust
struct Host {
    key: String,         // canonical endpoint key, e.g. "local" or normalized SSH target
    name: String,        // display name
    kind: HostKind,      // Local or Remote
    ssh_target: Option<String>,
}
```

#### Host identity

`key` is the canonical identity used throughout the UI and persistence.

- Local uses a reserved built-in key: `local`
- Remote hosts use a normalized endpoint key derived from the SSH target
- Matching for sessions, commands, and places is always done by host key

Saved host records are metadata layered on top of endpoint identity. If a workspace is opened
for a host that does not currently exist in the saved host list, the workspace still has a
stable host key and the UI still works.

### 3. New Workspace dialog

When the user chooses a host from **New**, the GUI opens a host-specific dialog instead of
creating immediately.

```
┌──────────────────────────────────────────────┐
│ New Workspace: dev-box                       │
│ Search: [______________________________]     │
│                                              │
│ Suggested                                    │
│  • Home                                      │
│  • Root                                      │
│                                              │
│ Saved Places                                 │
│  • ~/pro/rttx                                │
│  • ~/src/redis                               │
│  • /srv/app                                  │
└──────────────────────────────────────────────┘
```

#### Behavior

- The dialog includes search
- The list is scoped to the selected host, plus global places
- `Home` and `Root` are always present as built-in global entries
- Choosing an entry creates a new daemon-backed workspace on that host at that place
- The dialog may later support "empty/new shell" explicitly, but the initial design assumes
  that choosing `Home` or `Root` covers the common path

This flow makes "new local" and "new remote" the same user experience.

### 4. Connect to Existing dialog

When the user chooses a host from **Connect to Existing**, the GUI opens a session picker for
that host.

```
┌──────────────────────────────────────────────┐
│ Connect to Existing: dev-box                 │
│ Search: [______________________________]     │
│                                              │
│ Available                                    │
│  • ~/pro/rttx           2 panes              │
│  • ~/src/redis          1 pane               │
│                                              │
│ Busy                                         │
│  • /srv/app             Connected elsewhere  │
└──────────────────────────────────────────────┘
```

#### Behavior

- The dialog shows all daemon-backed sessions discoverable on the selected host
- Sessions connected by another client are visible but disabled
- Sessions already attached by the same client are visible but disabled
- Busy and already-open sessions must be visually distinct
- This RFC does not add takeover. That remains future work

The key product rule is explicitness: the user is consciously in a "connect to existing"
flow, not discovering existing sessions as a side effect of a create action.

### 5. Direct mode

Direct mode remains supported, but it is intentionally separated from the primary daemon-backed
flows.

#### Product position

- Direct is a backup path
- Direct is not the default
- Direct should not shape the main mental model

This keeps the feature available without letting it stand in the way of the primary experience.

### 6. Places and commands

Bookmarks are replaced with **Places**. Commands remain commands.

Both are tag-based rather than single-host-bound.

```rust
struct Place {
    uuid: String,
    path: String,
    host_tags: Vec<String>,   // empty = global
}

struct SavedCommand {
    uuid: String,
    title: String,
    body: String,
    default_run_mode: CommandRunMode,
    host_tags: Vec<String>,   // empty = global
}
```

#### Tagging rules

- `host_tags.is_empty()` means the item is global
- One or more tags means the item is scoped to those hosts
- A place or command may be tagged for any number of hosts
- Host tags are host keys, not display names

#### Built-in global places

The global place set includes these entries by default:
- `Home`
- `Root`

Additional global places and commands are user-managed content.

#### Migration

Existing bookmarks are migrated as follows:
- bookmark with only `directory` → local-tagged Place
- bookmark with only `ssh_target` → Host
- bookmark with `ssh_target` + `directory` → Place tagged with that host key
- bookmark with `tmux_session` → dropped

Existing commands migrate into the new tag model:
- local-only commands become tagged with `local`
- commands that were intended to be global may later be retagged by the user

### 7. Host-aware right sidebar

The right sidebar becomes host-aware, but not host-blind.

```
┌──────────────────────────────────────────────┐
│ Search: [______________________________]     │
│ Host: [dev-box ▾]                            │
│                                              │
│ [Places] [Commands]                          │
│                                              │
│ Host-specific                                │
│  • ~/pro/rttx                                │
│  • ~/src/redis                               │
│                                              │
│ Global                                       │
│  • Home                                      │
│  • Root                                      │
│  • /tmp                                      │
└──────────────────────────────────────────────┘
```

#### Default behavior

- When the user switches workspaces, the sidebar host selector automatically follows the
  active workspace host
- Places and commands shown in the host-specific section are those tagged with the selected
  host key
- Items with no tags appear in the global section

#### Manual override

The user may manually change the host selector in the sidebar to inspect content for a
different host without switching tabs.

#### Selector population

The sidebar host selector must not be derived only from the current saved host list.

It is populated from:
- host keys currently assigned to places or commands
- plus the active workspace host, when needed for context

This guarantees that orphaned content remains visible and manageable.

#### Orphaned tags

If a host record is deleted but items still reference that host key:

- the tag remains intact
- the selector still shows it
- the UI renders it as missing/orphaned
- cleanup is explicit, not automatic

Automatic cleanup on host deletion is worse UX because it silently removes information users
may still need to inspect or retag later.

### 8. Split pane behavior

Split-pane creation should use the same explicit model as top-level creation, but it remains
constrained to the current workspace.

```
┌──────────────────────────────┐
│ Clone parent                │
│ New shell                   │
│ Open place: Home            │
│ Open place: ~/pro/rttx      │
└──────────────────────────────┘
```

#### Split chooser rules

- **Clone parent**: same launch context and working directory as the source pane
- **New shell**: new pane in the current workspace using the workspace's default launch mode
- **Open place**: new pane in the current workspace at a host-compatible place

Because a workspace is still a single tab bound to one endpoint/runtime policy, split never
offers cross-host or cross-runtime actions.

### 9. Terminology

| Old term | New term | Reason |
|----------|----------|--------|
| Bookmark | Place | A saved navigation target |
| Bookmark (SSH) | Host | A connection target |
| Template | *(removed)* | Not part of the primary model |
| tmux bookmark | *(removed)* | Dropped |
| Persistent workspace | Workspace | Default daemon-backed workspace path |
| Direct workspace | Direct | Secondary fallback path |

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Explicit primary actions | Top bar uses `New`, `Connect to Existing`, `New Direct` |
| G2 — Same local/remote model | Both flows begin with host selection |
| G3 — Explicit session discovery | Existing sessions are reachable only through a dedicated attach flow |
| G4 — Host-aware but manageable sidebar | Default host-following selector, global section, orphan visibility |
| G5 — Direct as backup | Direct remains available but secondary |
| G6 — Remove dead weight | Tmux and templates are dropped |

---

## Development Plan

- [ ] Replace the top bar entrypoints with `New`, `Connect to Existing`, and `New Direct`
- [ ] Introduce canonical host keys for local and remote endpoints
- [ ] Add the **New Workspace** dialog with search and host-scoped place selection
- [ ] Add the **Connect to Existing** dialog with clear available/busy state
- [ ] Replace bookmark storage with Places + host-tagging
- [ ] Update commands to use host tags instead of single-host binding
- [ ] Add built-in global places: `Home` and `Root`
- [ ] Rework the right sidebar around search + host selector + tagged content
- [ ] Preserve and surface orphaned tags instead of cleaning them automatically
- [ ] Replace clone-only split with an explicit in-workspace split chooser
- [ ] Remove tmux-related data model and UI paths
- [ ] Remove template-related UI and data paths

---

## Open Questions

- **Q1** — In the **New Workspace** dialog, do we want an explicit "Empty shell" row in addition
  to `Home` and `Root`, or are those sufficient?
- **Q2** — Should the host menus in the top bar show recent hosts first, or always keep a fixed
  local-then-remote ordering?
- **Q3** — Do we want aliases for places in v2, or is path-only presentation sufficient for the
  first implementation?
- **Q4** — Should host deletion offer an explicit cleanup dialog for affected tags immediately,
  or should cleanup remain a later management action?

---

## References

- [RFC-002: Adwaita Modernization & SessionRow Redesign](./RFC-002-adwaita-modernization.md)
- [RFC-013: Persistent Host Sessions](./RFC-013-persistent-host-sessions.md)
- [RFC-015: Workspace Sidebar Row Content Specification](./RFC-015-workspace-sidebar-rows.md)
