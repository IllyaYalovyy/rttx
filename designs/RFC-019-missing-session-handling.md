# RFC-019: Missing Daemon Session Handling

| Field         | Value                                   |
|---------------|-----------------------------------------|
| Status        | Accepted (partially implemented)        |
| Author(s)     | Illya Yalovyy                           |
| Supersedes    | —                                       |
| Superseded by | —                                       |

---

## Summary

A managed workspace holds a reference to a daemon-side runtime session by UUID. That session
can disappear independently of the GUI — the daemon restarts without recovery, another client
kills the runtime, a session times out, or the user deletes it from the CLI. When the GUI
later tries to reattach to that UUID, the attach fails and the error handling today does not
isolate the failure: the user ends up with a stuck workspace, and sibling workspaces sharing
the same endpoint connection can be affected.

This RFC makes missing-session handling explicit and fault-isolated: a missing session
produces a clearly-labelled workspace state, never damages other workspaces on the same
endpoint, and offers the user an explicit, low-surprise way to discover and clean up
orphaned workspaces.

---

## Goals

- **G1** — A failed attach because the target session is missing never degrades the shared
  endpoint connection or any sibling workspace
- **G2** — The affected workspace reaches a named, testable terminal state — not a "still
  connecting" ambiguity
- **G3** — The user can discover orphaned workspaces on demand and close them in a single
  action
- **G4** — Recovery from a missing session never silently deletes workspace state the user
  may still want to see
- **G5** — The feature reuses the existing `ListSessions` RPC and the state machine from
  RFC-018; no new proto messages and no new transport states

## Non-Goals

- **NG1** — Automatic session takeover (NG3 of RFC-016 still applies)
- **NG2** — Recovering a session's pane contents after the daemon loses it — once gone, gone
- **NG3** — Server-side session persistence beyond what RFC-007 already specifies
- **NG4** — Auto-deleting orphaned workspaces without an explicit user action

---

## Background & Motivation

### Current behaviour

The GUI connects to a daemon endpoint once and multiplexes all workspaces for that endpoint
over a single `EndpointConnectionManager` connection (defined in `daemon_bridge.rs`). Each
managed workspace holds a `runtime_id` and uses `attach_session(runtime_id, …)` to rebuild
local state from the daemon's snapshot.

Three failure paths exist when the session UUID no longer exists on the daemon:

1. **At initial attach** — `attach_session` (in `daemon.rs`) returns
   `ServerError { code, message }`. The caller must translate that into workspace state.
2. **After reconnect** — on endpoint reconnect, every workspace re-attaches. Stale UUIDs
   return the same error.
3. **Unsolicited errors during operation** — `dispatch_managed_runtime_message` (in
   `window/runtime.rs`) receives `Error` frames for commands referencing a since-removed
   pane/session. Current code logs at warn level and returns, leaving the workspace in an
   indeterminate state. This was the symptom described in issue
   [#407](https://github.com/IllyaYalovyy/rttx/issues/407) (now closed).

### Why sibling workspaces break today

There is only one bidirectional stream per endpoint. If the failing-attach path panics,
drops the stream, or triggers an endpoint-level reconnect, every workspace sharing that
endpoint loses its connection. RFC-018's state machine describes the transport layer well
but does not yet distinguish *"the session I wanted does not exist"* from *"the transport is
down"*, so the UI treats them the same.

### Why this is worth its own RFC

The scope touches:
- Connection state semantics (extension of RFC-018)
- Workspace lifecycle and persistence
- A new UX surface — the cleanup dialog — and a new menu action
- Error classification across three failure paths

That is broader than a single bug fix and needs agreement on the state machine extension
and on the surprising-behaviour boundary (proactive discovery vs. on-demand).

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Disappearing sessions no longer cascade; clear "session no longer exists" label; one-click cleanup action |
| Contributors | Error classification is explicit; one state per failure mode; RFC-018 state machine extended consistently |
| Packagers    | No impact |

---

## Considered Options

### Option A — Explicit "Refresh from daemon" menu action

User invokes a menu action when they suspect state has drifted. The client queries
`ListSessions` for each active endpoint, marks workspaces with missing sessions as
`SessionMissing`, and presents a cleanup dialog.

**Pros**: Zero surprise — the user asked for it. Predictable timing. No hidden network
traffic. Dovetails with "All Hosts" view in RFC-016.

**Cons**: User must know to invoke it. Until they do, stuck workspaces look disconnected
rather than missing.

### Option B — Proactive reconciliation on every reattach/open

On endpoint reconnect or workspace open, the client lists sessions and reconciles
automatically. Missing sessions move to `SessionMissing` immediately.

**Pros**: No manual action. Disappearing sessions surface as soon as the user looks at
them.

**Cons**: More network chatter. Closes the door on "keep a disconnected workspace around as
a reference" — though this is already awkward today. Less predictable: a user who runs
`rttx-server kill` in a terminal may see the workspace relabel itself before they expected.

### Option C — Hybrid: passive on reconnect, explicit on demand

Only query `ListSessions` as part of the normal reconnect flow (once per endpoint, not per
workspace). Additionally expose the explicit "Refresh" action for on-demand use.

**Pros**: Free detection when the transport comes back — matches when the user is already
expecting state to settle. Manual action covers the steady-state case (daemon stayed up,
but a session was killed elsewhere).

**Cons**: Slightly more implementation work than Option A. Two code paths to test.

---

## Decision

**Chosen option: Option C — Hybrid.**

Rationale:

- Option A alone leaves the common "daemon restarted, session lost" case as a poor
  experience until the user guesses to click Refresh
- Option B is too surprising and expensive for the common case where the daemon is stable
- Option C adds the `ListSessions` call exactly once per reconnect — the user already
  expects state to re-settle at that moment — and keeps the explicit action for the
  "steady-state but stale" case. The incremental cost over Option A is small.

The default behaviour errs toward *visible* rather than *silent*: a missing session is
shown, not auto-closed. Cleanup is always an explicit user action.

---

## Design

### 1. Extend the connection state machine (RFC-018)

**Status: implemented.**

`ConnectionStatus::SessionMissing` was added to the state enum in `runtime.rs`:

```rust
pub enum ConnectionStatus {
    Starting,
    Connecting,
    Connected,
    Reconnecting { attempt: u32, retry_in_secs: u32 },
    Blocked(ConnectionProblem),
    Disconnected,
    Recovered,
    /// The daemon has no record of this workspace's runtime.
    SessionMissing,
}
```

Semantics:

- `SessionMissing` means the endpoint transport is fine, but the daemon has no record of
  this workspace's `runtime_id`.
- It is *durable*: once observed, the workspace stays in `SessionMissing` until the user
  closes it or assigns a new runtime.
- Input is disabled in this state (`accepts_input()` returns `false`).
- The sidebar icon uses the `warning` CSS class (same family as `Disconnected`, distinct
  tooltip: "Session no longer exists on daemon").

Transitions (implemented via `advance_connection_status` and `ConnectionEvent::SessionMissing`):

- `Connecting → SessionMissing` — initial attach returned `ServerError` with the
  session-not-found code (`ERR_SESSION_NOT_FOUND`, code 4)
- `Connected → SessionMissing` — unsolicited error from the daemon identifies the session
  or a bound pane as not found *and* a follow-up `ListSessions` confirms the session is
  gone
- `Disconnected → SessionMissing` — post-reconnect attach failed for the same reason
- `SessionMissing → *` — terminal state except for explicit user action (close workspace,
  or a future "rebind to existing session" action)

### 2. Fault isolation

**Status: partially implemented.**

Error classification uses `classify_connection_problem` in `runtime.rs` to map
`DaemonError::ServerError { code: 4, .. }` (the daemon's `ERR_SESSION_NOT_FOUND`) to
`ConnectionProblem::SessionMissing`. This replaces the originally proposed
`DaemonError::SessionMissing(runtime_id)` typed variant — the classification approach is
simpler and avoids adding a new `DaemonError` variant.

The initial-attach and reconnect paths in `daemon_bridge.rs` now detect
`ConnectionProblem::SessionMissing` and emit `ConnectionStatus::SessionMissing` for the
affected workspace only, without tearing down the endpoint connection.

**Remaining work:** unsolicited `Error` frames in `dispatch_managed_runtime_message`
(`window/runtime.rs`) are still logged and returned without triggering reconciliation.
Step 3 of the development plan addresses this.

Acceptance: a test (`session_missing_does_not_affect_sibling_workspaces` in
`session_lifecycle.rs`) proves that marking one workspace as `SessionMissing` does not
affect a sibling workspace on the same endpoint.

### 3. Reconciliation — the single primitive

**Status: not yet implemented as a standalone helper.**

The original design proposed a `reconcile_workspace_against_daemon` async method on
`Window`. In practice, the implemented approach routes session-missing detection through
the existing `EndpointEvent::WorkspaceConnectionChanged` event in the state machine
(`workspace_state.rs`), which is simpler and avoids adding async I/O to the `Window` type.

The full `ListSessions`-based reconciliation helper (querying the daemon and cross-
referencing against all bound `runtime_id`s) remains unimplemented. It is needed for:
- The reconnect-path bulk reconciliation (Step 5)
- The "Refresh sessions from daemon" menu action (Step 6)

### 4. Trigger points

| Trigger | Scope | Status | Notes |
|---------|-------|--------|-------|
| Initial attach returned `SessionMissing` | Single workspace | ✅ Implemented | No extra `ListSessions` call needed — we already know |
| Unsolicited pane/session-not-found error | Single workspace | ❌ Not yet | Call reconcile to disambiguate "session gone" from "transient race" |
| Endpoint reconnect succeeded | All workspaces on that endpoint | ✅ Implemented | Reattach per workspace; missing sessions detected individually |
| User menu action "Refresh sessions from daemon" | All workspaces on *all* endpoints | ❌ Not yet | Iterates across endpoints; see §5 |

### 5. User action: "Refresh sessions from daemon"

**Status: not yet implemented.**

A new top-level action, available both as:

- An item in the hamburger menu under an existing "Workspaces" section
- A button in the cleanup dialog when any orphans exist

Behaviour:

1. For each endpoint with active workspaces, call `ListSessions`
2. Mark missing workspaces as `SessionMissing`
3. Open the cleanup dialog if any were marked missing or were already in that state

No keyboard shortcut is assigned initially — this is a rare, deliberate action.

### 6. Cleanup dialog

**Status: not yet implemented.**

```
┌──────────────────────────────────────────────┐
│ Orphaned Workspaces                          │
│                                              │
│ These workspaces reference sessions that no  │
│ longer exist on the daemon.                  │
│                                              │
│ ☑  Gameday — dev-box                         │
│ ☑  notes — local                             │
│ ☐  scratch — local                           │
│                                              │
│          [Keep Selected]     [Close Checked] │
└──────────────────────────────────────────────┘
```

- All orphans pre-checked for close
- User unchecks any to keep (they remain in the sidebar as `SessionMissing`)
- "Close Checked" removes the workspace rows and their persistent state
- "Keep Selected" dismisses the dialog without closing anything

The dialog is the only place that closes missing workspaces in bulk. Closing a single
orphan also works from the workspace's own context menu (existing "Close workspace"
already handles this once §2 is in place).

### 7. Per-workspace presentation

**Status: implemented.**

`SessionMissing` workspaces render with:

- **Sidebar icon**: same icon as the endpoint type (`computer-symbolic` for local,
  `network-server-symbolic` for remote) with `warning` CSS class
- **Tooltip**: `Session no longer exists on daemon`
- **Short label**: `Session Missing` (in pane headers)
- **Full label**: `Session no longer exists` (in sidebar summaries)

No confirmation prompt on Close — the session is already gone.

### 8. Persistence

A workspace in `SessionMissing` is still persisted to local state (same as a
`Disconnected` workspace today). On next GUI start, it comes back in the same state and
still appears in the Refresh / Orphan dialog.

This preserves G4: we never silently drop a workspace record, even across restarts, until
the user explicitly closes it.

---

## Goals Alignment

| Goal | How addressed | Status |
|------|---------------|--------|
| G1 — Isolate failed attach | §2 keeps errors per-workspace; endpoint stream is not touched | ✅ Implemented for attach/reconnect paths |
| G2 — Named terminal state | `SessionMissing` added to RFC-018 state enum | ✅ Implemented |
| G3 — Explicit discovery and cleanup | §5 Refresh action + §6 cleanup dialog | ❌ Not yet implemented |
| G4 — No silent deletion | §8 — workspaces persist in `SessionMissing` until user closes | ✅ Implemented |
| G5 — Reuse existing protocol | `ListSessions` RPC already implemented | ✅ No new protocol messages needed |

---

## Development Plan

- [x] **Step 1** — Add `ConnectionStatus::SessionMissing` and its CSS/icon presentation
  *(prerequisite: —)*
- [x] **Step 2** — Classify `ERR_SESSION_NOT_FOUND` (code 4) as
  `ConnectionProblem::SessionMissing` via `classify_connection_problem`; emit
  `ConnectionStatus::SessionMissing` from the attach and reconnect paths in
  `daemon_bridge.rs` *(prerequisite: Step 1)*
- [ ] **Step 3** — Isolate unsolicited daemon-error handling in
  `dispatch_managed_runtime_message` — stop swallowing `Error` frames and trigger
  per-workspace reconciliation instead of logging and returning
  *(prerequisite: Step 1)*
- [ ] **Step 4** — Implement `ListSessions`-based reconciliation that cross-references
  all bound `runtime_id`s for an endpoint against the daemon's session list
  *(prerequisite: Step 2)*
- [ ] **Step 5** — Call reconcile from the reconnect path (once per endpoint)
  *(prerequisite: Step 4)*
- [ ] **Step 6** — Add the "Refresh sessions from daemon" menu action and wire it to
  reconcile across all endpoints *(prerequisite: Step 4)*
- [ ] **Step 7** — Implement the orphan cleanup dialog *(prerequisite: Step 6)*
- [x] **Step 8** — Unit tests for `SessionMissing` state transitions and fault isolation;
  integration test (`session_missing_does_not_affect_sibling_workspaces`) proving a
  missing session on one workspace does not affect event delivery to a sibling
  *(prerequisite: Step 1)*

---

## Open Questions

- [ ] **Q1** — Should `SessionMissing` workspaces participate in auto-reconnect (they
  currently would via the shared endpoint reconciliation)? Proposed: no — once marked, only
  an explicit user action (close, or future rebind) transitions out of the state.
- [x] **Q2** — ~~Should the daemon expose a distinct error code for "session not found" so
  the client does not have to pattern-match on text?~~ **Resolved.** The daemon uses
  `ERR_SESSION_NOT_FOUND` (code 4), defined in `protocol.rs`. The client maps this via
  `classify_connection_problem` to `ConnectionProblem::SessionMissing`.
- [ ] **Q3** — Should the Refresh action also verify that direct workspaces' local
  daemons are reachable, or remain strictly scoped to managed workspaces?

---

## References

- [RFC-007: Session Recovery](./RFC-007-session-recovery.md)
- [RFC-016: Workspace Management v2](./RFC-016-workspace-management-v2.md)
- [RFC-018: Workspace Connection State Machine](./RFC-018-workspace-connection-state-machine.md)
- GitHub issue [#478](https://github.com/IllyaYalovyy/rttx/issues/478) (closed — RFC written)
- GitHub issue [#407](https://github.com/IllyaYalovyy/rttx/issues/407) (closed — swallowed errors fixed for attach/reconnect; unsolicited error path remains)
