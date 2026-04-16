# RFC-021: Client/Server Protocol v3

| Field         | Value                                                       |
|---------------|-------------------------------------------------------------|
| Status        | Review                                                      |
| Author(s)     | jd2023                                                      |
| Supersedes    | -                                                           |
| Superseded by | -                                                           |

---

## Summary

Define the next daemon protocol direction so daemon-backed workspaces can grow around stable
runtime, endpoint, terminal, and recovery semantics instead of one-off wire fields.

The current v2 protocol has grown significantly since this RFC was drafted. Several of the
motivating issues have been resolved within the v2 wire format, and the referenced RFCs (RFC-016,
RFC-018, RFC-020) have reached Implemented status. The remaining protocol gaps — capability
negotiation, request/response correlation, typed errors, structured terminal input events, and
resumable snapshots — still justify a v3 evolution, but the urgency has shifted from terminal
correctness (largely addressed) to protocol structure and evolution mechanics.

Protocol v3 should keep protobuf framing and SSH/stdin transport flexibility, but add capability
negotiation, structured command/event envelopes, resumable snapshots, typed errors, and explicit
ownership/discovery semantics.

---

## Goals

- **G1** - Make protocol evolution deliberate through version and capability negotiation
- **G2** - Give every daemon feature a natural protocol domain: endpoint inventory, runtime
  lifecycle, pane lifecycle, terminal I/O, recovery, and control
- **G3** - Make terminal interaction state first-class so VTE parity bugs are not fixed one
  shortcut at a time
- **G4** - Support reconnect and reattach using explicit state and revisions, not raw scrollback
  replay alone
- **G5** - Support RFC-016 host-aware creation and explicit "connect to existing" discovery
- **G6** - Preserve the transport model: local Unix socket and remote `ssh rttx-server attach-stdio`
  continue to speak the same logical protocol
- **G7** - Make protocol behavior testable with daemon integration tests and a VTE parity harness

## Non-Goals

- **NG1** - Implement protocol v3 in this RFC change
- **NG2** - Add mixed-endpoint workspaces; RFC-016 keeps one workspace on one endpoint
- **NG3** - Invest further in direct terminals beyond keeping them as a fallback path
- **NG4** - Replace protobuf framing or require a network service beyond Unix sockets and SSH stdio
- **NG5** - Provide forever compatibility with every pre-production protocol version

---

## Implementation Snapshot

This section documents what has been built within the v2 protocol since this RFC was drafted,
which motivating issues have been resolved, and what protocol gaps remain.

### Terminal mode state in PaneSnapshot (partial)

The v2 `PaneSnapshot` message now carries five terminal interaction modes:

| Field | Proto type | Tracked in ScreenPerformer |
|-------|-----------|---------------------------|
| `bracketed_paste_mode` | `bool` | ✅ |
| `application_cursor_keys` | `bool` | ✅ |
| `application_keypad` | `bool` | ✅ |
| `mouse_tracking_mode` | `uint32` | ✅ |
| `sgr_mouse_mode` | `bool` | ✅ |

The daemon's `ScreenPerformer` also tracks `focus_event_mode` and `cursor_visible`, but these
are not yet included in the `PaneSnapshot` protobuf message. A future protocol revision should
add these fields so newly-attached clients can restore the correct mode state on reconnection.

`alternate_screen` and `keyboard_protocol` (Kitty progressive enhancement) are not tracked.

### Terminal response ownership (RFC-020 — Implemented)

The daemon now answers DA1, DA2, DSR, and DECRQM queries directly in `ScreenPerformer`. The
`strip_client_queries` function removes these sequences from client-bound output to prevent
duplicate responses. Detached persistent sessions no longer hang waiting for a client VTE to
generate terminal responses.

### Bounded push channels (partial #362)

Client push channels are now bounded (`PUSH_CHANNEL_BOUND = 4096`) with `try_send`. When a
client falls behind, messages are dropped with a warning log. This addresses the memory growth
concern from #362 but does not yet implement the protocol-level `StreamOverflow` resync path
described in this RFC's Section 11.

### Multi-client ownership semantics

The v2 protocol includes `RuntimeAttachMode` (read-write, read-only, take-over) and
`RuntimeClientRole` enums. `AttachBlocked` reports when a runtime cannot be attached. `SessionInfo`
carries `has_write_owner`, `read_only_client_count`, and `current_client_role`. This partially
addresses Section 9 (Ownership and Multi-Client Semantics) but without the explicit lease model
or typed ownership events described there.

### Input sync for daemon-backed panes (#463 — closed)

Input sync now works for managed panes. The client resolves `input_sync_targets` through
`WorkspaceState` and sends input to sibling daemon panes via the connection manager, not just
through local VTE `feed_child`.

### Diagnostics

The v2 protocol includes `GetDiagnostics`/`DiagnosticsReport` messages for runtime health
inspection, which were not anticipated in the original RFC.

### Remaining v2 protocol gaps

The following structural issues from the Background section remain unaddressed in v2:

1. **No compatibility window** — exact `PROTOCOL_VERSION` equality is still enforced
2. **Terminal input is untyped raw bytes** — `Input { data }` remains the only input path
3. **Request/response correlation is implicit** — no request IDs or envelopes
4. **Errors are ad hoc** — `Error { code, message }` lacks typed variants
5. **Runtime/session terminology mismatch** — wire messages still use `Session*` names
6. **Snapshots lack revisions for resync** — no `ResyncRuntime` or chunked scrollback
7. **Endpoint inventory is basic** — `ListSessions` lacks busy/disabled/takeover metadata

---

## Background & Motivation

The current daemon protocol is a length-prefixed protobuf stream with a flat `ClientMessage` and
`ServerMessage` `oneof`. It uses exact `PROTOCOL_VERSION` equality during `Hello`/`HelloAck` and
then exchanges commands and events such as `CreateSession`, `AttachSession`, `Input`, `Resize`,
`Snapshot`, `Delta`, `PaneCreated`, and `Pong`.

That simple shape was a good starting point, but recent work shows several structural problems.

### Current Protocol Limits

1. **No compatibility window.**
   The client and daemon reject each other unless their protocol versions match exactly. This is
   safe, but it prevents additive rollout, feature probing, and mixed client/server upgrades.

2. **Terminal input is untyped raw bytes.**
   `Input { data }` cannot distinguish ordinary text, a key event, bracketed paste, mouse input,
   focus changes, IME commits, compose output, or a terminal-generated response. The daemon sees
   only bytes after the client has already made terminal-semantics decisions.

3. **Terminal output is untyped raw bytes.** *(partially addressed)*
   `Delta { data }` is a useful low-level stream, but it carries no revision and does not identify
   the semantic state changes implied by the bytes. The daemon now parses output for title, cwd,
   DSR replies, DA1, DA2, DECRQM, bracketed paste mode, cursor keys, keypad, mouse modes, focus
   event mode, and cursor visibility. Query sequences are stripped from client-bound output via
   `strip_client_queries`. The remaining gap is that parsed state changes are not emitted as
   separate typed events — the client must infer mode changes from the snapshot on attach.

4. **Snapshots are partially state-oriented.** *(partially addressed)*
   `PaneSnapshot` now carries five terminal mode fields (bracketed paste, cursor keys, keypad,
   mouse tracking, SGR mouse) in addition to scrollback bytes and pane metadata. However,
   `focus_event_mode` and `cursor_visible` are tracked by the daemon but not yet in the proto
   message. Snapshots still lack revisions for delta catch-up and are not chunked for large
   scrollback.

5. **Request and response correlation is implicit.**
   Some operations expect the next non-push message to be their response while asynchronous pushes
   are filtered around it. This works while the protocol is small, but it becomes fragile as more
   inventory, ownership, terminal, and recovery events are added.

6. **Errors are ad hoc.**
   `Error { code, message }` lacks operation context, retryability, user-action classification,
   affected IDs, and stable typed variants. The client must infer UI policy from numeric constants
   and strings.

7. **Runtime/session terminology leaks through the wire.**
   The product model now uses workspace/runtime/endpoint/pane terminology. The protocol still names
   many wire messages `Session*`. That mismatch makes new contributors translate concepts at every
   boundary.

8. **Discovery is not endpoint-aware enough.**
   RFC-016 (now Implemented) needed explicit host selection and "connect to existing" inventory.
   Current `SessionInfo` carries `has_write_owner`, `read_only_client_count`, and
   `current_client_role`, which partially supports busy-session visibility. However, the protocol
   lacks disabled reasons, takeover eligibility metadata, and endpoint-level identity.

9. **Backpressure recovery is implementation-only.** *(partially addressed)*
   Push channels are now bounded with drop-on-full semantics. However, the protocol does not
   describe how a client should detect dropped messages or resynchronize. There is no
   `StreamOverflow` event or `ResyncRuntime` command.

10. **Terminal response ownership is split.** *(largely addressed)*
    The daemon now answers DA1, DA2, DSR, and DECRQM queries directly and strips these from
    client-bound output (RFC-020, Implemented). The remaining gap is that focus in/out sequences
    are still generated by the client, and OSC color/clipboard queries depend on client attachment.

### Issue Pressure

The open issue list and recent terminal regression history point at the same protocol gap from
multiple directions. Since this RFC was drafted, most of the motivating issues have been resolved
within the v2 protocol:

- #457 (closed): managed terminals used stateless key encoding instead of VTE keyboard semantics
- #458 (closed): managed paste needed bracketed paste semantics
- #459 (closed): managed mouse reporting could be preempted by client gestures
- #460 (closed): reattached clients now restore terminal interaction modes from PaneSnapshot
- #461 (closed): daemon now owns terminal response semantics (RFC-020 Implemented)
- #462 (closed): managed terminal input now covers IME and compose
- #463 (closed): input sync now includes daemon-backed panes
- #464 (closed): VTE parity harness added for managed terminal input and mouse behavior
- #478 (closed): missing daemon sessions handled gracefully (RFC-019)
- #362 (open): push channels are now bounded, but protocol-level resync is not yet defined
- #493 (open): RFC tracking issue for protocol v3

The terminal correctness issues (#457–#464) were resolved by improving the v2 protocol and client
implementation rather than waiting for v3. The remaining protocol gaps — version negotiation,
request correlation, typed errors, structured input events, and resync semantics — are structural
improvements that would make future evolution safer but are not blocking current functionality.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | More reliable daemon-backed terminals, clearer existing-session discovery, fewer shortcut/paste/mouse regressions, safer reconnect behavior |
| Contributors | Clear protocol domains and evolution rules; fewer one-off protocol fields and less client/server guesswork |
| Packagers    | No immediate packaging impact; future client/server upgrades can report clearer compatibility errors |

---

## Considered Options

### Option A - Keep v2 and add fields as needed

**Pros**: Minimal immediate work. Existing code continues to compile and tests stay focused on
individual regressions.

**Cons**: Repeats the current failure mode. Every terminal behavior becomes a new special case:
one field for bracketed paste, another for mouse, another for keyboard mode, another for focus
tracking, another for terminal responses. Compatibility remains exact-version only.

### Option B - Replace the daemon protocol with a terminal-emulator protocol

The daemon would become a full terminal emulator and transmit high-level cell-grid operations to
the client instead of raw byte deltas.

**Pros**: Strong ownership of terminal state. Detached and reattached clients could be purely
state-driven.

**Cons**: Too large for the current project stage. rttx should not rewrite VTE. This would also
increase the risk of diverging from GNOME Terminal behavior, which is the compatibility target.

### Option C - Evolve protobuf into a versioned command/event protocol

Keep the transport and protobuf tooling, but introduce a v3 handshake, capability negotiation,
request/event envelopes, typed protocol domains, authoritative terminal interaction state, and
resync semantics.

**Pros**: Directly addresses the structural issues while preserving what works today. It lets v2
and v3 coexist during migration and creates a natural home for RFC-016, RFC-018, terminal parity,
and reconnect work.

**Cons**: Requires a careful migration plan and a temporary dual-stack period.

---

## Decision

**Chosen option: Option C** - evolve protobuf into a versioned command/event protocol.

Rationale: The daemon architecture is the right product direction, but the wire contract is still
too primitive. A structured v3 protocol fixes the root design gap without rewriting terminal
emulation or inventing a new transport.

---

## Design

### 1. Version And Capability Negotiation

Protocol v3 replaces exact-version equality with negotiated compatibility.

Conceptual handshake:

```protobuf
message ClientHello {
  uint32 min_protocol_version = 1;
  uint32 max_protocol_version = 2;
  bytes client_id = 3;
  string client_name = 4;
  string client_version = 5;
  repeated Capability capabilities = 6;
}

message ServerHello {
  uint32 negotiated_protocol_version = 1;
  bytes server_id = 2;
  string server_version = 3;
  repeated Capability capabilities = 4;
}
```

Negotiation rules:

- The client sends the lowest and highest protocol versions it can speak.
- The server selects the highest mutually supported version.
- If there is no overlap, the server returns a typed `ProtocolMismatch` error.
- Capabilities advertise feature support inside a negotiated major protocol.
- Unknown capability values must be ignored unless explicitly marked required by the command.
- Incompatible semantic changes still bump `PROTOCOL_VERSION`.

Initial capabilities should include:

- `CAP_RUNTIME_INVENTORY_V2`
- `CAP_REQUEST_RESPONSE_IDS`
- `CAP_TERMINAL_MODE_STATE`
- `CAP_TERMINAL_INPUT_EVENTS`
- `CAP_BRACKETED_PASTE_INTENT`
- `CAP_MOUSE_INPUT_EVENTS`
- `CAP_TERMINAL_RESPONSES`
- `CAP_RESYNC_FROM_REVISION`
- `CAP_BOUNDED_STREAMS`
- `CAP_RUNTIME_TAKEOVER`

### 2. Command/Event Envelopes

Protocol v3 should make correlation and stream ordering explicit.

```protobuf
message ClientEnvelope {
  uint64 request_id = 1;
  Command command = 2;
}

message ServerEnvelope {
  uint64 request_id = 1;      // zero for unsolicited events
  uint64 sequence = 2;        // monotonically increasing per connection
  oneof payload {
    CommandResult result = 3;
    Event event = 4;
    ProtocolError error = 5;
  }
}
```

Rules:

- Every client command that expects acknowledgement has a non-zero `request_id`.
- Server responses echo the `request_id`.
- Server push events use `request_id = 0`.
- `sequence` is per connection and helps detect dropped or reordered server events.
- Runtime and pane revisions remain domain revisions and are not replaced by connection sequence.

This removes the current "read until the next non-push message" pattern from the client actor.

### 3. Protocol Domains

Commands and events should be grouped around stable domains.

#### Control Domain

- handshake
- ping/pong or liveness probe
- protocol error
- graceful shutdown
- capability query

#### Endpoint Inventory Domain

- list available runtimes for the selected endpoint
- report attached/busy/read-only/takeover-eligible state
- report endpoint metadata needed by RFC-016 dialogs
- support explicit refresh and future subscriptions

#### Runtime Lifecycle Domain

- create runtime
- attach runtime
- detach runtime
- terminate runtime
- rename runtime
- request takeover
- report runtime created, attached, detached, terminated, renamed, blocked, or missing

#### Pane Lifecycle Domain

- create pane
- close pane
- resize pane
- set pane title
- report pane created, closed, resized, exited, title changed, or cwd changed

#### Terminal I/O Domain

- raw output delta
- terminal state delta
- raw input fallback
- key input intent
- paste intent
- mouse input intent
- focus input intent
- terminal response input

#### Recovery Domain

- attach snapshot
- resync from revision
- full snapshot fallback
- missing runtime reconciliation
- stream overflow or backpressure recovery

### 4. Terminal Input Model

The existing `Input { data }` should remain as a low-level escape hatch during migration, but it
must stop being the only terminal input path.

Conceptual input command:

```protobuf
message TerminalInput {
  bytes runtime_id = 1;
  bytes pane_id = 2;

  oneof kind {
    bytes raw_bytes = 3;
    KeyInput key = 4;
    PasteInput paste = 5;
    MouseInput mouse = 6;
    FocusInput focus = 7;
    TerminalResponse response = 8;
  }
}
```

Semantics:

- `raw_bytes` is for compatibility and intentionally low-level.
- `key` carries enough GTK/VTE event data for the daemon/client contract to choose the correct
  encoded bytes for the active terminal mode.
- `paste` carries paste text plus paste policy, so bracketed paste wrapping is not lost.
- `mouse` carries coordinates, button, modifiers, press/release/motion/wheel, and the source
  coordinate space.
- `focus` represents focus in/out when terminal focus reporting is active.
- `response` is for terminal-generated replies that are intentionally fed back to the PTY.

The first implementation does not need to encode every key and mouse protocol immediately. The
important architectural change is that the wire format can distinguish intent from bytes.

### 5. Terminal Interaction State

The daemon must own the terminal interaction state needed to preserve behavior across detach,
reattach, reconnect, and multiple clients. The v2 protocol already carries five mode fields in
`PaneSnapshot`; v3 should consolidate these into a single `TerminalModeState` message and add
the modes that the daemon tracks but does not yet expose over the wire.

Conceptual state:

```protobuf
message TerminalModeState {
  bool bracketed_paste = 1;
  bool focus_reporting = 2;
  bool application_cursor_keys = 3;
  bool application_keypad = 4;
  bool alternate_screen = 5;
  bool cursor_visible = 6;
  MouseMode mouse_mode = 7;
  KeyboardProtocol keyboard_protocol = 8;
}
```

Current tracking status in the daemon's `ScreenPerformer`:

| Mode | Tracked | In PaneSnapshot |
|------|---------|-----------------|
| bracketed paste | ✅ | ✅ |
| application cursor keys | ✅ | ✅ |
| application keypad | ✅ | ✅ |
| mouse tracking mode | ✅ | ✅ |
| SGR mouse mode | ✅ | ✅ |
| focus event mode | ✅ | ❌ (not in proto) |
| cursor visible | ✅ | ❌ (not in proto) |
| alternate screen | ❌ | ❌ |
| keyboard protocol | ❌ | ❌ |

The immediate next step is adding `focus_event_mode` and `cursor_visible` to the v2 `PaneSnapshot`
message. The full `TerminalModeState` consolidation can happen as part of v3.

`PaneSnapshot` should carry the full `TerminalModeState`. Live changes should be emitted as a
`TerminalModeChanged` event with runtime ID, pane ID, revision, and the updated mode state.

This is the protocol-level replacement for adding separate snapshot booleans every time a terminal
mode bug is found.

### 6. Terminal Output And Responses

Raw PTY bytes remain the primary output stream because VTE should continue to render the terminal.
The daemon now parses enough output to maintain authoritative interaction state and synthesizes
daemon-owned replies for DA1, DA2, DSR, and DECRQM queries (RFC-020, Implemented). Query sequences
are stripped from client-bound output via `strip_client_queries` to prevent duplicate responses.

Rules:

- `OutputDelta` carries raw bytes and a pane revision or output revision.
- Parsed semantic changes are emitted as separate typed events when they affect client state.
- Terminal-generated replies are explicit `TerminalResponse` writes, not indistinguishable user
  input.
- Detached runtimes must not depend on an attached client VTE to keep terminal modes coherent.

The daemon already satisfies the last rule for DA1, DA2, DSR, and DECRQM. Focus in/out sequences
remain client-generated since only the GUI knows focus state. OSC color and clipboard queries
remain client-dependent and are rare enough that the current behavior is acceptable.

### 7. Snapshot, Revision, And Resync

Snapshots should describe current runtime state, not only replay bytes.

```protobuf
message RuntimeSnapshot {
  bytes runtime_id = 1;
  uint64 runtime_revision = 2;
  RuntimeClientRole client_role = 3;
  repeated PaneSnapshot panes = 4;
}

message PaneSnapshot {
  bytes pane_id = 1;
  uint64 pane_revision = 2;
  string title = 3;
  string cwd = 4;
  uint32 cols = 5;
  uint32 rows = 6;
  optional int32 exit_status = 7;
  TerminalModeState terminal_modes = 8;
  repeated OutputChunk scrollback = 9;
}
```

Resync rules:

- Every runtime/pane mutation that can affect a reattached client has a revision.
- Raw output deltas carry a pane output revision.
- A client may request `ResyncRuntime { runtime_id, since_revision }`.
- The server may return either a delta catch-up or a full snapshot.
- Oversized scrollback is chunked instead of relying on one message below `MAX_MESSAGE_SIZE`.
- If the daemon no longer has the runtime, it returns a typed `RuntimeMissing` result so the
  client can apply RFC-019-style missing-session handling without breaking sibling workspaces.

### 8. Endpoint Inventory And Busy Runtime Discovery

RFC-016 (now Implemented) required an explicit "connect to existing" dialog for a selected host.
The v2 protocol partially supports this through `SessionInfo` fields: `has_write_owner`,
`read_only_client_count`, `current_client_role`, and `attached_client_count`. Protocol v3 should
extend this into a richer inventory model.

Inventory entries should include:

- runtime ID
- display name
- policy
- pane count
- active pane summary
- current lifecycle state
- current client role for this client
- write-owner presence
- read-only client count
- whether this client may attach read-write, attach read-only, or request takeover
- disabled reason when the runtime is visible but not selectable
- revision for freshness

Busy runtimes should be visible but disabled in the UI. The protocol should return enough reason
data for the client to explain why a runtime cannot be selected.

### 9. Ownership And Multi-Client Semantics

The v2 protocol already defines ownership as a partial model:

- `RuntimeAttachMode` supports read-write, read-only, and take-over attach requests
- `RuntimeClientRole` reports unattached, writer, or reader status
- `AttachBlocked` signals when a runtime cannot be attached
- `SessionInfo` carries `has_write_owner` and `read_only_client_count`

Protocol v3 should formalize this into a first-class runtime lease model:

- one writer lease per runtime
- zero or more readers
- same client cannot attach twice to the same runtime as separate workspaces
- takeover is an explicit command, not a side effect of attach
- lease loss, owner disconnect, and forced takeover are typed events

This aligns with RFC-016's visible busy-session model and avoids making the client infer policy
from `AttachBlocked` alone.

### 10. Error Model

Replace ad hoc numeric errors with typed protocol errors.

```protobuf
message ProtocolError {
  ErrorKind kind = 1;
  string message = 2;
  string operation = 3;
  repeated ErrorTarget targets = 4;
  bool retryable = 5;
  bool user_action_required = 6;
}
```

Initial error kinds:

- `PROTOCOL_MISMATCH`
- `UNSUPPORTED_CAPABILITY`
- `INVALID_ARGUMENT`
- `AUTHENTICATION_FAILED`
- `PERMISSION_DENIED`
- `ENDPOINT_UNAVAILABLE`
- `RUNTIME_NOT_FOUND`
- `PANE_NOT_FOUND`
- `OWNERSHIP_CONFLICT`
- `TAKEOVER_REQUIRED`
- `STREAM_OVERFLOW`
- `INTERNAL`

The client should map typed errors to `ConnectionProblem` and UI policy without string matching.

### 11. Backpressure And Bounded Streams

The daemon now uses bounded push channels (`PUSH_CHANNEL_BOUND = 4096`) with `try_send`
drop-on-full semantics. This prevents unbounded memory growth but does not give the client a way
to detect or recover from dropped messages.

Protocol v3 should pair the existing bounded channels with an explicit recovery path:

- Server push channels remain bounded (already implemented).
- If a client falls behind and lossy dropping is unsafe, the server marks the client out of sync.
- The client receives `StreamOverflow` or detects a missing sequence and requests resync.
- The server sends a full snapshot if it cannot provide safe delta catch-up.

This turns #362 from an implementation-only queue change into a protocol behavior with defined
client recovery.

### 12. Naming

New v3 messages should use product terminology:

- endpoint
- workspace only in client UI state, not daemon protocol identity
- runtime
- pane
- place/command only in client-side host-aware UX, unless a future daemon feature owns them

Existing v2 `Session*` names can remain inside compatibility code until v2 is removed.

### 13. Backward Compatibility

During migration:

- Keep v2 support until the v3 client and server are both implemented.
- The daemon may accept v2 and v3 on the same socket/stdio transport.
- New v3-only behavior must be capability-gated.
- The client should present clear protocol mismatch errors when versions cannot negotiate.
- Because rttx is not production-stable yet, v2 removal does not require persisted protocol
  migration once all active development builds have moved to v3.

Persisted state changes caused by v3 implementation must still use the existing
`#[serde(default)]` compatibility pattern.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 | Negotiated versions and capabilities replace exact equality and one-off version bumps |
| G2 | Protocol domains make runtime, pane, terminal, inventory, recovery, and control behavior explicit |
| G3 | Terminal input events and `TerminalModeState` provide a stable home for VTE parity work |
| G4 | Revisioned snapshots and resync define reconnect behavior beyond raw scrollback replay |
| G5 | Endpoint inventory includes busy/disabled runtime discovery for RFC-016 |
| G6 | The design keeps protobuf framing over Unix socket and SSH stdio |
| G7 | Request IDs, typed events, and state snapshots are directly testable |

---

## Development Plan

- [x] **Step 1** - Land this RFC and review the protocol domains *(prerequisite: -)*
- [x] **Step 2** - Add a VTE parity harness for managed terminal input and mouse behavior
  *(prerequisite: Step 1; closed #464)*
- [ ] **Step 3** - Add v3 handshake, capability negotiation, envelopes, and typed errors while
  preserving v2 compatibility *(prerequisite: Step 1)*
- [ ] **Step 4** - Add revisioned runtime/pane snapshots and resync commands
  *(prerequisite: Step 3; supports #362 resync path)*
- [x] **Step 5** - Add `TerminalModeState` to snapshots and mode-change events
  *(prerequisite: Step 4; closed #458 and #460)* — partially done: five modes are in
  `PaneSnapshot`; `focus_event_mode` and `cursor_visible` are tracked but not yet in the proto
  message; full `TerminalModeState` consolidation deferred to v3
- [x] **Step 6** - Add terminal input intent commands for paste, key, mouse, focus, IME/compose,
  and terminal responses *(prerequisite: Step 5; closed #457, #459, #461, and #462)* — resolved
  within v2 by improving client-side VTE integration and daemon response ownership (RFC-020)
  rather than adding structured input commands; the v3 `TerminalInput` design remains valid for
  future structured input
- [ ] **Step 7** - Add endpoint inventory v2 for host-aware "connect to existing" flows
  *(prerequisite: Step 3; supports RFC-016)* — RFC-016 is Implemented with v2 `SessionInfo`
  fields; richer inventory metadata deferred to v3
- [ ] **Step 8** - Define bounded push-channel behavior and stream overflow resync
  *(prerequisite: Step 4; supports #362)* — bounded channels implemented; protocol-level resync
  not yet defined
- [ ] **Step 9** - Remove v2 compatibility after the v3 client/server path is stable
  *(prerequisite: Steps 3-8)*

---

## Open Questions

- [ ] **Q1** - How much keyboard encoding should live in the daemon versus a shared
  client/server terminal-input crate? The current implementation keeps keyboard encoding in the
  client (VTE commit path). The v3 `TerminalInput` design would move structured key events to the
  wire, but the practical need is reduced now that VTE parity issues (#457) are resolved.
- [ ] **Q2** - Should terminal mode tracking remain a minimal state parser, or should we adopt
  more of VTE's parser behavior through shared tests before adding features? The daemon's
  `ScreenPerformer` has grown to handle DA1, DA2, DSR, DECRQM, and seven tracked modes. This
  is already beyond "minimal" but well short of a full terminal emulator.
- [x] **Q3** - Should read-only clients receive all terminal state deltas immediately, or only
  after attach snapshot plus output stream subscription? **Resolved**: read-only clients receive
  a full snapshot on attach followed by live deltas, same as write clients. The v2 implementation
  does not distinguish delta delivery by client role.
- [x] **Q4** - What is the exact v2 removal point for a pre-production project? **Resolved**:
  v2 removal happens after v3 client and server are both stable (Step 9). Since rttx is
  pre-production, there is no persisted protocol migration requirement — all active builds move
  to v3 together.
- [ ] **Q5** - Should remote endpoint metadata remain entirely client-owned, or should remote
  daemons expose stable host identity for inventory and troubleshooting?

---

## References

- [RFC-013: Persistent Host Sessions](RFC-013-persistent-host-sessions.md) *(Implemented)*
- [RFC-016: Workspace Management v2](RFC-016-workspace-management-v2.md) *(Implemented)*
- [RFC-018: Workspace Connection State Machine](RFC-018-workspace-connection-state-machine.md) *(Implemented)*
- [RFC-019: Missing Session Handling](RFC-019-missing-session-handling.md) *(Accepted, partially implemented)*
- [RFC-020: Terminal Response Ownership](RFC-020-terminal-response-ownership.md) *(Implemented)*
- [Issue #362: replace unbounded push channel with bounded channel](https://github.com/IllyaYalovyy/rttx/issues/362) *(open — bounded channels implemented, protocol-level resync not yet defined)*
- [Issue #457: managed terminals use stateless key encoding](https://github.com/IllyaYalovyy/rttx/issues/457) *(closed)*
- [Issue #458: managed paste ignores bracketed paste mode](https://github.com/IllyaYalovyy/rttx/issues/458) *(closed)*
- [Issue #459: managed mouse reporting is preempted by client gestures](https://github.com/IllyaYalovyy/rttx/issues/459) *(closed)*
- [Issue #460: reattached clients do not restore terminal interaction modes explicitly](https://github.com/IllyaYalovyy/rttx/issues/460) *(closed)*
- [Issue #461: daemon does not own full terminal response semantics](https://github.com/IllyaYalovyy/rttx/issues/461) *(closed)*
- [Issue #462: managed terminal input lacks explicit IME and compose coverage](https://github.com/IllyaYalovyy/rttx/issues/462) *(closed)*
- [Issue #463: input sync does not include daemon-backed panes](https://github.com/IllyaYalovyy/rttx/issues/463) *(closed)*
- [Issue #464: add VTE parity harness](https://github.com/IllyaYalovyy/rttx/issues/464) *(closed)*
- [Issue #478: handle missing daemon sessions gracefully](https://github.com/IllyaYalovyy/rttx/issues/478) *(closed)*
- [Issue #493: RFC tracking issue for client/server protocol v3](https://github.com/IllyaYalovyy/rttx/issues/493)
- [Issue #614: review and update RFC-021](https://github.com/IllyaYalovyy/rttx/issues/614)
