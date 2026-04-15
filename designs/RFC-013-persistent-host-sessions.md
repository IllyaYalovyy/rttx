# RFC-013: Daemon-Backed Workspaces and Runtimes

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | RFC-013 v1 (tmux-first) |
| Superseded by | ---                     |

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
- inventory reconciliation is safe by default, but explicit user `Close Workspace` is
  authoritative: it must remove the workspace, stop any attached runtime, and suppress later
  automatic resurrection
- attaching to an existing runtime is explicit; `New Workspace` always creates exactly one new
  workspace
- workspace-level reconnect and remediation actions render once per workspace in the tab/sidebar
  row, not once per pane

## Current implementation snapshot (2026-04)

The architecture defined in this RFC is fully implemented on `mainline`:

- `rttx-server` and `rttx-proto` live in the same repository as the client
- daemon inventory, runtime revisions, retention policy, and single-writer ownership are
  implemented in `services/rttx-server/`
- the client has explicit `ephemeral` / `persistent` workspace policy, no implicit fallback,
  explicit connection-state UI, and shared terminal-handle behavior for managed panes
- pure state extraction is complete in `clients/rttx/src/runtime.rs` (connection state machine,
  failure classification, binding reconciliation) and `clients/rttx/src/workspace_state.rs`
  (managed workspace open/restore, endpoint event processing, inventory recovery)
- the endpoint connection manager lives in `clients/rttx/src/daemon_bridge.rs` with one async
  actor per endpoint, multiplexing workspaces and runtimes on a shared transport
- recovered-workspace synthesis from daemon inventory is implemented and tested
- `window.rs` has been decomposed into `window/mod.rs`, `window/runtime.rs`, `window/actions.rs`,
  `window/sidebar.rs`, `window/terminal.rs`, `window/dialogs.rs`, and `window/input.rs`
- managed workspace lifecycle UX matches the intended contract: `Close Workspace` is authoritative
  and destructive (#192), attach is explicit (#194), terminate/detach removed from normal close
  flow (#195), reconnect actions render per workspace not per pane (#196)
- `SessionMissing` state (RFC-019) handles workspaces whose daemon-side runtime has disappeared
- RFC-016 workspace management v2 provides the action-oriented creation model: `New`, `Connect to
  Existing`, `New Direct`
- RFC-018 connection state machine is implemented as `advance_connection_status` in `runtime.rs`
- RFC-020 terminal response ownership ensures the daemon answers terminal queries independently of
  client attachment

Remaining forward-looking work is tracked in separate RFCs:

- RFC-021 defines the protocol v3 direction (capability negotiation, structured envelopes, typed
  errors)
- RFC-022 proposes a v2 daemon state storage layout (per-session directories, schema versioning,
  dirty-flag writes)
- #319 tracks integration test coverage for `window/runtime.rs` orchestration logic

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
- **R6** — Reconciliation is safe by default. Missing GUI state alone may not delete a daemon
  runtime or pane, but explicit user `Close Workspace` is destructive and must suppress later
  automatic recovery of the closed workspace/runtime.
- **R7** — Layout and presentation belong to the workspace. PTYs, scrollback, runtime CWD/title,
  and process lifetime belong to the runtime.
- **R8** — Pane create/close/split operations must be acknowledged by the runtime before the GUI
  commits the resulting layout mutation.
- **R9** — Multiple windows may connect to the same endpoint. A specific runtime has one writer by
  default; any multi-attach mode must be explicit.
- **R10** — Users must always know the current runtime state: `Starting`, `Connecting`,
  `Connected`, `Reconnecting`, `Blocked`, `Disconnected`, `Recovered`, or `SessionMissing`.
- **R11** — `New Workspace` must never implicitly attach to or surface unrelated existing runtimes.
  Attaching to an existing runtime is a separate explicit action.
- **R12** — Workspace connection state and actions render once per workspace, using the tab/sidebar
  row as the primary control surface. Panes may show passive status only.

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
- terminal response semantics (RFC-020): the daemon answers terminal queries independently of
  client attachment
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

The GUI keeps one connection manager per endpoint (`daemon_bridge.rs`). It:

- establishes the transport (Unix socket locally, SSH stdio remotely)
- multiplexes workspaces and runtimes on that endpoint
- routes protocol messages onto the GTK main loop
- owns reconnect backoff for transient failures
- classifies failures into `transient` or `needs user action` via `ConnectionProblem`

---

## Design

### Workspace state machine

The connection state machine is implemented as a pure function (`advance_connection_status` in
`runtime.rs`) that maps `ConnectionEvent` values to `ConnectionStatus` states. See RFC-018 for the
full specification.

States:

- `Starting`
- `Connecting`
- `Connected`
- `Reconnecting { attempt, retry_in_secs }`
- `Blocked(ConnectionProblem)`
- `Disconnected`
- `Recovered`
- `SessionMissing` — the daemon has no record of this workspace's runtime (RFC-019)

`ConnectionProblem` classifies errors into: `DaemonUnavailable` (transient, auto-retryable),
`VersionMismatch`, `OwnershipConflict`, `PermissionDenied`, `SessionMissing`, `Protocol`, and
`UserActionRequired`.

GTK renders these states in the workspace tab/sidebar row. Panes may disable input and show
minimal passive status, but workspace-level actions such as `Retry now`, `Close Workspace`, or
`Edit connection` do not appear once per pane.

### User-facing workspace lifecycle

These operations match normal user expectations (RFC-016 defines the creation model):

- **New Workspace** — always creates exactly one new workspace and one new runtime according to the
  chosen endpoint/policy. It must not implicitly surface unrelated existing runtimes.
- **Close Workspace** — authoritative destructive action. It removes the workspace from the UI and
  saved state, stops any attached runtime and its processes, and must not auto-resurrect later.
- **Connect to Existing Runtime...** — explicit user action exposed from the new-workspace dialog.
  This is the only ordinary way to surface a runtime that already exists on a daemon but has no
  visible workspace in the GUI.
- **Detach Runtime** — advanced keep-running operation only if retained. It must not appear as part
  of the ordinary close path, and detached runtimes should require explicit later attach rather
  than silently reappearing.
- **Terminate Runtime** — not a primary workspace action. If any explicit runtime-stop action
  remains for advanced flows, it must not leave a disconnected placeholder behind in the normal
  close path.

### Failure classification

**Transient** (`ConnectionProblem::DaemonUnavailable`)
- local daemon still starting
- local socket temporarily unavailable
- SSH timeout or broken pipe
- host reboot / sleep / wake
- daemon restart

These auto-retry with backoff.

**Needs user action** (all other `ConnectionProblem` variants)
- bad credentials (`PermissionDenied`)
- host key verification failure (`UserActionRequired`)
- protocol version mismatch (`VersionMismatch`)
- runtime already owned by another client (`OwnershipConflict`)
- runtime no longer exists on daemon (`SessionMissing`)
- protocol-level errors (`Protocol`)

These stop in `Blocked` with a clear explanation and a user action.

### Reconciliation contract

Use stable ids everywhere:

- `workspace_id`
- `runtime_id`
- `pane_binding_id == daemon_pane_id`

Rules:

1. If a runtime exists and the GUI has matching persisted workspace metadata, reconcile it into
   that workspace.
2. If a runtime exists but the GUI has no matching workspace metadata, do not surface it
   automatically in the normal workspace list; it is eligible only for explicit attach.
3. If a workspace was explicitly closed by the user, inventory refresh must not recreate it.
4. If a workspace is bound to a missing runtime during restore or reconnect, keep the workspace
   and mark it `SessionMissing` (RFC-019) unless the user explicitly closed it.
5. If a runtime has extra panes not present in the layout, add recovered panes.
6. If the layout references missing runtime panes, keep explicit placeholders.
7. Reconciliation may create recovered objects automatically for known workspaces, but it may
   never delete a daemon runtime or pane automatically absent explicit user intent.

### Terminal model

There is one product-level terminal model for managed execution. Any user-facing action such as
search, zoom, copy, paste, title updates, or cwd reporting targets a shared terminal abstraction
(`terminal/handle.rs`) rather than splitting behavior across "direct" and "persistent" paths.

The daemon owns terminal response semantics (RFC-020) so that applications behave identically
whether or not a GUI client is attached.

### Relationship to recipe recovery (RFC-007)

RFC-007 still matters for workspace-owned recovery metadata: bookmarks, SSH targets, retry UX, and
honest replay of what the user asked the pane to do. It is no longer the primary managed execution
architecture. The daemon-backed runtime model is primary, and recipe recovery augments it where
replayable context is useful.

---

## Development Plan

All planned items are implemented:

1. **Terminology and doc cleanup** ✅
   Product docs aligned on `Workspace`, `Runtime`, `Endpoint`, `Policy`, and the no-fallback rule.
2. **Endpoint connection manager** ✅
   `runtime.rs` holds pure connection-state logic; `workspace_state.rs` owns managed-workspace
   transitions; `daemon_bridge.rs` provides the async endpoint-scoped actor with one connection per
   endpoint, multiplexing workspaces and runtimes.
3. **Workspace/runtime binding model** ✅
   Stable ids and binding map persist; restore and reconcile do not depend on layout position.
4. **Homogeneous workspace policy** ✅
   One endpoint and one runtime policy per workspace enforced.
5. **Explicit connection UX** ✅
   `Connecting`, `Reconnecting`, `Blocked`, `Recovered`, and `SessionMissing` UI states with
   connection icon in sidebar prefix and state-specific icons/colors.
6. **Shared terminal abstraction** ✅
   `terminal/handle.rs` provides unified search, zoom, copy, paste, cwd/title tracking for both
   direct and managed terminals.
7. **Authoritative workspace lifecycle** ✅
   `Close Workspace` is destructive and final (#192). Attach is explicit (#194). Advanced daemon
   lifecycle actions removed from normal close flow (#195).
8. **Test strategy** ✅
   Pure state tests in `runtime.rs` (64 tests) and `workspace_state.rs` (40 tests). Runtime
   behavior gate enforces coverage on every PR. GTK integration tests cover wiring and widget
   contracts. Daemon bridge reconnection and error recovery tests in place (#318).

---

## Rejected Behaviors

The following behaviors are rejected by this RFC and are not present in the implementation:

- `Close Workspace` deleting only local GUI metadata while the runtime continues running and later
  reappears through inventory recovery
- `New Workspace` implicitly surfacing unrelated daemon runtimes
- `Terminate Runtime` leaving a disconnected workspace placeholder that requires another close
  action
- rendering workspace-level retry/edit/close actions once per managed pane instead of once per
  workspace

---

## Backlog

All original backlog items have been completed:

1. ~~`#191` — Simplify managed workspace lifecycle UX~~ ✅
2. ~~`#192` — Make `Close Workspace` destructive for managed runtimes~~ ✅
3. ~~`#194` — Add explicit `Attach to Existing Runtime` flow~~ ✅
4. ~~`#195` — Remove terminate/detach from normal close flow~~ ✅
5. ~~`#196` — Move managed reconnect/connection actions from panes to workspace tabs~~ ✅
6. ~~`#193` — Add regression coverage for managed workspace close, attach, and reconnect UX~~ ✅

Test coverage tracked in RFC-010:

- ~~`#318` — Integration tests for daemon bridge reconnection and error recovery~~ ✅
- `#319` — Integration tests for `window/runtime.rs` managed workspace orchestration (open)

---

## Open Questions

- **Q1** — Should multi-attach to the same runtime eventually support `share`, `read-only mirror`,
  or `take over`, and how explicit should that handoff be? Single-writer ownership is enforced;
  `OwnershipConflict` blocks concurrent attach.
- **Q2** — ~~What default scrollback retention and disk cap should each pane use?~~
  *Resolved*: 10 MB per pane for both in-memory scrollback and on-disk scrollback log
  (`services/rttx-server/src/pane.rs`).
- **Q3** — ~~Should local daemon auto-start happen only on demand, or also at login when the user
  opts in?~~
  *Resolved*: On-demand by default. The `auto_start_daemon` preference (default `true`) controls
  whether the GUI auto-starts the local daemon when a managed workspace is created.
- **Q4** — How should the GUI surface endpoint version skew when different hosts run different
  daemon versions? Version mismatch is detected during handshake and surfaced as
  `Blocked(VersionMismatch)`. Cross-host version skew awareness is deferred to protocol v3
  (RFC-021).
- **Q5** — Should daemon reconstruction be surfaced as terminal output, a GUI overlay, or both?
  The `reconstructed` flag is propagated from daemon to client via the protocol; surfacing
  strategy is not yet decided.

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
- [RFC-016: Workspace Management v2](./RFC-016-workspace-management-v2.md)
- [RFC-018: Workspace Connection State Machine](./RFC-018-workspace-connection-state-machine.md)
- [RFC-019: Missing Daemon Session Handling](./RFC-019-missing-session-handling.md)
- [RFC-020: Terminal Response Ownership](./RFC-020-terminal-response-ownership.md)
- [RFC-021: Client/Server Protocol v3](./RFC-021-client-server-protocol-v3.md)
- [RFC-022: Daemon State Storage](./RFC-022-daemon-state-storage.md)
- [`services/rttx-server/README.md`](../services/rttx-server/README.md)
- [Zellij terminal workspace](https://github.com/zellij-org/zellij)
- [Zellij session resurrection docs](https://zellij.dev/documentation/session-resurrection)
- [`pty-process` crate](https://docs.rs/pty-process/latest/pty_process/)
- [`prost` crate](https://docs.rs/prost/latest/prost/) --- protocol buffer serialization
- [`vte` crate](https://docs.rs/vte/latest/vte/) --- terminal escape sequence parser (no GTK)
