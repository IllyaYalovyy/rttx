# RFC-007: Per-Pane Recovery Recipes & Smart Session Restoration

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented (L1–L2; L3–L4 superseded by RFC-013) |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

> Historical note (2026-04): RFC-013 is now the authoritative architecture RFC for daemon-backed
> execution. RFC-016 removed tmux integration and replaced bookmarks with Places. This RFC still
> defines recipe-based recovery metadata and the retry UX contract. The data model section below
> reflects the current implementation; removed variants (`Bookmark`, `SessionTemplate`,
> `LocalTmux`, `RemoteTmux`) exist only in backward-compatible deserialization code.
>
> Older uses of `session` in this document map to the current product term `workspace` unless the
> text is explicitly referring to current code types such as `SessionState`.

## Summary

rttx persists more than layout geometry. Each pane carries a recovery recipe — a structured
description of how the pane was created and what it was for. On restart, rttx replays these
recipes to reconstruct working context: local folders and SSH connections. The goal is honest
recovery, not fake state serialization.

The workspace is the user-facing recovery unit: users restore, retry, and manage whole
workspaces. The pane is the execution unit: each pane has its own recoverable target and may
succeed or fail independently.

---

## Goals

- **G1** — Restarting the app restores the split layout, active workspace, and active pane
- **G2** — Each pane replays its startup recipe or reconnect target (local folder, SSH) on restore
- **G3** — Pane origin is tracked (empty shell, command, manual) and survives restart
- **G4** — Recovery is honest: rttx only promises what it can actually deliver
- **G5** — Recovery failure is non-destructive: the pane stays alive and offers retry

## Non-Goals

- **NG1** — In-memory shell state (variables, partially typed commands, foreground TUI state) is not preserved
- **NG2** — CRIU / process snapshotting is not used; it is fragile and incompatible with networked processes
- **NG3** — Shell integration (OSC-based cwd reporting, prompt boundaries) is a later phase
- **NG4** — rttx does not silently create new remote sessions when recovery expected to attach to an existing one

---

## Background & Motivation

Most terminal emulators treat session restore as a UI concept: same number of splits, same
geometry, fresh shells. For a developer who was connected to a production server via SSH,
a fresh shell is not a restore — it is an empty starting point that requires minutes of manual
reconnection.

rttx can do better without becoming a process supervisor. The key insight is that what matters
is not the exact in-memory state of the shell, but the path the user took to get there. That
path — the startup recipe — is small, serializable, and replayable.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Restarting rttx restores working context; SSH panes reconnect automatically (roadmap) |
| Contributors | Recovery data is part of `session/recovery.rs`; stored in `SessionState.terminal_recovery` |
| Packagers | No change; recovery state is stored in the existing `sessions.json` |

---

## Considered Options

### Option A — Shell integration only *(reconstructed)*

Use OSC escape sequences from the shell to report cwd, prompt boundaries, and command
boundaries. Store these in the session state.

**Pros**: Non-invasive; improves cwd accuracy; enables semantic scrollback.
**Cons**: Does not recover SSH connections. Requires shell configuration by the user. Provides
metadata, not recovery.

### Option B — Per-pane recovery recipes

Store a structured `startup: Vec<StartupStep>` in each pane's `PaneRecovery` data. On restore,
replay the chain: `cd`, `ssh`. Structured `PaneTarget` values cover the high-value recovery
paths.

**Pros**: Works without shell integration. Covers the most valuable recovery paths (local cd,
SSH). Honest: the recipe describes exactly what will be replayed, no magic.
**Cons**: Cannot recover arbitrary shell state. Recipe must be written at pane creation time,
not after the fact.

### Option C — PTY daemon / client-server architecture

A persistent background process owns the PTYs. The GTK UI connects and disconnects without
killing the shells.

**Pros**: True process persistence; closest to "as if you never closed the terminal."
**Cons**: Major architectural change. Significant complexity.

**Current status**: This is now the live architecture. RFC-013 defines the daemon-backed runtime
model implemented in `rttx-server`. Recipe recovery (Option B) augments it with workspace-owned
recovery metadata.

### Option D — CRIU process snapshotting *(reconstructed)*

Freeze and restore arbitrary process trees at the OS level.

**Pros**: Theoretically complete state recovery.
**Cons**: Fragile with interactive/networked processes. Not suitable for SSH sessions or TUI
applications. Incompatible with the rock-solid stability goal.

---

## Decision

Chosen option: B for workspace-owned recovery metadata, with C as the runtime architecture

Recovery recipes are the right abstraction for workspace-owned metadata. They are serializable,
testable, and honest about what they can and cannot restore. Shell integration (Option A) adds
accuracy on top of recipes and is planned for a later phase.

The PTY daemon (Option C) is now the live runtime architecture per RFC-013. The daemon owns PTYs,
scrollback, and process lifetime. Recipe recovery augments the daemon model with replayable
context that the daemon does not own: which bookmark or command created the pane, and what SSH
target to reconnect to.

---

## Design

### Data model

The following types live in `clients/rttx/src/session/recovery.rs`:

```rust
pub enum PaneSource {
    EmptyShell,
    Command { title: String },
    Manual,
}

pub enum StartupStep {
    SendText { text: String, execute: bool },
}

pub enum PaneTarget {
    LocalFolder { path: String },
    RemoteShell { ssh_target: String, remote_folder: Option<String> },
}

pub struct PaneRecovery {
    pub source: PaneSource,
    pub target: Option<PaneTarget>,
    pub startup: Vec<StartupStep>,
}
```

`PaneRecovery` is stored in `SessionState.terminal_recovery: BTreeMap<String, PaneRecovery>`,
keyed by terminal UUID. It is separate from `LayoutNode::Terminal` to allow the layout tree to
remain a pure geometry structure. Both structures serialize with `#[serde(default)]` so old
`sessions.json` files without recovery data continue to work.

Removed variants (`Bookmark`, `SessionTemplate` in `PaneSource`; `LocalTmux`, `RemoteTmux` in
`PaneTarget`) are handled by custom `Deserialize` implementations that map them to `Manual` or
`None` respectively, preserving backward compatibility with older persisted state.

Structured `PaneTarget` values are the high-value recovery path:

- `LocalFolder` — start shell in a saved directory
- `RemoteShell` — reconnect SSH, optionally cd to a remote folder

`startup` remains available as a flexible escape hatch and compatibility layer, but the
long-term design prefers structured targets over raw shell text for SSH flows.

### Recovery levels

| Level | What is restored | Status |
| --- | --- | --- |
| L1 — UI | Layout, splits, active workspace, active pane | Implemented |
| L2 — Context | CWD, custom title, pane origin, startup recipe replay | Implemented |
| L3 — Reconnect | SSH reconnect, retry UX | Superseded by RFC-013 daemon model |
| L4 — True persistence | PTY processes kept alive; UI reconnects to live shells | Superseded by RFC-013 daemon model |

L3 and L4 are now handled by the daemon-backed runtime architecture defined in RFC-013. The
daemon owns PTYs, scrollback, and process lifetime. The GUI reconnects to the daemon rather
than replaying recipes for process persistence. Recipe recovery still provides the workspace-owned
metadata (which bookmark created the pane, what SSH target to use) that the daemon does not own.

### Replay on restore

On startup, for each pane with recovery metadata:

1. Create the terminal widget and spawn the shell
2. Attempt recovery once automatically
3. If the attempt succeeds, mark the pane ready
4. If the attempt fails, keep the pane alive, mark it failed, and offer `Retry`

Target-specific behavior:

- `LocalFolder { path }`
  Start shell in that directory when possible; otherwise fall back to `cd`
- `RemoteShell { ssh_target, remote_folder }`
  Attempt SSH connection, then optional remote `cd`

For `SendText { text, execute: true }`: send `text\n` to the VTE PTY
For `SendText { text, execute: false }`: send `text` without newline (user presses Enter)

The same recovery mechanism is used after restart and for later manual retry. A temporary network
failure should not kill the pane or the workspace.

### Workspace vs pane responsibilities

- Workspace owns layout, aggregate status, and the user-facing restore/retry workflow
- Pane owns the concrete recovery target and execution attempt
- A workspace may be partially restored: one pane can be ready while another is failed and waiting
  for retry

### Failure UX

Recovery must never use modal dialogs.

If a pane fails to recover:

- the pane remains open
- the terminal widget remains available
- the pane shows a compact in-pane recovery strip with short error text and `Retry`
- the workspace may also surface degraded state in the workspace row, but the primary control lives in
  the pane itself

Connection state for daemon-backed workspaces is managed by the workspace-level state machine
defined in RFC-018. Workspace-level reconnect and remediation actions render once per workspace
in the sidebar row, not once per pane (per RFC-013).

### Honest promise

rttx makes explicit guarantees by recovery level:

- **Local shell pane**: layout + cwd + best-effort recipe replay. Shell state is not preserved.
- **SSH pane**: reconnect to the same host; replay post-connect steps. Depends on auth setup.
- **Daemon-backed pane**: PTY and scrollback survive GUI detach and daemon restart. The daemon
  reconstructs from persisted state (per RFC-013 and RFC-022).

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Layout + active pane restored | `WindowState` persists session list, active index, active pane UUID |
| G2 — Startup recipe / reconnect replay | `PaneTarget` + `startup` replayed during pane recovery |
| G3 — Pane origin tracked | `PaneSource` enum stored in `PaneRecovery` |
| G4 — Honest recovery | L1/L2 implemented; L3/L4 delegated to daemon (RFC-013) |
| G5 — Failure is non-destructive | pane-level failure state with manual retry, no modal dialog |

---

## Development Plan

- [x] Layout, CWD, active workspace, active pane persistence (L1)
- [x] `PaneSource` and `StartupStep` data model
- [x] Recipe serialization and restore replay (L2)
- [x] New workspace from bookmark/place (replays startup chain)
- [x] `PaneTarget` model — `LocalFolder` and `RemoteShell` implemented
- [x] Automatic startup attempt — recovery on launch for eligible panes
- [ ] **Manual retry UX** — in-pane retry strip, no modal dialog
- [ ] **SSH auto-reconnect** — reconnect failed/restored `RemoteShell` panes
- [ ] **Shell integration** — OSC-based cwd + prompt boundaries for better capture and replay timing

Items previously listed here for tmux integration, remote tmux chains, and PTY daemon are now
tracked elsewhere:
- Tmux integration was removed per RFC-016
- PTY daemon is the live architecture per RFC-013
- Daemon state persistence is defined in RFC-022

---

## Open Questions

- [x] **Q1** — Replay timing: fixed settle delay is simple but fragile on slow SSH connections;
  shell integration markers are accurate but require shell configuration. What is the right
  default before shell integration exists?
  *Resolved*: The daemon model (RFC-013) handles process persistence directly. Recipe replay
  timing matters only for the initial SSH connection command, where a fixed delay is acceptable
  as a pragmatic default.
- [x] **Q2** — Capture ergonomics: how much of a manually established SSH flow can be captured
  reliably into a place or session target without shell integration?
  *Resolved*: RFC-016 replaced bookmarks with Places. SSH targets are captured at workspace
  creation time through the explicit host selection UI. Manual flows are not auto-captured.

---
