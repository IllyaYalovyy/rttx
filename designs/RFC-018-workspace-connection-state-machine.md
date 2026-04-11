# RFC-018: Workspace Connection State Machine

| Field         | Value                                                       |
|---------------|-------------------------------------------------------------|
| Status        | Accepted                                                    |
| Author(s)     | jd2023                                                      |
| Supersedes    | —                                                           |
| Superseded by | —                                                           |

---

## Summary

Managed workspaces should use a small durable connection state machine whose states describe what
is true right now. Recovery from a lost daemon connection is an event, not a long-lived status:
after reconnect and reattach succeed, the workspace is simply `Connected` again.

---

## Goals

- **G1** — Make workspace connection state easy to reason about and hard to misuse
- **G2** — Use the same state semantics for local and remote daemon-backed workspaces
- **G3** — Keep direct terminals outside the daemon connection state machine
- **G4** — Separate durable state from transient user notifications such as "recovered"
- **G5** — Make the state machine testable without GTK or daemon I/O

## Non-Goals

- **NG1** — Implement the state-machine refactor in this RFC PR
- **NG2** — Change the daemon protocol or heartbeat protocol
- **NG3** — Expand investment in direct terminals beyond keeping them as a fallback path
- **NG4** — Change workspace, split, or bookmark UX from RFC-016

---

## Background & Motivation

The current client model includes `ConnectionStatus::Recovered` as a durable status. In practice,
most of the UI treats it as `Connected`:

- input is enabled for both `Connected` and `Recovered`
- the pane header renders both as `Connected`
- the sidebar connection icon uses the same `accent` color for both

That makes `Recovered` a weak state. It represents something that happened in the past, not the
current ability of the workspace to accept input. It also creates a maintenance trap: every new UI,
test, or state transition must remember to treat `Recovered` like `Connected`.

The revised model keeps recovery visible where it is useful, but only as a transient event: a log
entry, toast, one-shot pulse, or activity marker. The stable state after successful recovery is
`Connected`.

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

`Starting`

The client is starting or locating the endpoint daemon. This state is only meaningful when the
client must auto-start a local daemon before connecting. Remote endpoints skip this state and begin
at `Connecting`. Implementations may omit `Starting` if daemon startup is fast enough to be
invisible — the key invariant is that no input is accepted until `Connected`.

`Connecting`

The client is opening a transport, handshaking, creating or attaching the runtime, and reconciling
runtime panes with the workspace layout.

`Connected`

The workspace is attached to a live runtime and terminal input can be delivered to its panes. This
is also the state after a successful reconnect and reattach.

`Reconnecting { attempt, retry_in_secs }`

The connection was lost or a transient connection attempt failed, and automatic reconnect is
scheduled. Input is disabled while the workspace waits for the next attempt.

`Disconnected`

The workspace is not connected and no automatic reconnect is currently active. This covers explicit
user disconnect, user-stopped daemons, retry exhaustion if we add a retry limit, or any future
paused reconnect state. The cause is not encoded in the durable state — if the UI needs to
distinguish "user chose to disconnect" from "retries exhausted," it should check the most recent
transient event (`ReconnectAbandoned` vs user-initiated detach).

`Blocked(ConnectionProblem)`

The connection cannot continue without user action or a non-transient fix. Examples include
permission problems, protocol mismatch, runtime ownership conflict, or an unrecoverable protocol
error.

### Transient Events

Transient events describe something that happened. They are not durable workspace statuses.

`Started`

Endpoint startup began.

`ConnectAttemptStarted`

Transport, handshake, attach, or create work began.

`Connected`

Initial connection succeeded.

`ConnectionLost`

The heartbeat, transport, or daemon event stream detected that the connection is no longer usable.

`RetryScheduled { attempt, retry_in_secs }`

A transient failure will be retried automatically.

`ReconnectSucceeded`

Reconnect and runtime reattach succeeded after a previous loss. This may show a toast, write a log
entry, or briefly pulse the row, but the durable state becomes `Connected`.

`ConnectFailed(ConnectionProblem)`

A connect, attach, or protocol operation failed and has been classified.

`ReconnectAbandoned`

Automatic reconnect stopped or was explicitly paused. The durable state becomes `Disconnected`.

### Transition Rules

- New managed workspace starts in `Starting` when local daemon startup is needed.
- New managed workspace starts in `Connecting` when the endpoint is already expected to exist.
- `Starting` moves to `Connecting` once endpoint startup succeeds.
- `Starting` or `Connecting` moves to `Connected` after handshake and runtime attach/create succeed.
- `Starting` or `Connecting` moves to `Reconnecting` after a transient failure with a scheduled retry.
- `Starting` or `Connecting` moves to `Blocked` after a non-transient failure.
- `Connected` moves to `Reconnecting` when the connection is lost and automatic reconnect is active.
- `Connected` moves to `Disconnected` when the user explicitly disconnects or automatic reconnect is not active.
- `Connected` moves to `Blocked` when a non-transient error arrives on a live connection (e.g., ownership conflict from another client attaching).
- `Reconnecting` moves to `Connecting` when the retry timer fires.
- `Reconnecting` moves to `Connected` when reconnect and reattach succeed.
- `Reconnecting` moves to `Blocked` when a retry hits a non-transient failure.
- `Reconnecting` moves to `Disconnected` when retry is abandoned or paused.
- `Disconnected` moves to `Starting` or `Connecting` when the user requests reconnect.
- `Blocked` moves to `Starting` or `Connecting` only after user action or an explicit reconnect request. Blocked states never auto-resolve — the user must act.

No transition should produce a durable `Recovered` status. Recovery is represented by the
`ReconnectSucceeded` event and a resulting durable `Connected` status.

### Presentation Mapping

The tab/sidebar indicator must keep one color mapping for every managed endpoint type:

| Durable state | Icon color class | Meaning |
|---|---|---|
| `Starting` | `dim-label` | Work is in progress |
| `Connecting` | `dim-label` | Work is in progress |
| `Connected` | `accent` | Input can be delivered |
| `Reconnecting` | `dim-label` | Automatic recovery is pending |
| `Disconnected` | `warning` | Not connected and no retry is active |
| `Blocked` | `error` | User action or non-transient fix required |

Endpoint type controls icon shape only:

| Endpoint | Icon |
|---|---|
| Local managed | `computer-symbolic` |
| Remote managed | `network-server-symbolic` |
| Direct terminal | `utilities-terminal-symbolic` |

Direct terminals may continue to show the terminal icon with the connected presentation while the
child process is active, but direct terminal process state must not be mixed into the daemon
connection state machine.

### Connection Problems

`ConnectionProblem` remains the classification layer between daemon/protocol errors and UI policy.

Transient problems may schedule automatic reconnect. Non-transient problems produce `Blocked`.
The first known transient class is daemon or transport unavailability. Protocol mismatch,
permission denied, runtime ownership conflict, and user-action-required server errors are blocked.

### Compatibility

The connection status is currently runtime UI state held in a `HashMap` on the window object — it
is not persisted to disk. No migration is needed. If a future change persists connection status and
encounters a serialized `Recovered` variant, loading must map it to `Connected` using
`#[serde(default)]` or an equivalent backward-compatible loader pattern.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 | Removes the durable `Recovered` alias and keeps state meanings distinct |
| G2 | Local and remote managed workspaces share the same durable states and color mapping |
| G3 | Direct terminals are explicitly outside the daemon lifecycle model |
| G4 | Recovery is expressed as `ReconnectSucceeded`, not as a durable status |
| G5 | Transition rules can be covered by pure unit tests before GTK or daemon integration tests |

---

## Development Plan

- [ ] **Step 1** — Replace durable `Recovered` with `Connected` in the pure transition model
- [ ] **Step 2** — Add a transient reconnect-success event for logs, toasts, or row pulse feedback
- [ ] **Step 3** — Update pane header and sidebar presentation tests to assert no durable recovered state leaks into UI
- [ ] **Step 4** — Update daemon reconnect flow so successful reattach emits `Connected` plus the transient event
- [ ] **Step 5** — Add integration coverage for heartbeat loss, reconnect scheduling, successful reattach, and blocked failures
- [ ] **Step 6** — Verify direct terminal presentation does not reference `ConnectionStatus` or the managed state machine; add a compile-time or test assertion if any coupling exists

Steps 1–3 can land as a single PR. Steps 4–5 are the core behavioral work. Step 6 is a quick audit.

---

## Open Questions

- [ ] **Q1** — Should `ReconnectSucceeded` show a toast, a one-shot sidebar pulse, both, or only log output?
- [x] **Q2** — Should rttx eventually add retry exhaustion, or should transient reconnect remain unbounded until the user closes or disconnects the workspace? **Decision**: Reconnect remains unbounded for now. The `Disconnected` state is reachable only via explicit user action (detach/close) or future explicit "stop retrying" UI. If retry exhaustion is added later, it will be a separate RFC with UX design for the exhaustion notification.

---

## Implementation Notes

- **Naming convention**: In code, durable states should use the `ConnectionState` type and events
  should use a separate `ConnectionEvent` enum. This avoids confusion between identically-named
  states and events (e.g., `ConnectionState::Connected` vs `ConnectionEvent::Connected`).

---

## References

- [#421 — RFC: revise workspace connection state machine](https://github.com/IllyaYalovyy/rttx/issues/421)
- [RFC-013: Persistent Host Sessions](RFC-013-persistent-host-sessions.md)
- [RFC-016: Workspace Management v2](RFC-016-workspace-management-v2.md)
