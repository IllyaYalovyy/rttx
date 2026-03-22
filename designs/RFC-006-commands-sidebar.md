# RFC-006: Commands & Bookmarks Right Sidebar

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Accepted                |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

A dedicated right-hand utility sidebar provides searchable access to bookmarks and saved commands.
Bookmarks set context (SSH, tmux, folder); commands execute work (run or insert literal shell
text). Both live globally — not bound to a specific session or pane — and are accessed from the
same sidebar panel. The left sidebar remains session-only navigation.

---

## Goals

- **G1** — Bookmarks (SSH, tmux, folder, combinations) are searchable and runnable from one sidebar
- **G2** — Saved commands (single-line and multiline) support Run and Insert actions
- **G3** — Left sidebar is unaffected; only session navigation lives there
- **G4** — Commands and bookmarks are globally stored, not per-session

## Non-Goals

- **NG1** — No custom placeholder or template DSL in the command body; literal shell text only
- **NG2** — No shell history import or automatic command capture
- **NG3** — No scripting engine, result capture, or command chaining in v1
- **NG4** — Session templates (composition of bookmarks + commands) are a later feature

---

## Background & Motivation

Before the right sidebar, bookmarks were accessible only through a modal dialog or a new-session
flow. There was no persistent home for reusable commands. Users had to keep common commands in
shell aliases or in external notes — outside the terminal where they are actually used.

The session list is the wrong place to add these tools. Mixing navigation objects (sessions) with
action objects (commands, bookmarks) creates a cluttered, ambiguous sidebar. The mental model
breaks down: the left side shows where you are; the right side shows what you can do.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | One-click access to SSH/tmux/folder bookmarks and saved commands from a searchable panel |
| Contributors | Commands and bookmarks are separate persistent JSON files; independently testable |
| Packagers | None |

---

## Considered Options

### Option A — Add commands/bookmarks as tabs within the left session sidebar *(reconstructed)*

**Pros**: One sidebar to learn; no layout change.
**Cons**: Conflates navigation (sessions) with action objects (commands, bookmarks). The session
list becomes a mode-switched panel with unclear state. Tab width would have to accommodate all
three concepts.

### Option B — Modal dialog launcher (keyboard shortcut → search → run) *(reconstructed)*

**Pros**: Takes no persistent screen space; keyboard-first.
**Cons**: Loses the browsability of a persistent panel. Users cannot glance at recent bookmarks or
pinned commands. A modal dialog also conflicts with the goal of keyboard-first session switching
(a modal dialog steals focus from the terminal).

### Option C — Dedicated right utility sidebar

**Pros**: Clear mental model (left = where I am, right = what I can do). Independently hideable.
Can house bookmarks, commands, and eventually templates without cramming them into the session
list. Already aligns with `adw::OverlaySplitView` which supports a second sidebar.
**Cons**: More screen real estate; second sidebar toggle needed.

---

## Decision

Chosen option: C

The dual-sidebar model provides the clearest mental model and the most room to grow. The
`adw::OverlaySplitView` already handles the overlay behavior on narrow screens. The left sidebar
remains sacred — session-only — and the right sidebar houses all reusable workflow tools.

---

## Design

### Sidebar sections

```text
Right Utility Sidebar
├── Search entry (searches all sections)
├── Bookmarks
│     ├── [search results or grouped list]
│     └── Each row: type icon + label + action button
└── Commands
      ├── [search results or pinned + all]
      └── Each row: title + preview + Run / Insert buttons
```

### Bookmark data model

```rust
struct Bookmark {
    uuid: String,
    name: String,
    ssh_target: Option<String>,    // SSH target (user@host)
    tmux_session: Option<String>,  // tmux attach target
    directory: Option<String>,     // local or remote folder
}
```

A bookmark may combine any subset of `host`, `tmux_session`, and `folder`. Execution replays
the appropriate `StartupStep` chain (see RFC-007).

### Command data model

```rust
struct SavedCommand {
    uuid: String,
    title: String,
    body: String,                        // literal shell text, multiline supported
    default_run_mode: CommandRunMode,    // Run (body + \n) or InsertOnly (body, no \n)
}
```

### Execution semantics

- **Run**: send `body + "\n"` to the active terminal's VTE PTY
- **Insert**: send `body` without newline; user presses Enter when ready
- Default for multiline commands: `InsertOnly` (avoids accidental multi-step execution)

### Search ranking

1. Exact title match
2. Title prefix match
3. Tag match
4. Body text match
5. Pinned items float to top regardless of rank

### Persistence

- `bookmarks.json` in XDG config directory (separate from `sessions.json`)
- `commands.json` in XDG config directory
- Both use `#[serde(default)]` on all optional fields for forward/backward compatibility

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Bookmarks searchable and runnable | Bookmark section in right sidebar; runs `StartupStep` chain in active pane |
| G2 — Commands with Run/Insert | `CommandRunMode` enum; explicit action buttons per row |
| G3 — Left sidebar unaffected | Right sidebar is a separate `adw::OverlaySplitView` end widget |
| G4 — Global storage | `bookmarks.json` and `commands.json` independent of session state |

---

## Development Plan

- [x] Bookmark data model and persistence
- [x] Right utility sidebar panel
- [x] Bookmarks section with search
- [x] Commands data model and persistence
- [x] Commands section with Run and Insert actions
- [x] Multiline command support
- [ ] **Pin / recent commands** — *tracked in todo.md — Commands / Templates*
- [ ] **Context menu integration** — run/insert from terminal right-click — *tracked in todo.md — Context Menu*
- [ ] **Session templates** — composition layer over bookmarks and commands — *tracked in todo.md — Commands / Templates*

---

## Open Questions

- [ ] **Q1** — Should bookmarks and commands share one search entry or have separate search entries per section? Shared search reduces UI complexity; separate entries allow section-specific filtering.

---
