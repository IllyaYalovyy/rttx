# RFC-006: Places & Commands Right Sidebar

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

A dedicated right-hand utility sidebar provides searchable access to places and saved commands,
scoped by host. Places set context (directory on a local or remote host); commands execute work
(run or insert literal shell text). Both support host tags for scoping — global when untagged,
host-specific when tagged. The left sidebar remains workspace-only navigation.

---

## Goals

- **G1** — Places (directory paths on local or remote hosts) are searchable and runnable from one sidebar
- **G2** — Saved commands (single-line and multiline) support Run and Insert actions
- **G3** — Left sidebar is unaffected; only workspace navigation lives there
- **G4** — Commands and places are globally stored, not per-workspace, with optional host scoping

## Non-Goals

- **NG1** — No custom placeholder or template DSL in the command body; literal shell text only
- **NG2** — No shell history import or automatic command capture
- **NG3** — No scripting engine, result capture, or command chaining
- **NG4** — Session templates were considered and explicitly dropped (see RFC-016)

---

## Background & Motivation

Before the right sidebar, bookmarks were accessible only through a modal dialog or a new-session
flow. There was no persistent home for reusable commands. Users had to keep common commands in
shell aliases or in external notes — outside the terminal where they are actually used.

The workspace list is the wrong place to add these tools. Mixing navigation objects (workspaces)
with action objects (commands, places) creates a cluttered, ambiguous sidebar. The mental model
breaks down: the left side shows where you are; the right side shows what you can do.

The original design used "bookmarks" that combined SSH targets, tmux sessions, and directories
into a single object. This was replaced by a simpler model: places are directory paths, hosts
are managed separately, and the host system (RFC-013) handles SSH connectivity at a higher level.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | One-click access to directory places and saved commands from a searchable, host-scoped panel |
| Contributors | Places, commands, and hosts are separate persistent JSON files; independently testable |
| Packagers | None |

---

## Considered Options

### Option A — Add commands/bookmarks as tabs within the left workspace sidebar *(rejected)*

**Pros**: One sidebar to learn; no layout change.
**Cons**: Conflates navigation (workspaces) with action objects (commands, places). The workspace
list becomes a mode-switched panel with unclear state.

### Option B — Modal dialog launcher (keyboard shortcut → search → run) *(rejected)*

**Pros**: Takes no persistent screen space; keyboard-first.
**Cons**: Loses the browsability of a persistent panel. Users cannot glance at recent places or
pinned commands. A modal dialog also steals focus from the terminal.

### Option C — Dedicated right utility sidebar *(chosen)*

**Pros**: Clear mental model (left = where I am, right = what I can do). Independently hideable.
Can house places, commands, and host-scoped content without cramming them into the workspace
list.
**Cons**: More screen real estate; second sidebar toggle needed.

---

## Decision

Chosen option: C

The dual-sidebar model provides the clearest mental model and the most room to grow. The left
sidebar remains sacred — workspace-only — and the right sidebar houses all reusable workflow tools.

---

## Design

### Sidebar layout

```text
Right Utility Sidebar (width_request=320)
├── Host selector dropdown + Add/Delete host buttons
├── Search entry (filters both tabs)
├── StackSwitcher with two tabs:
│   ├── "Places" tab
│   │   ├── Section headers (host name or "Global") in All Hosts view
│   │   ├── Built-in places: Home (~), Root (/)
│   │   └── User places: ActionRow with name + path subtitle
│   └── "Commands" tab
│       ├── Section headers (host name or "Global") in All Hosts view
│       └── Command rows: ActionRow with title + preview subtitle
```

The host selector at the top filters both tabs. Selecting a specific host shows only items
tagged for that host plus global (untagged) items. The "All Hosts" view groups items by host
with section headers.

### Host data model

```rust
pub const LOCAL_KEY: &str = "local";

pub enum HostKind { Local, Remote }

pub struct Host {
    pub key: String,
    pub name: String,
    pub kind: HostKind,
    pub ssh_target: Option<String>,
}
```

Hosts are managed separately from places and commands. The built-in local host is not persisted.
Remote hosts are identified by a normalized SSH key (hostname, lowercased, without user prefix).
Host deletion cascades to associated places and commands via a confirmation dialog.

### Place data model

```rust
pub struct Place {
    pub uuid: String,
    pub name: String,
    pub path: String,
    pub host_tags: Vec<String>,  // empty = global
}
```

A place is a directory path. Built-in places (`Home` and `Root`) have stable UUIDs
(`builtin:home`, `builtin:root`) and are not persisted. Host tags scope a place to specific
hosts; an empty tag list means the place is global.

### Command data model

```rust
pub enum CommandRunMode { Run, Insert }

pub struct SavedCommand {
    pub uuid: String,
    pub title: String,
    pub body: String,
    pub default_run_mode: CommandRunMode,
    pub host_tags: Vec<String>,  // empty = global
}
```

### Execution semantics

- **Run**: send `body + "\n"` to the active terminal's PTY
- **Insert**: send `body` without newline; user presses Enter when ready
- Default run mode for new commands: `Run`
- **Place click**: send `cd <path>\n` to the active pane

### Search

A single shared search entry filters both the Places and Commands tabs simultaneously.
Matching is case-insensitive substring search across title, body/path, and host tags.
Empty or whitespace-only queries match everything.

### Persistence

- `places.json` in XDG config directory
- `commands.json` in XDG config directory
- `hosts.json` in XDG config directory
- All use `#[serde(default)]` on newer fields for forward/backward compatibility
- Pretty-printed JSON arrays
- Missing file on load returns empty list (graceful degradation)

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Places searchable and runnable | Places tab in right sidebar; click sends `cd` to active pane |
| G2 — Commands with Run/Insert | `CommandRunMode` enum; explicit action buttons per row |
| G3 — Left sidebar unaffected | Right sidebar is a separate panel with its own toggle |
| G4 — Global storage with host scoping | `places.json`, `commands.json`, and `hosts.json` independent of workspace state; host tags for scoping |

---

## Development Plan

- [x] Place data model and persistence (`places.json`)
- [x] Host data model and persistence (`hosts.json`)
- [x] Right utility sidebar panel with tab layout
- [x] Places tab with search and host filtering
- [x] Commands data model and persistence (`commands.json`)
- [x] Commands tab with Run and Insert actions
- [x] Multiline command support
- [x] Host selector dropdown with add/delete
- [x] Host tag scoping for places and commands
- [x] Drag-and-drop command reorder
- [x] Host deletion with cascading cleanup dialog
- [x] Unified search across both tabs
- [ ] **Pin / recent commands** — tracked in [#44](https://github.com/IllyaYalovyy/rttx/issues/44)

---

## Resolved Questions

- **Q1** — Bookmarks and commands (now places and commands) share one search entry. Shared search
  reduces UI complexity and was chosen over separate per-section entries.

---
