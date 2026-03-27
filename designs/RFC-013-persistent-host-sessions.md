# RFC-013: Daemon-Backed Workspaces and Runtimes

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Draft (v3)              |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | RFC-013 v1 (tmux-first) |
| Superseded by | ---                       |

---

## Summary

Standardize rttx on one managed execution model: daemon-backed runtimes served by `rttx-server`.
The same runtime architecture serves local and remote hosts. User-facing containers are called
workspaces; daemon-owned live backends are called runtimes.

The core decisions:

- one daemon-backed runtime path serves both local and remote endpoints
- runtime policy is `ephemeral` or `persistent`; both policies use the same backend architecture
- there is no implicit fallback to a separate direct-terminal implementation when the daemon or SSH
  transport is unavailable
- transient disconnects auto-reconnect; failures that require user action remain explicit in the
  workspace UI
- GUI state, daemon state, and bindings reconcile non-destructively; missing GUI metadata must
  never delete a daemon runtime or pane automatically

---

## Requirements

- **R1** — Workspace creation must be non-blocking. A new workspace may open in `Starting`,
  `Connecting`, or `Reconnecting` state, but the GTK UI must stay responsive.
- **R2** — Every managed workspace is daemon-backed. The product does not silently switch to a
  different execution model when an endpoint is unavailable.
- **R3** — One workspace binds to exactly one endpoint and one runtime policy. Local and remote
  panes do not mix inside the same workspace.
- **R4** — One app window may host multiple workspaces with different endpoints and policies.
- **R5** — Transient failures auto-reconnect with backoff. Failures that require user action stop
  in an explicit blocked/error state.
- **R6** — Reconciliation is non-destructive. Missing GUI state may create recovered workspaces;
  missing runtime state may create disconnected placeholders. Neither case may implicitly delete
  daemon runtimes or panes.
- **R7** — Layout and presentation belong to the workspace. PTYs, scrollback, runtime CWD/title,
  and process lifetime belong to the runtime.
- **R8** — Pane create/close/split operations must be acknowledged by the runtime before the GUI
  commits the resulting layout mutation.
- **R9** — Multiple windows may connect to the same endpoint. A specific runtime has one writer by
  default; any multi-attach mode must be explicit.
- **R10** — Users must always know the current runtime state: `Starting`, `Connecting`,
  `Connected`, `Reconnecting`, `Blocked`, `Disconnected`, or `Recovered`.

---

## Goals

- **G1** — One runtime architecture for local and remote managed execution
- **G2** — One daemon per endpoint, with endpoint-scoped connection management and reconnect
- **G3** — Clear user-facing semantics: workspace, runtime, endpoint, pane, layout, policy
- **G4** — Safe recovery and reconciliation that never destroys daemon state implicitly
- **G5** — Host-side daemon stays portable: no GTK, no VTE, no systemd requirement
- **G6** — Frontend owns rendering, selection, copy, search, clipboard, and workspace presentation
- **G7** — The core state machine is pure and testable outside GTK

## Non-Goals

- **NG1** — Do not preserve a first-class managed direct-terminal path alongside the daemon-backed
  model
- **NG2** — Do not require `systemd --user` or any Linux-only service manager
- **NG3** — Do not require VTE or GTK on the host side
- **NG4** — Do not attempt process checkpointing (CRIU or similar)
- **NG5** — Do not build a TUI client for `rttx-server`; it remains a backend for the rttx GUI
- **NG6** — Do not allow reconciliation to infer destructive intent from stale or missing metadata

---

## Terminology

| Term | Meaning |
|---|---|
| Workspace | Top-level GUI object in the rttx sidebar |
| Runtime | Daemon-owned live backend object attached to one workspace |
| Pane | One terminal tile inside a workspace/runtime |
| Layout | The arrangement of panes and split ratios inside a workspace |
| Endpoint | The local daemon or one remote host daemon reached over SSH |
| Policy | Runtime retention model: `ephemeral` or `persistent` |

Current Rust code still uses `Session*` names in some modules and persisted types. This RFC uses
`Workspace` and `Runtime` for the product concepts; code-level names may lag until follow-up
refactors land.

---

## Architecture

### Overview

```
Local machine                          Remote host
--------------                         -----------
rttx (GTK GUI)                         rttx-server (daemon)
    |                                      |
    |--- Unix socket ----> local daemon    |--- PTYs (bash, zsh, ...)
    |                                      |--- state.json / scrollback
    |--- SSH stdio -----> remote daemon    |
    |
    |--- workspace metadata + bindings
```

### Ownership boundaries

**GUI / workspace state**
- window placement, sidebar order, selected workspace, split ratios, custom titles
- presentation-only status and banners
- bindings between workspace pane nodes and daemon pane ids

**Daemon / runtime state**
- runtime existence and policy
- PTYs, scrollback, cwd, runtime titles, process lifetime
- runtime reconstruction after daemon restart
- endpoint-scoped transport serving one or more clients

**Bindings**
- `workspace_id`
- `runtime_id`
- `pane_binding_id == daemon_pane_id`

Absence is never deletion. If either side is missing state, reconciliation creates recovered or
disconnected objects rather than destroying live daemon data.

### Runtime policies

Both policies are daemon-backed:

- **Ephemeral** — disposable runtime intended to be cleaned up when no workspace remains attached
  or when the user closes it explicitly
- **Persistent** — runtime intended to survive GUI detach, app restart, reconnect, and daemon
  reconstruction after restart

### Endpoint rules

- A workspace is homogeneous: one endpoint and one policy
- One window may contain multiple workspaces that target different endpoints and policies
- Multiple windows may connect to the same endpoint
- A runtime has one writer by default; shared/mirrored attach would be an explicit future mode

### Connection management

The GUI keeps one connection manager per endpoint. It:

- establishes the transport (Unix socket locally, SSH stdio remotely)
- multiplexes workspaces and runtimes on that endpoint
- routes protocol messages onto the GTK main loop
- owns reconnect backoff for transient failures
- classifies failures into `transient` or `needs user action`

---

## Design

### Workspace state machine

The application layer should expose a pure state machine:

- `Starting`
- `Connecting`
- `Connected`
- `Reconnecting { attempt, next_retry_at }`
- `Blocked { reason }`
- `Disconnected`
- `Recovered`

GTK renders these states with banners, disabled input, and actions such as `Retry now`, `Close`,
or `Edit connection`.

### Failure classification

**Transient**
- local daemon still starting
- local socket temporarily unavailable
- SSH timeout or broken pipe
- host reboot / sleep / wake
- daemon restart

These auto-retry with backoff.

**Needs user action**
- bad credentials
- host key verification failure
- protocol version mismatch
- missing daemon binary
- unsupported server version

These stop in `Blocked` with a clear explanation and a user action.

### Reconciliation contract

Use stable ids everywhere:

- `workspace_id`
- `runtime_id`
- `pane_binding_id == daemon_pane_id`

Rules:

1. If a runtime exists but the GUI has no workspace for it, create a recovered workspace.
2. If a workspace is bound to a missing runtime, keep the workspace and mark it disconnected or
   orphaned.
3. If a runtime has extra panes not present in the layout, add recovered panes.
4. If the layout references missing runtime panes, keep explicit placeholders.
5. Reconciliation may create recovered objects automatically, but it may never delete a daemon
   runtime or pane automatically.

### Terminal model

There is one product-level terminal model for managed execution. Any user-facing action such as
search, zoom, copy, paste, title updates, or cwd reporting should target a shared terminal
abstraction rather than splitting behavior across "direct" and "persistent" paths.

### Relationship to recipe recovery (RFC-007)

RFC-007 still matters for workspace-owned recovery metadata: bookmarks, SSH/tmux targets, retry
UX, and honest replay of what the user asked the pane to do. It is no longer the primary managed
execution architecture. The daemon-backed runtime model is primary, and recipe recovery augments it
where replayable context is useful.

---

## Development Plan

1. **Terminology and doc cleanup**
   Align product docs on `Workspace`, `Runtime`, `Endpoint`, `Policy`, and the no-fallback rule.
2. **Endpoint connection manager**
   Replace synchronous bridge calls with an endpoint-scoped async manager and a pure connection
   state machine.
3. **Workspace/runtime binding model**
   Persist stable ids and a binding map so restore and reconcile do not depend on layout position.
4. **Homogeneous workspace policy**
   Enforce one endpoint and one runtime policy per workspace while allowing mixed workspaces in the
   same window.
5. **Explicit connection UX**
   Add `Connecting`, `Reconnecting`, `Blocked`, and `Recovered` UI states with safe retry
   controls.
6. **Shared terminal abstraction**
   Make search, zoom, copy, paste, cwd/title tracking, and notifications operate through one
   terminal abstraction for all managed workspaces.
7. **Safe destructive actions**
   Separate `Close workspace`, `Detach runtime`, `Terminate runtime`, and `Delete local metadata`.
8. **Test strategy**
   Push most workflow logic into pure reducer/state-machine tests; keep GTK tests for wiring and
   widget contracts only.

---

## Open Questions

- **Q1** — Should multi-attach to the same runtime eventually support `share`, `read-only mirror`,
  or `take over`, and how explicit should that handoff be?
- **Q2** — What default scrollback retention and disk cap should each pane use?
- **Q3** — Should local daemon auto-start happen only on demand, or also at login when the user
  opts in?
- **Q4** — How should the GUI surface endpoint version skew when different hosts run different
  daemon versions?
- **Q5** — Should daemon reconstruction be surfaced as terminal output, a GUI overlay, or both?

---

## Prior Art: Zellij

[Zellij](https://github.com/zellij-org/zellij) (MIT, Rust, 30k+ stars) validates the core
architecture of this RFC:

- A Rust-native PTY-owning server works reliably in production at scale
- Periodic serialization (every 1 second) of layout + commands + scrollback to disk is proven
- Session resurrection from serialized state after reboot works well
- The "don't auto-run resurrected commands" safety pattern is important
- Versioned protocol contracts allow binary upgrades without breaking sessions

Key differences from rttx:
- Zellij is a TUI multiplexer running inside a terminal; rttx IS the terminal (GTK4)
- Zellij uses one server for all sessions (same as this RFC's endpoint-scoped process model)
- Zellij cannot run on remote hosts without SSH + tmux; `rttx-server` can serve remote runtimes natively

---

## References

- [RFC-007: Per-Pane Recovery Recipes & Smart Session Restoration](./RFC-007-session-recovery.md)
- [rttxd repository](https://github.com/IllyaYalovyy/rttxd)
- [Zellij terminal workspace](https://github.com/zellij-org/zellij)
- [Zellij session resurrection docs](https://zellij.dev/documentation/session-resurrection)
- [`pty-process` crate](https://docs.rs/pty-process/latest/pty_process/)
- [`prost` crate](https://docs.rs/prost/latest/prost/) --- protocol buffer serialization
- [`vte` crate](https://docs.rs/vte/latest/vte/) --- terminal escape sequence parser (no GTK)
