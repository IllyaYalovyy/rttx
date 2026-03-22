# RFC-007: Per-Pane Recovery Recipes & Smart Session Restoration

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Accepted                |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

rttx persists more than layout geometry. Each pane carries a recovery recipe — a structured
description of how the pane was created and what it was for. On restart, rttx replays these
recipes to reconstruct working context: local folders, SSH connections, tmux sessions, or any
combination. The goal is honest recovery, not fake state serialization.

---

## Goals

- **G1** — Restarting the app restores the split layout, active session, and active pane
- **G2** — Each pane replays its startup recipe (local cd, SSH, tmux attach) on restore
- **G3** — Pane origin is tracked (empty shell, bookmark, command) and survives restart
- **G4** — Recovery is honest: rttx only promises what it can actually deliver

## Non-Goals

- **NG1** — In-memory shell state (variables, partially typed commands, foreground TUI state) is not preserved
- **NG2** — PTY processes are not kept alive between rttx restarts (that is tmux's job)
- **NG3** — CRIU / process snapshotting is not used; it is fragile and incompatible with networked processes
- **NG4** — Shell integration (OSC-based cwd reporting, prompt boundaries) is a later phase

---

## Background & Motivation

Most terminal emulators treat session restore as a UI concept: same number of splits, same
geometry, fresh shells. For a developer who was connected to a production server via SSH inside
a tmux session, a fresh shell is not a restore — it is an empty starting point that requires
minutes of manual reconnection.

rttx can do better without becoming a process supervisor. The key insight is that what matters
is not the exact in-memory state of the shell, but the path the user took to get there. That
path — the startup recipe — is small, serializable, and replayable.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Restarting rttx restores working context; SSH/tmux panes reconnect automatically (roadmap) |
| Contributors | Recovery data is part of `LayoutNode::Terminal`; recipe types in `session/layout.rs` |
| Packagers | No change; recovery state is stored in the existing `sessions.json` |

---

## Considered Options

### Option A — Shell integration only *(reconstructed)*

Use OSC escape sequences from the shell to report cwd, prompt boundaries, and command
boundaries. Store these in the session state.

**Pros**: Non-invasive; improves cwd accuracy; enables semantic scrollback.
**Cons**: Does not recover SSH connections or tmux sessions. Requires shell configuration by
the user. Provides metadata, not recovery.

### Option B — Per-pane recovery recipes

Store a structured `startup_chain: Vec<StartupStep>` in each pane's `LayoutNode::Terminal`
data. On restore, replay the chain: `cd`, `ssh`, `tmux attach`, `SendCommand`.

**Pros**: Works without shell integration. Covers the most valuable recovery paths (local cd,
SSH, tmux). Honest: the recipe describes exactly what will be replayed, no magic.
**Cons**: Cannot recover arbitrary shell state. Recipe must be written at pane creation time,
not after the fact.

### Option C — PTY daemon / client-server architecture *(reconstructed)*

A persistent background process owns the PTYs. The GTK UI connects and disconnects without
killing the shells.

**Pros**: True process persistence; closest to "as if you never closed the terminal."
**Cons**: Major architectural change. Significant complexity. Out of scope until recipe-based
recovery proves its value.

### Option D — CRIU process snapshotting *(reconstructed)*

Freeze and restore arbitrary process trees at the OS level.

**Pros**: Theoretically complete state recovery.
**Cons**: Fragile with interactive/networked processes. Not suitable for SSH sessions or TUI
applications. Incompatible with the rock-solid stability goal.

---

## Decision

Chosen option: B, with A as a later enhancement and C as a possible endgame

Recovery recipes are the right v1 abstraction. They are serializable, testable, and honest about
what they can and cannot restore. Shell integration (Option A) adds accuracy on top of recipes
and is planned for a later phase. The PTY daemon (Option C) is only worth building after recipe
recovery proves its value to users.

---

## Design

### Data model

```rust
pub struct PaneRecovery {
    pub source: PaneSource,
    pub startup_chain: Vec<StartupStep>,
}

pub enum PaneSource {
    EmptyShell,
    Bookmark(String),   // bookmark UUID
    Command(String),    // saved command UUID
    SessionTemplate(String),
    Manual,
}

pub enum StartupStep {
    SendText { text: String, execute: bool },
    // Roadmap: LocalCd { path }, Ssh { target }, TmuxAttach { session }, TmuxAttachOrCreate { session }
}
```

`PaneRecovery` is stored in `SessionState.terminal_recovery: BTreeMap<String, PaneRecovery>`,
keyed by terminal UUID. It is separate from `LayoutNode::Terminal` to allow the layout tree to
remain a pure geometry structure. Both structures serialize with `#[serde(default)]` so old
`sessions.json` files without recovery data continue to work.

### Recovery levels

| Level | What is restored | Status |
| --- | --- | --- |
| L1 — UI | Layout, splits, active session, active pane | Implemented |
| L2 — Context | CWD, custom title, pane origin, startup recipe replay | Implemented |
| L3 — Reconnect | SSH reconnect, tmux reattach, `ssh → tmux → folder` chains | Roadmap |
| L4 — True persistence | PTY processes kept alive; UI reconnects to live shells | Future |

### Replay on restore

On startup, for each terminal UUID with a non-empty `startup_chain` in `terminal_recovery`:

1. Create the terminal widget and spawn the shell
2. Wait for the shell prompt (via a short settle delay or shell integration marker)
3. Send each `StartupStep` to the VTE PTY in order

For `SendText { text, execute: true }`: send `text\n` to the VTE PTY
For `SendText { text, execute: false }`: send `text` without newline (user presses Enter)

Higher-level steps (`LocalCd`, `Ssh`, `TmuxAttach`) are roadmap: they will be implemented as
sugar that expands to `SendText` chains during recipe construction.

### Honest promise

rttx makes explicit guarantees by recovery level:

- **Local shell pane**: layout + cwd + recipe replay. Shell state is not preserved.
- **SSH pane**: reconnect to the same host; replay post-connect steps. Depends on auth setup.
- **Tmux pane**: reattach to the named tmux session. Tmux is the real state carrier.
- **SSH + tmux pane**: reconnect SSH then reattach tmux. Best practical recovery path.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Layout + active pane restored | `WindowState` persists session list, active index, active pane UUID |
| G2 — Startup recipe replay | `startup_chain` replayed into VTE PTY after shell spawns |
| G3 — Pane origin tracked | `PaneSource` enum stored in `PaneRecovery` |
| G4 — Honest recovery | L1/L2 implemented; L3/L4 scope explicitly bounded |

---

## Development Plan

- [x] Layout, CWD, active session, active pane persistence (L1)
- [x] `PaneSource` and `StartupStep` data model
- [x] Recipe serialization and restore replay (L2)
- [x] New session from bookmark (replays bookmark startup chain)
- [ ] **SSH auto-reconnect** — detect SSH origin in recipe; reconnect on restore — *tracked in todo.md — Session Management*
- [ ] **Tmux auto-reattach** — detect tmux origin; reattach on restore — *tracked in todo.md — Session Management*
- [ ] **`ssh → tmux → folder` chains** — *tracked in todo.md — Session Management*
- [ ] **Shell integration** — OSC-based cwd + prompt boundaries for L3 accuracy — *tracked in todo.md — Session Management*
- [ ] **PTY daemon** — background session persistence after recipe-based recovery proves value — *tracked in todo.md — Session Management*

---

## Open Questions

- [ ] **Q1** — Settle delay vs shell integration marker for recipe replay timing: a fixed delay is simple but fragile on slow SSH connections; shell integration markers are accurate but require user shell configuration. What is the right default?

---
