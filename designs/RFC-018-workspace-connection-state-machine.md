# RFC-018: Workspace Connection State Machine

| Field         | Value                                                       |
|---------------|-------------------------------------------------------------|
| Status        | Implemented                                                 |
| Author(s)     | jd2023                                                      |
| Supersedes    | —                                                           |
| Superseded by | —                                                           |

---

## Summary

Managed workspaces use a durable connection state machine whose states describe what is true right
now. The state machine is implemented as a pure function (`advance_connection_status`) that maps
events to states without GTK or daemon I/O dependencies.

## Current implementation snapshot (2026-04)

- Durable states live in `ConnectionStatus` (not `ConnectionState` as originally proposed) in
  `clients/rttx/src/runtime.rs`
- Events live in `ConnectionEvent` in the same module
- `advance_connection_status` is a pure event-to-state mapping — the current state parameter is
  accepted for signature symmetry but not consulted; callers are responsible for sending
  semantically valid event sequences
- `ConnectionProblem` classifies daemon/protocol errors into 7 variants; only
  `DaemonUnavailable` is transient (auto-retryable)
- `SessionMissing` was added as a durable state by RFC-019 for workspaces whose daemon-side
  runtime has disappeared
- `Recovered` remains as a durable state — the RFC's original goal of making recovery purely
  transient has not been completed (see [Development Plan](#development-plan))
- Presentation mapping (`connection_icon`, `present_connection_status`) and input gating
  (`accepts_input`) are implemented as pure functions in `runtime.rs`
- The reconnect loop lives in `daemon_bridge.rs` (`EndpointActor`) with linear backoff
  (`delay = min(attempt, max_delay)`) and backoff preservation across reattach failures

---

## Goals

- **G1** — Make workspace connection state easy to reason about and hard to misuse
- **G2** — Use the same state semantics for local and remote daemon-backed workspaces
- **G3** — Keep direct terminals outside the daemon connection state machine
- **G4** — Separate durable state from transient user notifications such as "recovered"
- **G5** — Make the state machine testable without GTK or daemon I/O

## Non-Goals

- **NG1** — ~~Implement the state-machine refactor in this RFC PR~~ (no longer applicable — the
  core state machine is implemented)
- **NG2** — Change the daemon protocol or heartbeat protocol
- **NG3** — Expand investment in direct terminals beyond keeping them as a fallback path
- **NG4** — Change workspace, split, or bookmark UX from RFC-016

---

## Background & Motivation

The original client model included `ConnectionStatus::Recovered` as a durable status. In practice,
most of the UI treats it as `Connected`:

- input is enabled for both `Connected` and `Recovered`
- the pane header renders both as `Connected` (via `short_label()`)
- the sidebar connection icon uses the same `accent` color for both

That makes `Recovered` a weak state. It represents something that happened in the past, not the
current ability of the workspace to accept input. It also creates a maintenance trap: every new UI,
test, or state transition must remember to treat `Recovered` like `Connected`.

The revised model keeps recovery visible where it is useful, but only as a transient event: a log
entry, toast, one-shot pulse, or activity marker. The stable state after successful recovery is
`Connected`. This goal is partially achieved — the state machine and presentation layer are
implemented, but the `Recovered` variant has not yet been collapsed into `Connected`.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Connection indicators become more predictable: recovered workspaces look connected because they are connected |
| Contributors | Runtime lifecycle code has fewer duplicate connected states to handle |
| Packagers    | No packaging impact |

---

## Considered Options

### Option A — Keep `Recovered` as a durable status

**Pros**: No implementation work. Existing tests continue to pass.

**Cons**: Keeps a misleading state that mostly aliases `Connected`. Every UI path must special-case
it. The state machine describes history instead of current truth.

### Option B — Add more durable history states

Examples: `RecoveredRecently`, `Lost`, `RetryPaused`, `AttachRecovered`.

**Pros**: Could encode richer lifecycle history directly in the status enum.

**Cons**: Makes the central state machine more complex and pushes notification history into every
consumer. Most UI only needs to know whether input is accepted, reconnect is in progress, or user
action is required.

### Option C — Keep durable state minimal and emit recovery as an event

**Pros**: Durable state answers one question: "what is true now?" Recovery remains observable
without forcing every consumer to model it as a separate current state.

**Cons**: Consumers that want to show recovery feedback need to listen for a separate event instead
of checking the durable status.

---

## Decision

**Chosen option: Option C** — minimal durable states plus transient lifecycle events.

Rationale: rttx favors rock-solid state handling over elaborate lifecycle presentation. A workspace
that has successfully reattached is connected. Historical context belongs in events, logs, and
one-shot UI feedback, not in the durable status enum.

---

## Design

### Scope

This state machine applies only to daemon-backed workspaces:

- local persistent or ephemeral workspaces
- remote persistent or ephemeral workspaces

Direct terminals do not attach to a daemon-owned runtime and must not participate in this state
machine. Direct terminal process lifecycle should remain a separate terminal concern.

### Durable States

The `ConnectionStatus` enum in `clients/rttx/src/runtime.rs` defines the following variants:

`Starting`

The client is starting or locating the endpoint daemon. Emitted for local endpoints where the
daemon may need to be auto-started. Remote endpoints skip this state and begin at `Connecting`.

`Connecting`

The client is opening a transport, handshaking, creating or attaching the runtime, and reconciling
runtime panes with the workspace layout.

`Connected`

The workspace is attached to a live runtime and terminal input can be delivered to its panes. This
is the target state after both initial connection and reconnection (once `Recovered` is removed).

`Reconnecting { attempt, retry_in_secs }`

The connection was lost or a transient connection attempt failed, and automatic reconnect is
scheduled. Input is disabled while the workspace waits for the next attempt. The `attempt` counter
drives linear backoff: `delay = min(attempt, max_delay)` where `max_delay` defaults to 10 seconds.

`Blocked(ConnectionProblem)`

The connection cannot continue without user action or a non-transient fix. Blocked states never
auto-resolve — the user must act (e.g., via "Retry Connection" in the workspace context menu).

`Disconnected`

The workspace is not connected and no automatic reconnect is currently active. Currently reachable
only as a transient intermediate state during the `Disconnected` → `Reconnecting` emission
sequence in `handle_disconnect`. The cause is not encoded in the durable state.

`Recovered` *(to be removed — see Development Plan)*

Emitted after successful reattach during reconnection. Functionally identical to `Connected`:
`accepts_input()` returns true, `short_label()` returns "Connected", and `connection_icon` uses
the `accent` CSS class. Exists only in the reconnect handler in `daemon_bridge.rs`. The original
RFC design calls for replacing this with a `Connected` emission plus a transient
`ReconnectSucceeded` event.

`SessionMissing` *(added by RFC-019)*

The daemon has no record of this workspace's runtime. Emitted when an attach or command fails with
server error code 4 (session not found). Does not trigger auto-reconnect. The user can close the
orphaned workspace or retry. See RFC-019 for the full design.

### Events

The `ConnectionEvent` enum drives state transitions via `advance_connection_status`. The function
maps each event directly to a state without consulting the current state:

| Event | Resulting State |
|---|---|
| `Started` | `Starting` |
| `Connected` | `Connected` |
| `Lost` | `Disconnected` |
| `RetryScheduled { attempt, retry_in_secs }` | `Reconnecting { attempt, retry_in_secs }` |
| `Failed(ConnectionProblem)` | `Blocked(ConnectionProblem)` |
| `Recovered` | `Recovered` |
| `SessionMissing` | `SessionMissing` |

The original RFC proposed separate "transient event" names (`ConnectAttemptStarted`,
`ConnectionLost`, `ReconnectSucceeded`, `ConnectFailed`, `ReconnectAbandoned`). The implementation
uses shorter names and does not distinguish transient events from state-producing events — every
`ConnectionEvent` variant produces a `ConnectionStatus`. The `ReconnectAbandoned` event was not
implemented because reconnect is unbounded (see Q2).

### Transition Rules

The `advance_connection_status` function accepts a `_current` state parameter but does not use it.
All transitions are event-driven with no guard conditions. The caller (`EndpointActor` in
`daemon_bridge.rs`) is responsible for emitting semantically valid event sequences.

Observed transition sequences in the implementation:

**Initial connection (local endpoint):**
`Starting` → `Connecting` → `Connected`

**Initial connection (remote endpoint):**
`Connecting` → `Connected`

**Connection lost with auto-reconnect:**
`Connected` → `Disconnected` → `Reconnecting` → `Connecting` → `Recovered`

**Transient failure during initial connect:**
`Starting`/`Connecting` → `Disconnected` → `Reconnecting`

**Non-transient failure:**
`Starting`/`Connecting` → `Blocked(problem)`

**Session missing during reconnect:**
`Reconnecting` → `SessionMissing`

**Session missing during command:**
any → `SessionMissing`

**Reconnect backoff preservation:** When `ensure_connected` succeeds (resetting the attempt
counter to 0) but the subsequent reattach fails with a transient error, the saved attempt counter
is restored before scheduling the next retry. This prevents backoff from resetting to 1 second
after a partial reconnect success.

**Non-transient retry:** Non-transient failures during reconnect still schedule a retry using
`max_delay` directly, because the underlying problem may resolve (e.g., daemon restarts with a
compatible version).

### Presentation Mapping

Implemented in `connection_icon` and `present_connection_status` in `runtime.rs`.

Icon shape encodes workspace type (constant for the lifetime of the row):

| Endpoint | Icon |
|---|---|
| Local managed | `computer-symbolic` |
| Remote managed | `network-server-symbolic` |
| Direct terminal | `utilities-terminal-symbolic` |

Icon color encodes connection state (changes dynamically):

| Durable state | Icon color class | Tooltip | Input enabled |
|---|---|---|---|
| `Starting` | `dim-label` | "Connecting…" | No |
| `Connecting` | `dim-label` | "Connecting…" | No |
| `Connected` | `accent` | "Connected to local/remote runtime" | Yes |
| `Recovered` | `accent` | "Connected to local/remote runtime" | Yes |
| `Reconnecting` | `dim-label` | "Connecting…" | No |
| `Disconnected` | `warning` | "Disconnected from runtime" | No |
| `SessionMissing` | `warning` | "Session no longer exists on daemon" | No |
| `Blocked` | `error` | "Connection blocked" | No |

Direct terminals may continue to show the terminal icon with the connected presentation while the
child process is active, but direct terminal process state must not be mixed into the daemon
connection state machine.

### Connection Problems

`ConnectionProblem` classifies daemon/protocol errors into reconnectable vs blocked UI policy.
Implemented in `runtime.rs` with mapping logic in `classify_connection_problem`.

| Variant | Transient | Description |
|---|---|---|
| `DaemonUnavailable` | Yes | I/O error or daemon disconnected; triggers auto-reconnect |
| `VersionMismatch` | No | Protocol version incompatibility during handshake |
| `OwnershipConflict` | No | Runtime already owned by another client (server error 9 or `AttachBlocked`) |
| `PermissionDenied` | No | Access denied |
| `SessionMissing` | No | Daemon has no record of the requested session (server error 4) |
| `Protocol(String)` | No | Frame-level or unexpected message error |
| `UserActionRequired(String)` | No | Server error requiring user intervention (error 8 or catch-all) |

Only `DaemonUnavailable` is transient. All other problems produce `Blocked` status.

### Compatibility

The connection status is runtime UI state held in a `HashMap<String, ConnectionStatus>` on the
window object — it is not persisted to disk. No migration is needed. If a future change persists
connection status and encounters a serialized `Recovered` variant, loading must map it to
`Connected` using `#[serde(default)]` or an equivalent backward-compatible loader pattern.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 | State machine is a pure function testable without GTK; `Recovered` alias remains as tech debt |
| G2 | Local and remote managed workspaces share the same durable states and color mapping |
| G3 | Direct terminals are explicitly outside the daemon lifecycle model |
| G4 | Partially achieved — `Recovered` is still durable; collapsing it to `Connected` + transient event is pending |
| G5 | `advance_connection_status` and presentation functions are covered by pure unit tests |

---

## Development Plan

- [x] **Step 1** — Implement the pure connection state machine (`ConnectionStatus`,
  `ConnectionEvent`, `advance_connection_status`) — *done: `runtime.rs`*
- [x] **Step 2** — Implement presentation mapping (`connection_icon`, `present_connection_status`,
  `ConnectionPresentation`) — *done: `runtime.rs`*
- [x] **Step 3** — Implement connection problem classification (`ConnectionProblem`,
  `classify_connection_problem`) — *done: `runtime.rs`*
- [x] **Step 4** — Integrate state machine into daemon reconnect flow (`EndpointActor` in
  `daemon_bridge.rs`) with linear backoff and backoff preservation — *done*
- [x] **Step 5** — Add unit and integration test coverage for state transitions, reconnect
  scheduling, backoff preservation, and problem classification — *done:
  `tests/session_lifecycle.rs`, `tests/reconnect_scheduling.rs`, `tests/gtk_boundary_contracts.rs`*
- [x] **Step 6** — Add `SessionMissing` state for orphaned workspaces per RFC-019 — *done*
- [ ] **Step 7** — Replace durable `Recovered` with `Connected` and add a transient
  `ReconnectSucceeded` event for logs, toasts, or row pulse feedback
- [ ] **Step 8** — Update pane header and sidebar presentation tests to assert no durable
  recovered state leaks into UI
- [ ] **Step 9** — Verify direct terminal presentation does not reference `ConnectionStatus` or
  the managed state machine; add a compile-time or test assertion if any coupling exists

Steps 7–8 collapse the `Recovered` variant. Step 9 is a quick audit.

---

## Open Questions

- [ ] **Q1** — Should `ReconnectSucceeded` show a toast, a one-shot sidebar pulse, both, or only
  log output? The current implementation uses a durable `Recovered` status as the feedback
  mechanism. Once `Recovered` is removed (Step 7), this question must be answered to decide the
  replacement feedback channel.
- [x] **Q2** — Should rttx eventually add retry exhaustion, or should transient reconnect remain
  unbounded until the user closes or disconnects the workspace? **Decision**: Reconnect remains
  unbounded for now. The `Disconnected` state is reachable only via explicit user action
  (detach/close) or future explicit "stop retrying" UI. If retry exhaustion is added later, it
  will be a separate RFC with UX design for the exhaustion notification.

---

## Implementation Notes

- **Naming convention**: The RFC originally proposed `ConnectionState` for durable states. The
  implementation uses `ConnectionStatus` instead. Events use `ConnectionEvent`. This naming is
  established and should not be changed without a codebase-wide rename.
- **Event-only mapping**: `advance_connection_status` ignores the current state parameter. This
  simplifies the function but places the burden of valid sequencing on callers. The underscore
  prefix (`_current`) documents this intentionally.
- **Backoff model**: Linear ramp `delay = min(attempt, max_delay)` with `max_delay = 10s`.
  Non-transient errors during reconnect still retry using `max_delay` directly because the
  underlying problem may resolve independently (e.g., daemon restart).

---

## References

- [#421 — RFC: revise workspace connection state machine](https://github.com/IllyaYalovyy/rttx/issues/421)
- [RFC-013: Persistent Host Sessions](RFC-013-persistent-host-sessions.md)
- [RFC-016: Workspace Management v2](RFC-016-workspace-management-v2.md)
- [RFC-019: Missing Daemon Session Handling](RFC-019-missing-session-handling.md)
