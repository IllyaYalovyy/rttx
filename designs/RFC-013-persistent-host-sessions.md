# RFC-013: Persistent Host Sessions with Raw tmux Compatibility

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Draft                   |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Add a new persistent host-session architecture to `rttx` while preserving the current direct
terminal model and the current raw tmux workflow.

The core product decision is:

- keep today's direct VTE-based terminal sessions
- keep today's raw tmux support for users who explicitly want to use tmux itself
- add a new persistent session family whose lifetime is independent from the GUI
- support two persistent engines behind one frontend contract:
  `persistent-tmux` first and `persistent-native` second
- isolate failures per session, not behind one global daemon

This design addresses the main UX problem discussed in this thread: tmux inside a visible terminal
pane preserves persistence, but it does not solve selection and clipboard conflicts. The new
persistent mode must therefore treat tmux as a hidden host-side engine rather than as the visible
UI surface.

---

## Goals

- **G1** — Support sessions that keep running, updating, and preserving terminal state after the
  `rttx` GUI disconnects or crashes
- **G2** — Avoid a single point of failure that can kill all persistent sessions at once
- **G3** — Preserve the current raw tmux workflow unchanged for users who explicitly want tmux
- **G4** — Provide a "native" persistent option that does not require tmux
- **G5** — Keep older-host compatibility by minimizing host-side dependencies and avoiding a GTK/VTE
  requirement on the host
- **G6** — Keep the frontend UX under `rttx` control in persistent mode so selection, copy, and
  search no longer depend on visible tmux behavior
- **G7** — Keep the architecture open so the tmux-backed persistent engine can ship first without
  locking the product into tmux forever

## Non-Goals

- **NG1** — Do not remove or weaken the current direct VTE terminal path
- **NG2** — Do not replace raw tmux mode with a forced managed mode
- **NG3** — Do not require `systemd --user` or any Linux-only service manager on the host
- **NG4** — Do not require VTE or GTK to be installed on the host side
- **NG5** — Do not promise that the first `persistent-native` implementation is as battle-tested as
  tmux from day one
- **NG6** — Do not build one global PTY daemon that owns all sessions

---

## Background & Motivation

The current app architecture is a normal terminal-emulator architecture:

- the GTK frontend creates a `vte4::Terminal`
- the VTE widget owns a live PTY and child process
- the app layers bookmarks, sessions, recovery, links, and clipboard shortcuts on top

This works well for direct shell sessions and for the existing "tmux inside a terminal pane"
workflow. It does not satisfy the stronger requirement that a session must continue running after
the GUI goes away.

That stronger requirement changes the problem shape:

- some host-side process must remain alive while the GUI is gone
- that process must continue draining PTY output or foreground programs can block
- that process, not the GUI, becomes the long-lived source of truth for session state

At the same time, using visible tmux inside a VTE pane has an important UX limitation:

- tmux still owns mouse/copy-mode semantics inside the pane
- selection and clipboard behavior still depend on tmux's interaction model
- users still feel like they are "inside tmux"

That means "raw tmux in a pane" and "persistent `rttx` session" are different product modes and
must be modeled explicitly as such.

The design also needs an honest failure model. A single central daemon that owns every PTY would be
convenient, but it would create exactly the wrong failure domain: one crash kills all sessions.
The correct isolation boundary is the session, not the whole app.

This RFC builds on RFC-007's recovery direction but goes beyond recipe replay. Recovery describes
how to reconstruct context. Persistent sessions keep the context alive in the first place.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Can choose between today's direct/raw workflows and new persistent sessions that survive GUI disconnects |
| Contributors | Introduces a host-side session runtime, a transport protocol, and two pane families in the frontend |
| Packagers    | Need a host-side helper binary; tmux remains optional but recommended for the first persistent engine |

---

## Considered Options

### Option A — Keep only the current direct model

Keep VTE-owned PTYs and continue launching raw shells, SSH, and tmux inside visible panes.

**Pros**: Smallest codebase. Lowest implementation risk. Keeps the current architecture simple.
**Cons**: Cannot keep sessions alive after GUI disconnect. Raw tmux continues to have the same
selection/clipboard limitations that motivated this RFC.

### Option B — One global persistent daemon for all sessions

Run one host daemon that owns all PTYs for all persistent sessions.

**Pros**: Simplifies discovery and process management. Easiest place to centralize protocol logic.
**Cons**: Wrong failure model. One daemon crash risks every session. Violates the explicit
requirement to avoid "everything crashes together."

### Option C — Raw tmux only

Lean into tmux as the answer: keep or expand the current visible-tmux workflow and do not build a
separate persistent mode.

**Pros**: Reuses a mature multiplexer. Very low backend effort.
**Cons**: Does not solve the visible tmux UX problem. Selection and clipboard remain constrained by
tmux behavior inside the terminal pane.

### Option D — Persistent mode with a hidden tmux backend first, plus a native backend path

Preserve raw tmux as one explicit mode, but add a separate persistent session family. Use a hidden
tmux backend first for durable detach/reattach semantics, then add a native host engine behind the
same interface.

**Pros**: Best risk-adjusted path. Delivers true detached persistence quickly. Keeps raw tmux
available. Preserves a future path away from tmux.
**Cons**: Requires new host-side runtime, protocol, and a second frontend pane path.

---

## Decision

**Chosen option: Option D**

The product should explicitly support both:

- `raw tmux` for users who want tmux as tmux
- `persistent sessions` for users who want `rttx` to own the visible UX

Persistent sessions must use per-session host processes, not one global PTY owner. The first
persistent engine should be tmux-backed because tmux already solves the durability requirement well.
The architecture must nevertheless be engine-agnostic so a native host engine can later replace
tmux for users who prefer fewer dependencies or tighter product control.

---

## Design

### 1. Session families

The app supports two top-level session families.

#### Direct sessions

These keep today's model:

- local shell in VTE
- SSH in VTE
- raw tmux in VTE

Their behavior remains intentionally unchanged.

#### Persistent sessions

These are new:

- session lifetime is independent from the GUI
- the host runtime owns the authoritative session state
- the frontend attaches, detaches, and reattaches as a client

Persistent sessions expose two engines:

- `persistent-tmux`
- `persistent-native`

### 2. Product modes

The session creation/bookmark model gains an explicit execution mode.

Suggested modes:

- `direct`
- `raw-tmux`
- `persistent`

For persistent mode, add:

- `persistent_engine = tmux | native`

Expected defaults:

- normal local sessions default to `direct`
- explicit tmux bookmarks default to `raw-tmux`
- remote persistent workflows default to `persistent` with engine `tmux` first

### 3. Process model

Use a shared-nothing host runtime.

#### Frontend

`rttx` remains the GUI client:

- owns windows, tabs, sidebar, bookmarks, preferences, search UI
- may crash or disconnect without killing persistent sessions
- does not own live persistent PTYs

#### Broker

An optional `rttx-broker` process may exist for:

- launching `rttx-sessiond`
- locating existing sessions
- publishing socket paths or connection metadata

Rules:

- broker owns no PTYs
- broker is not on the data path for live pane traffic
- broker may die without killing existing sessions

#### Session daemon

Each persistent session runs in its own host process:

- `rttx-sessiond <session-id>`

Responsibilities:

- own only that session's PTYs and pane graph
- keep draining PTY output while no GUI client is attached
- keep session-local state such as layout, scrollback, current screen model, and metadata
- serve one or more reconnecting clients

Failure model:

- one `rttx-sessiond` crash affects only one session
- there is no global PTY owner whose crash can kill all sessions

### 4. Engine abstraction inside `rttx-sessiond`

`rttx-sessiond` should expose one engine-neutral internal contract.

Core operations:

- `create_pane`
- `split_pane`
- `close_pane`
- `resize`
- `send_input`
- `paste`
- `snapshot`
- `subscribe_deltas`
- `attach_client`
- `detach_client`
- `terminate_session`

Two implementations:

- `TmuxEngine`
- `NativeEngine`

This lets the frontend and transport stay stable while the backend engine changes.

### 5. Raw tmux compatibility

Raw tmux support remains as it is now.

Properties of `raw-tmux` mode:

- launches tmux inside a visible VTE pane
- preserves tmux keybindings and copy-mode exactly as today
- preserves current selection/clipboard limitations exactly as today

This mode exists for users who explicitly want tmux itself, not a tmux-backed `rttx` experience.

The app must not silently "upgrade" raw tmux mode into managed persistent mode.

### 6. Persistent tmux engine

This is the first shipping persistent engine.

#### Core approach

- `rttx-sessiond` creates or attaches to a private tmux session on the host
- tmux is the durability and pane-multiplexing engine
- `rttx` does not show raw tmux UI as the primary surface
- `rttx-sessiond` talks to tmux through control mode and tmux commands

#### Why this engine ships first

- tmux already guarantees that sessions live after client disconnect
- tmux already supports remote host workflows well
- tmux remains compatible with older systems better than a fresh native mux
- if `rttx-sessiond` crashes, the hidden tmux session may continue to exist

#### UX rule

In `persistent-tmux`, tmux is an implementation detail.

That means:

- selection belongs to `rttx`
- copy belongs to `rttx`
- search belongs to `rttx`
- pane chrome belongs to `rttx`

The user should not feel like they are operating raw tmux in a terminal pane.

### 7. Persistent native engine

This is the long-term engine.

#### Core approach

- `rttx-sessiond` owns PTYs directly
- it spawns shells, SSH commands, and remote child processes itself
- it continuously drains output and maintains an in-memory terminal state per pane

#### Reused building blocks

Recommended host-side components:

- `portable-pty` for PTY/process management
- `vt100` for a first screen model and terminal parser

These are sufficient for an MVP and avoid requiring GTK/VTE on the host.

#### Native engine caveat

Unlike the tmux engine, if `rttx-sessiond` is the PTY owner and crashes, that specific session is
at risk. This is acceptable for per-session isolation but means the native engine starts with a
weaker durability story than the tmux engine.

The product should therefore treat:

- `persistent-tmux` as the stable first persistent engine
- `persistent-native` as a preview until its behavior and fidelity are proven

### 8. Frontend pane model

The frontend should support two pane families.

#### `VTEPane`

Used for:

- `direct`
- `raw-tmux`

This reuses the current widget and behavior.

#### `PersistentPaneView`

Used for:

- `persistent-tmux`
- `persistent-native`

This pane renders from:

- an initial host-provided snapshot
- a stream of host-provided deltas

It owns:

- selection
- copy
- search
- link handling on the locally rendered model

VTE remains part of the product, but it is no longer the sole pane implementation.

### 9. Why persistent mode is not "VTE attached to a remote PTY"

Persistent mode requires the host runtime to keep canonical terminal state while the GUI is gone.
That means the host side must be able to:

- continue draining output
- preserve screen state and scrollback
- answer reconnect requests with a snapshot

Therefore the canonical terminal model for persistent mode must live in the host session engine,
not in a client-side VTE widget.

VTE still fits well for direct mode. It is not the right source of truth for detached persistent
mode.

### 10. Transport and reconnect model

Use a versioned framed protocol over:

- Unix sockets for local host sessions
- SSH stdio tunneling for remote host sessions

Representative message types:

- `Hello`
- `Capabilities`
- `OpenSession`
- `Attach`
- `Detach`
- `Snapshot`
- `Delta`
- `Input`
- `Paste`
- `Resize`
- `CreatePane`
- `SplitPane`
- `ClosePane`
- `Bell`
- `Exit`
- `Error`

Reconnect behavior:

- client attaches
- host sends a full `Snapshot`
- host resumes `Delta` streaming

### 11. Selection and clipboard semantics

#### Direct / raw tmux

No change:

- current VTE selection behavior remains
- current raw tmux behavior remains

#### Persistent mode

Frontend-local semantics:

- selection is performed against the locally rendered screen model
- copy always copies from `rttx`
- paste sends bytes to the focused host pane

This is the key UX difference that justifies the new mode.

### 12. Host compatibility policy

The host-side runtime must remain conservative:

- no GTK dependency
- no VTE dependency
- no systemd requirement
- no root requirement
- user-space installable
- launchable through ordinary `ssh`

Persistent engine availability:

- if host has tmux, `persistent-tmux` is available
- if host has `rttx-sessiond` native support, `persistent-native` is available
- if neither is available, the app falls back to `direct` workflows

### 13. Failure model

The architecture intentionally supports different durability levels.

#### GUI failure

- frontend crash does not kill persistent sessions

#### Broker failure

- existing sessions continue to run

#### Session daemon failure

- only one session is affected

#### Persistent tmux engine

- hidden tmux session may survive `rttx-sessiond` failure
- reconnect can potentially restore control over the tmux-backed session

#### Persistent native engine

- the affected session may die if its owning `rttx-sessiond` dies
- blast radius is limited to one session

This difference is intentional and should be documented honestly.

### 14. State model changes

Bookmark and session metadata need new fields.

Suggested additions:

- `execution_mode`
- `persistent_engine`
- host session identifiers
- optional reconnect metadata such as socket path, remote helper location, or tmux session name

These fields complement, not replace, RFC-007 recovery metadata.

### 15. Relationship to RFC-007

RFC-007 remains valid.

Recovery and persistence solve different problems:

- recovery reconstructs context after loss
- persistence keeps context alive so reconstruction is unnecessary

The two systems should coexist:

- `direct` sessions rely primarily on RFC-007 recovery
- `persistent` sessions rely primarily on host-side continuity
- recovery still matters if a persistent backend is unavailable or intentionally downgraded

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 | Per-session host runtimes keep sessions alive after GUI disconnect |
| G2 | No global PTY owner; one session daemon per persistent session |
| G3 | `raw-tmux` remains a first-class unchanged mode |
| G4 | `persistent-native` is built into the engine abstraction from the start |
| G5 | Host runtime avoids GTK/VTE/systemd requirements |
| G6 | Persistent mode uses `PersistentPaneView` with frontend-owned selection/copy UX |
| G7 | `TmuxEngine` and `NativeEngine` share one session-daemon contract |

---

## Development Plan

- [ ] **Step 1** — Define session-mode and persistent-engine schema changes for bookmarks and
  session creation UI *(prerequisite: —)*
- [ ] **Step 2** — Define the versioned host transport protocol and session-daemon lifecycle
  *(prerequisite: Step 1)*
- [ ] **Step 3** — Implement `rttx-broker` and `rttx-sessiond` process management with per-session
  isolation *(prerequisite: Step 2)*
- [ ] **Step 4** — Implement `persistent-tmux` backend using tmux control mode / commands
  *(prerequisite: Step 3)*
- [ ] **Step 5** — Implement `PersistentPaneView` and the frontend attach/snapshot/delta flow
  *(prerequisite: Step 4)*
- [ ] **Step 6** — Expose persistent remote sessions in bookmarks/session creation while preserving
  `direct` and `raw-tmux` modes *(prerequisite: Step 5)*
- [ ] **Step 7** — Ship `persistent-tmux` as the stable first persistent mode *(prerequisite:
  Step 6)*
- [ ] **Step 8** — Prototype `persistent-native` using `portable-pty` plus `vt100`
  *(prerequisite: Step 5)*
- [ ] **Step 9** — Validate fidelity, resize semantics, selection, and reconnect behavior for the
  native engine *(prerequisite: Step 8)*
- [ ] **Step 10** — Promote `persistent-native` from preview only after quality is acceptable
  *(prerequisite: Step 9)*

---

## Open Questions

- [ ] **Q1** — Should `persistent-tmux` use one tmux session per `rttx` session or one tmux
  window group per `rttx` session?
- [ ] **Q2** — Should `PersistentPaneView` be built as a custom GTK widget immediately, or should
  the first milestone use a simpler text-surface implementation to prove protocol and UX first?
- [ ] **Q3** — What minimum host version policy should `persistent-native` target for PTY APIs and
  SSH transport behavior?
- [ ] **Q4** — Should `persistent-native` remain preview until a per-session guardian/controller
  split exists, or is per-session isolation sufficient for the first stable release?
- [ ] **Q5** — How much scrollback should persistent engines keep by default, and should that be a
  host-side or client-side policy knob?

---

## References

- [RFC-007: Per-Pane Recovery Recipes & Smart Session Restoration](./RFC-007-session-recovery.md)
- [VTE gtk4 Terminal API](https://gnome.pages.gitlab.gnome.org/vte/gtk4/class.Terminal.html)
- [tmux Control Mode wiki](https://github.com/tmux/tmux/wiki/Control-Mode)
- [`portable-pty` crate](https://docs.rs/crate/portable-pty/0.4.0)
- [`vt100` crate](https://docs.rs/vt100/latest/vt100/)
- [`alacritty_terminal` crate](https://docs.rs/alacritty_terminal/latest/alacritty_terminal/)
- [`tmux_interface` crate](https://docs.rs/tmux_interface/latest/tmux_interface/)
