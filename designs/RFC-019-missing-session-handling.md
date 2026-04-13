# RFC-019: Missing Daemon Session Handling

| Field         | Value                                   |
|---------------|-----------------------------------------|
| Status        | Draft                                   |
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
over a single `EndpointConnectionManager` connection (`daemon_bridge.rs:156`). Each managed
workspace holds a `runtime_id` and uses `attach_session(runtime_id, …)` to rebuild local
state from the daemon's snapshot.

Three failure paths exist when the session UUID no longer exists on the daemon:

1. **At initial attach** — `attach_session` returns `ServerError { code, message }`
   (`daemon.rs:267`). The caller must translate that into workspace state.
2. **After reconnect** — on endpoint reconnect, every workspace re-attaches. Stale UUIDs
   return the same error.
3. **Unsolicited errors during operation** — `dispatch_managed_runtime_message` receives
   `Error` frames for commands referencing a since-removed pane/session. Current code
   (`window/runtime.rs:712-715`) just logs and returns, leaving the workspace in an
   indeterminate state. This is issue #407.

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

Add one state to the managed-workspace status enum:

```rust
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    SessionMissing,   // NEW
}
```

Semantics:

- `SessionMissing` means the endpoint transport is fine, but the daemon has no record of
  this workspace's `runtime_id`.
- It is *durable*: once observed, the workspace stays in `SessionMissing` until the user
  closes it or assigns a new runtime.
- Input is disabled in this state. The sidebar icon uses the `warning` CSS class (same
  family as `Disconnected`, distinct glyph).

Transitions:

- `Connecting → SessionMissing` — initial attach returned `ServerError` with the
  session-not-found code
- `Connected → SessionMissing` — unsolicited error from the daemon identifies the session
  or a bound pane as not found *and* a follow-up `ListSessions` confirms the session is
  gone
- `Disconnected → SessionMissing` — post-reconnect attach failed for the same reason
- `SessionMissing → *` — terminal state except for explicit user action (close workspace,
  or a future "rebind to existing session" action)

### 2. Fault isolation

All three failure paths must return a per-workspace error, not tear down the endpoint:

- `attach_session`: caller already receives a typed `DaemonError::ServerError { code, … }`.
  Promote the session-not-found code to `DaemonError::SessionMissing(runtime_id)` so
  callers do not pattern-match on integers. No transport-level cleanup.
- Unsolicited `Error` frames in `dispatch_managed_runtime_message`: stop swallowing. When
  the error references a bound pane or session, trigger one-shot reconciliation (see §3)
  for *that workspace only*. The endpoint stream continues to deliver events for other
  workspaces throughout.

Acceptance: a test that kills one session while two are active proves the second session
keeps receiving `Delta` / `TitleChanged` events uninterrupted.

### 3. Reconciliation — the single primitive

Introduce one helper on `Window`:

```rust
async fn reconcile_workspace_against_daemon(
    &self,
    workspace_uuid: &str,
    endpoint: &RuntimeEndpoint,
) -> ReconcileOutcome { … }
```

It:

1. Calls `ListSessions` on the endpoint (already supported by the daemon — `daemon.rs:224`)
2. Compares the workspace's `runtime_id` against the returned list
3. Returns `Present` or `Missing`
4. On `Missing`, sets the workspace status to `SessionMissing`

Everything else in this RFC is a caller of that helper.

### 4. Trigger points

| Trigger | Scope | Notes |
|---------|-------|-------|
| Initial attach returned `SessionMissing` | Single workspace | No extra `ListSessions` call needed — we already know |
| Unsolicited pane/session-not-found error | Single workspace | Call reconcile to disambiguate "session gone" from "transient race" |
| Endpoint reconnect succeeded | All workspaces on that endpoint | One `ListSessions` call, cross-reference against every bound runtime_id |
| User menu action "Refresh sessions from daemon" | All workspaces on *all* endpoints | Iterates across endpoints; see §5 |

### 5. User action: "Refresh sessions from daemon"

A new top-level action, available both as:

- An item in the hamburger menu under an existing "Workspaces" section
- A button in the cleanup dialog when any orphans exist

Behaviour:

1. For each endpoint with active workspaces, call `ListSessions`
2. Mark missing workspaces as `SessionMissing`
3. Open the cleanup dialog if any were marked missing or were already in that state

No keyboard shortcut is assigned initially — this is a rare, deliberate action.

### 6. Cleanup dialog

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

`SessionMissing` workspaces render with:

- **Sidebar icon**: distinct from `Disconnected` — proposed `network-workgroup-symbolic`
  with `warning` CSS class, or `dialog-question-symbolic`
- **Subtitle**: `Session no longer exists` followed by the endpoint label
- **Inline action**: context menu gains a single highlighted **Close** item; the
  pane area shows a small status page with that action rather than an empty pane

No confirmation prompt on Close — the session is already gone.

### 8. Persistence

A workspace in `SessionMissing` is still persisted to local state (same as a
`Disconnected` workspace today). On next GUI start, it comes back in the same state and
still appears in the Refresh / Orphan dialog.

This preserves G4: we never silently drop a workspace record, even across restarts, until
the user explicitly closes it.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Isolate failed attach | §2 keeps errors per-workspace; endpoint stream is not touched |
| G2 — Named terminal state | `SessionMissing` added to RFC-018 state enum |
| G3 — Explicit discovery and cleanup | §5 Refresh action + §6 cleanup dialog |
| G4 — No silent deletion | §8 — workspaces persist in `SessionMissing` until user closes |
| G5 — Reuse existing protocol | `ListSessions` RPC already implemented |

---

## Development Plan

- [ ] **Step 1** — Add `ConnectionStatus::SessionMissing` and its CSS/icon presentation
  *(prerequisite: —)*
- [ ] **Step 2** — Add `DaemonError::SessionMissing(runtime_id)` typed error from
  `attach_session` *(prerequisite: Step 1)*
- [ ] **Step 3** — Isolate unsolicited daemon-error handling in
  `dispatch_managed_runtime_message` — never tear down the endpoint
  *(prerequisite: Step 1)*
- [ ] **Step 4** — Implement `reconcile_workspace_against_daemon` using the existing
  `ListSessions` RPC *(prerequisite: Step 2)*
- [ ] **Step 5** — Call reconcile from the reconnect path (once per endpoint)
  *(prerequisite: Step 4)*
- [ ] **Step 6** — Add the "Refresh sessions from daemon" menu action and wire it to
  reconcile across all endpoints *(prerequisite: Step 4)*
- [ ] **Step 7** — Implement the orphan cleanup dialog *(prerequisite: Step 6)*
- [ ] **Step 8** — Unit tests for `reconcile_workspace_against_daemon` against a fake
  daemon; integration test proving a missing session on one workspace does not stop event
  delivery to a sibling *(prerequisite: Step 4)*

---

## Open Questions

- [ ] **Q1** — Should `SessionMissing` workspaces participate in auto-reconnect (they
  currently would via the shared endpoint reconciliation)? Proposed: no — once marked, only
  an explicit user action (close, or future rebind) transitions out of the state.
- [ ] **Q2** — Should the daemon expose a distinct error code for "session not found" so
  the client does not have to pattern-match on text? Current code uses numeric codes; we
  should document which code maps to `SessionMissing`.
- [ ] **Q3** — Should the Refresh action also verify that direct workspaces' local
  daemons are reachable, or remain strictly scoped to managed workspaces?

---

## References

- [RFC-007: Session Recovery](./RFC-007-session-recovery.md)
- [RFC-016: Workspace Management v2](./RFC-016-workspace-management-v2.md)
- [RFC-018: Workspace Connection State Machine](./RFC-018-workspace-connection-state-machine.md)
- GitHub issue [#478](https://github.com/IllyaYalovyy/rttx/issues/478)
- GitHub issue [#407](https://github.com/IllyaYalovyy/rttx/issues/407) — related symptom from swallowed errors
