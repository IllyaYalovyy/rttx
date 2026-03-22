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

The session is the user-facing recovery unit: users restore, retry, and manage whole sessions.
The pane is the execution unit: each pane has its own recoverable target and may succeed or fail
independently.

---

## Goals

- **G1** — Restarting the app restores the split layout, active session, and active pane
- **G2** — Each pane replays its startup recipe or reconnect target (local folder, SSH, tmux attach) on restore
- **G3** — Pane origin is tracked (empty shell, bookmark, command) and survives restart
- **G4** — Recovery is honest: rttx only promises what it can actually deliver
- **G5** — Recovery failure is non-destructive: the pane stays alive and offers one-click retry

## Non-Goals

- **NG1** — In-memory shell state (variables, partially typed commands, foreground TUI state) is not preserved
- **NG2** — PTY processes are not kept alive between rttx restarts (that is tmux's job)
- **NG3** — CRIU / process snapshotting is not used; it is fragile and incompatible with networked processes
- **NG4** — Shell integration (OSC-based cwd reporting, prompt boundaries) is a later phase
- **NG5** — rttx does not silently create a fresh tmux session when recovery expected to attach to an existing one

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
    pub target: Option<PaneTarget>,
    pub startup_chain: Vec<StartupStep>,
}

pub enum PaneSource {
    EmptyShell,
    Bookmark(String),   // bookmark UUID
    Command(String),    // saved command UUID
    SessionTemplate(String),
    Manual,
}

pub enum PaneTarget {
    LocalFolder { path: String },
    LocalTmux { session: String },
    RemoteShell { ssh_target: String, remote_folder: Option<String> },
    RemoteTmux { ssh_target: String, tmux_session: String },
}

pub enum RecoveryState {
    Idle,
    Connecting,
    Ready,
    Failed { message: String },
}

pub enum StartupStep {
    SendText { text: String, execute: bool },
    // Roadmap sugar: LocalCd { path }, Ssh { target }, TmuxAttach { session }
}
```

`PaneRecovery` is stored in `SessionState.terminal_recovery: BTreeMap<String, PaneRecovery>`,
keyed by terminal UUID. It is separate from `LayoutNode::Terminal` to allow the layout tree to
remain a pure geometry structure. Both structures serialize with `#[serde(default)]` so old
`sessions.json` files without recovery data continue to work.

Structured `PaneTarget` values are the high-value recovery path. They cover the workflows that
matter most and support reliable retry UX:

- `LocalFolder`
- `LocalTmux`
- `RemoteShell`
- `RemoteTmux`

`startup_chain` remains available as a flexible escape hatch and compatibility layer, but the
long-term design prefers structured targets over raw shell text for SSH/tmux flows.

### Recovery levels

| Level | What is restored | Status |
| --- | --- | --- |
| L1 — UI | Layout, splits, active session, active pane | Implemented |
| L2 — Context | CWD, custom title, pane origin, startup recipe replay | Implemented |
| L3 — Reconnect | SSH reconnect, tmux reattach, `ssh → tmux` chains, retry UX | Roadmap |
| L4 — True persistence | PTY processes kept alive; UI reconnects to live shells | Future |

### Replay on restore

On startup, for each pane with recovery metadata:

1. Create the terminal widget and spawn the shell
2. Attempt recovery once automatically
3. If the attempt succeeds, mark the pane ready
4. If the attempt fails, keep the pane alive, mark it failed, and offer `Retry`

Target-specific behavior:

- `LocalFolder { path }`
  Start shell in that directory when possible; otherwise fall back to `cd`
- `LocalTmux { session }`
  Attempt `tmux attach -t <session>`
- `RemoteShell { ssh_target, remote_folder }`
  Attempt SSH connection, then optional remote `cd`
- `RemoteTmux { ssh_target, tmux_session }`
  Attempt SSH connection, then `tmux attach -t <session>`

For `SendText { text, execute: true }`: send `text\n` to the VTE PTY
For `SendText { text, execute: false }`: send `text` without newline (user presses Enter)

For tmux-backed recovery, rttx always uses attach-only semantics. If the expected tmux session
does not exist, the pane must fail visibly. Creating a new empty tmux session automatically would
look like success while actually losing context.

The same recovery mechanism is used after restart and for later manual retry. A temporary network
failure should not kill the pane or the session.

### Session vs pane responsibilities

- Session owns layout, aggregate status, and the user-facing restore/retry workflow
- Pane owns the concrete recovery target and execution attempt
- A session may be partially restored: one pane can be ready while another is failed and waiting
  for retry

### Failure UX

Recovery must never use modal dialogs.

If a pane fails to recover:

- the pane remains open
- the terminal widget remains available
- the pane shows a compact in-pane recovery strip with short error text and `Retry`
- the session may also surface degraded state in the session row, but the primary control lives in
  the pane itself

### Honest promise

rttx makes explicit guarantees by recovery level:

- **Local shell pane**: layout + cwd + best-effort recipe replay. Shell state is not preserved.
- **SSH pane**: reconnect to the same host; replay post-connect steps. Depends on auth setup.
- **Local tmux pane**: reattach to the named tmux session. Tmux is the real state carrier.
- **SSH + tmux pane**: reconnect SSH then reattach tmux. Best practical recovery path.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Layout + active pane restored | `WindowState` persists session list, active index, active pane UUID |
| G2 — Startup recipe / reconnect replay | `PaneTarget` + `startup_chain` replayed during pane recovery |
| G3 — Pane origin tracked | `PaneSource` enum stored in `PaneRecovery` |
| G4 — Honest recovery | L1/L2 implemented; L3/L4 scope explicitly bounded |
| G5 — Failure is non-destructive | pane-level failure state with manual retry, no modal dialog |

---

## Development Plan

- [x] Layout, CWD, active session, active pane persistence (L1)
- [x] `PaneSource` and `StartupStep` data model
- [x] Recipe serialization and restore replay (L2)
- [x] New session from bookmark (replays bookmark startup chain)
- [ ] **PaneTarget model** — add structured `LocalFolder`, `LocalTmux`, `RemoteShell`, `RemoteTmux`
- [ ] **Automatic startup attempt** — try recovery once on launch for eligible panes
- [ ] **Manual retry UX** — in-pane retry strip, no modal dialog
- [ ] **SSH auto-reconnect** — reconnect failed/restored `RemoteShell` and `RemoteTmux` panes
- [ ] **Tmux auto-reattach** — attach-only semantics; visible failure if session is missing
- [ ] **`ssh → tmux` chains** — first-class high-value recovery path
- [ ] **Remote folder replay** — only for non-tmux remote shell panes
- [ ] **Shell integration** — OSC-based cwd + prompt boundaries for better capture and replay timing
- [ ] **PTY daemon** — background session persistence after recipe-based recovery proves value — *tracked in todo.md — Session Management*

---

## Open Questions

- [ ] **Q1** — Replay timing: fixed settle delay is simple but fragile on slow SSH connections; shell integration markers are accurate but require shell configuration. What is the right default before shell integration exists?
- [ ] **Q2** — Capture ergonomics: how much of a manually established SSH/tmux flow can be captured reliably into a bookmark or session target without shell integration?

---
